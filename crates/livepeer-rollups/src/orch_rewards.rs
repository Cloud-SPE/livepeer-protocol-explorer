use anyhow::{Context, Result};
use bigdecimal::BigDecimal;
use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use sqlx::{PgPool, Row};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    str::FromStr,
};
use tracing::info;

const ARBITRUM_CHAIN_ID: i64 = 42161;
const CHECKPOINT_NAME: &str = "rollup_orch_rewards_daily";
const REORG_CHECKPOINT_NAME: &str = "rollup_orch_rewards_daily_reorg";
const REWARD_CUT_DENOMINATOR: i64 = 1_000_000;
const ZERO_RAW_REWARD_CUT: &str = "0";

#[derive(Debug, Default, Serialize)]
pub struct OrchRewardsSummary {
    pub events_seen: u64,
    pub rows_written: u64,
    pub groups_touched: u64,
    pub checkpoint_event_id: Option<i64>,
}

#[derive(Debug, Clone)]
struct RewardEventRow {
    event_id: i64,
    block_number: i64,
    log_index: i32,
    block_timestamp: DateTime<Utc>,
    orchestrator_address: String,
    valuation_version: String,
    total_tokens_native: BigDecimal,
    total_tokens_usd: Option<BigDecimal>,
}

#[derive(Debug, Clone)]
struct RewardCutPoint {
    block_number: i64,
    log_index: i32,
    reward_cut_raw: BigDecimal,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct AggregateKey {
    day_utc: NaiveDate,
    orchestrator_address: String,
    valuation_version: String,
}

#[derive(Debug, Clone)]
struct AggregateRow {
    reward_event_count: i64,
    sum_total_tokens: BigDecimal,
    sum_total_tokens_usd: BigDecimal,
    sum_orch_tokens: BigDecimal,
    sum_orch_tokens_usd: BigDecimal,
    sum_delegators_tokens: BigDecimal,
    sum_delegators_tokens_usd: BigDecimal,
    usd_rows_priced: i64,
    source_max_event_id: i64,
}

impl AggregateRow {
    fn zero() -> Self {
        Self {
            reward_event_count: 0,
            sum_total_tokens: BigDecimal::from(0),
            sum_total_tokens_usd: BigDecimal::from(0),
            sum_orch_tokens: BigDecimal::from(0),
            sum_orch_tokens_usd: BigDecimal::from(0),
            sum_delegators_tokens: BigDecimal::from(0),
            sum_delegators_tokens_usd: BigDecimal::from(0),
            usd_rows_priced: 0,
            source_max_event_id: 0,
        }
    }
}

pub async fn run_once(
    pg: &PgPool,
    include_tentative: bool,
    batch_limit: i64,
) -> Result<OrchRewardsSummary> {
    let mut reward_cut_cache: HashMap<String, Vec<RewardCutPoint>> = HashMap::new();
    let reorg_rows_written =
        process_reorg_mutations(pg, include_tentative, &mut reward_cut_cache).await?;
    let checkpoint = load_checkpoint(pg).await?;
    let rows = fetch_reward_rows(pg, include_tentative, checkpoint, batch_limit).await?;
    if rows.is_empty() {
        // TD-023: still tick `indexer_checkpoints.updated_at` on empty
        // polls so dashboards distinguish "alive and caught up" from
        // "stalled". `GREATEST` in the upsert prevents block regression.
        advance_checkpoint(pg, checkpoint.unwrap_or(0)).await?;
        return Ok(OrchRewardsSummary {
            rows_written: reorg_rows_written,
            checkpoint_event_id: checkpoint,
            ..Default::default()
        });
    }

    let mut aggregates: HashMap<AggregateKey, AggregateRow> = HashMap::new();
    let mut max_event_id = checkpoint.unwrap_or(0);

    for row in &rows {
        let key = AggregateKey {
            day_utc: row.block_timestamp.date_naive(),
            orchestrator_address: row.orchestrator_address.clone(),
            valuation_version: row.valuation_version.clone(),
        };
        let reward_cut_raw = reward_cut_raw_at_block(
            pg,
            &mut reward_cut_cache,
            &row.orchestrator_address,
            row.block_number,
            row.log_index,
        )
        .await?;
        let orch_tokens = share_from_raw_percent(&row.total_tokens_native, &reward_cut_raw);
        let delegators_tokens = &row.total_tokens_native - &orch_tokens;
        let orch_tokens_usd = row
            .total_tokens_usd
            .as_ref()
            .map(|usd| share_from_raw_percent(usd, &reward_cut_raw));
        let delegators_tokens_usd = row
            .total_tokens_usd
            .as_ref()
            .zip(orch_tokens_usd.as_ref())
            .map(|(usd, orch)| usd - orch);

        let agg = aggregates.entry(key).or_insert_with(AggregateRow::zero);
        agg.reward_event_count += 1;
        agg.sum_total_tokens += row.total_tokens_native.clone();
        agg.sum_orch_tokens += orch_tokens;
        agg.sum_delegators_tokens += delegators_tokens;
        if let Some(total_usd) = row.total_tokens_usd.as_ref() {
            agg.sum_total_tokens_usd += total_usd.clone();
            agg.sum_orch_tokens_usd += orch_tokens_usd.expect("orch usd set when total usd set");
            agg.sum_delegators_tokens_usd +=
                delegators_tokens_usd.expect("delegators usd set when total usd set");
            agg.usd_rows_priced += 1;
        }
        agg.source_max_event_id = agg.source_max_event_id.max(row.event_id);
        max_event_id = max_event_id.max(row.event_id);
    }

    let groups_touched = aggregates.len() as u64;
    let mut rows_written = reorg_rows_written;
    for (key, agg) in aggregates {
        upsert_aggregate(pg, &key, &agg).await?;
        rows_written += 1;
    }
    advance_checkpoint(pg, max_event_id).await?;

    let summary = OrchRewardsSummary {
        events_seen: rows.len() as u64,
        rows_written,
        groups_touched,
        checkpoint_event_id: Some(max_event_id),
    };
    info!(?summary, "orch rewards rollup complete");
    Ok(summary)
}

async fn process_reorg_mutations(
    pg: &PgPool,
    include_tentative: bool,
    reward_cut_cache: &mut HashMap<String, Vec<RewardCutPoint>>,
) -> Result<u64> {
    let checkpoint = load_named_checkpoint(pg, REORG_CHECKPOINT_NAME).await?;
    let rows = sqlx::query(
        r#"SELECT m.id AS mutation_id,
                  m.raw_event_id,
                  e.to_address AS orchestrator_address
             FROM reorg_mutations m
             JOIN raw_protocol_events e
               ON e.id = m.raw_event_id
            WHERE m.id > COALESCE($1, 0)
              AND e.chain_id = $2
              AND e.event_name = 'Reward'
              AND e.to_address IS NOT NULL
         ORDER BY m.id ASC"#,
    )
    .bind(checkpoint)
    .bind(ARBITRUM_CHAIN_ID)
    .fetch_all(pg)
    .await?;
    if rows.is_empty() {
        return Ok(0);
    }

    let mut keys = HashSet::new();
    let mut max_mutation_id = checkpoint.unwrap_or(0);
    for row in rows {
        let mutation_id = row.get::<i64, _>("mutation_id");
        let event_id = row.get::<i64, _>("raw_event_id");
        let orchestrator_address = row.get::<String, _>("orchestrator_address");
        max_mutation_id = max_mutation_id.max(mutation_id);

        for key in load_existing_candidate_keys(pg, &orchestrator_address, event_id).await? {
            keys.insert(key);
        }
        for key in load_current_event_keys(pg, include_tentative, event_id).await? {
            keys.insert(key);
        }
    }

    let mut rows_written = 0_u64;
    for key in keys {
        if let Some(aggregate) =
            rebuild_aggregate_for_key(pg, include_tentative, reward_cut_cache, &key).await?
        {
            replace_aggregate(pg, &key, &aggregate).await?;
            rows_written += 1;
        } else {
            delete_aggregate(pg, &key).await?;
        }
    }
    advance_named_checkpoint(pg, REORG_CHECKPOINT_NAME, max_mutation_id).await?;
    Ok(rows_written)
}

async fn fetch_reward_rows(
    pg: &PgPool,
    include_tentative: bool,
    resume_from_event_id: Option<i64>,
    limit: i64,
) -> Result<Vec<RewardEventRow>> {
    let finality_filter = if include_tentative {
        ""
    } else {
        "AND e.finality = 'finalized'"
    };
    let sql = format!(
        r#"SELECT
               e.id AS event_id,
               e.block_number,
               e.log_index,
               e.block_timestamp,
               e.to_address AS orchestrator_address,
               v.valuation_version,
               v.amount_native AS total_tokens_native,
               v.amount_usd AS total_tokens_usd
           FROM raw_protocol_events e
           JOIN event_valuations v
             ON v.event_id = e.id
            AND v.asset = 'LPT'
          WHERE e.chain_id = $1
            AND e.event_name = 'Reward'
            AND e.is_canonical = TRUE
            {finality_filter}
            AND e.to_address IS NOT NULL
            AND ($2::bigint IS NULL OR e.id > $2)
          ORDER BY e.id ASC
          LIMIT $3"#,
    );
    let rows = sqlx::query(&sql)
        .bind(ARBITRUM_CHAIN_ID)
        .bind(resume_from_event_id)
        .bind(limit)
        .fetch_all(pg)
        .await?;

    rows.into_iter()
        .map(|row| {
            Ok(RewardEventRow {
                event_id: row.get("event_id"),
                block_number: row.get("block_number"),
                log_index: row.get("log_index"),
                block_timestamp: row.get("block_timestamp"),
                orchestrator_address: row.get("orchestrator_address"),
                valuation_version: row.get("valuation_version"),
                total_tokens_native: row.get("total_tokens_native"),
                total_tokens_usd: row.get("total_tokens_usd"),
            })
        })
        .collect()
}

async fn reward_cut_raw_at_block(
    pg: &PgPool,
    cache: &mut HashMap<String, Vec<RewardCutPoint>>,
    orchestrator: &str,
    block_number: i64,
    log_index: i32,
) -> Result<BigDecimal> {
    if !cache.contains_key(orchestrator) {
        let rows = sqlx::query(
            r#"SELECT
                   block_number,
                   log_index,
                   COALESCE(raw_event -> 'decoded' ->> 'rewardCut', $3) AS reward_cut_raw
               FROM raw_protocol_events
              WHERE chain_id = $1
                AND is_canonical = TRUE
                AND contract_name = 'BondingManager'
                AND event_name = 'TranscoderUpdate'
                AND to_address = $2
              ORDER BY block_number ASC, log_index ASC"#,
        )
        .bind(ARBITRUM_CHAIN_ID)
        .bind(orchestrator)
        .bind(ZERO_RAW_REWARD_CUT)
        .fetch_all(pg)
        .await?;

        let points = rows
            .into_iter()
            .map(|row| {
                Ok(RewardCutPoint {
                    block_number: row.get("block_number"),
                    log_index: row.get("log_index"),
                    reward_cut_raw: BigDecimal::from_str(&row.get::<String, _>("reward_cut_raw"))
                        .context("parsing rewardCut raw")?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        cache.insert(orchestrator.to_string(), points);
    }

    let points = cache.get(orchestrator).unwrap();
    let mut selected = BigDecimal::from(0);
    for point in points {
        let ordering =
            compare_event_position(point.block_number, point.log_index, block_number, log_index);
        if ordering == Ordering::Greater {
            break;
        }
        selected = point.reward_cut_raw.clone();
    }
    Ok(selected)
}

async fn rebuild_aggregate_for_key(
    pg: &PgPool,
    include_tentative: bool,
    reward_cut_cache: &mut HashMap<String, Vec<RewardCutPoint>>,
    key: &AggregateKey,
) -> Result<Option<AggregateRow>> {
    let source_rows = fetch_reward_rows_for_key(pg, include_tentative, key).await?;
    if source_rows.is_empty() {
        return Ok(None);
    }

    let mut aggregate = AggregateRow::zero();
    for row in source_rows {
        let reward_cut_raw = reward_cut_raw_at_block(
            pg,
            reward_cut_cache,
            &row.orchestrator_address,
            row.block_number,
            row.log_index,
        )
        .await?;
        let orch_tokens = share_from_raw_percent(&row.total_tokens_native, &reward_cut_raw);
        let delegators_tokens = &row.total_tokens_native - &orch_tokens;
        let orch_tokens_usd = row
            .total_tokens_usd
            .as_ref()
            .map(|usd| share_from_raw_percent(usd, &reward_cut_raw));
        let delegators_tokens_usd = row
            .total_tokens_usd
            .as_ref()
            .zip(orch_tokens_usd.as_ref())
            .map(|(usd, orch)| usd - orch);

        aggregate.reward_event_count += 1;
        aggregate.sum_total_tokens += row.total_tokens_native.clone();
        aggregate.sum_orch_tokens += orch_tokens;
        aggregate.sum_delegators_tokens += delegators_tokens;
        if let Some(total_usd) = row.total_tokens_usd.as_ref() {
            aggregate.sum_total_tokens_usd += total_usd.clone();
            aggregate.sum_orch_tokens_usd += orch_tokens_usd.expect("orch usd set");
            aggregate.sum_delegators_tokens_usd +=
                delegators_tokens_usd.expect("delegators usd set");
            aggregate.usd_rows_priced += 1;
        }
        aggregate.source_max_event_id = aggregate.source_max_event_id.max(row.event_id);
    }
    Ok(Some(aggregate))
}

fn compare_event_position(
    left_block: i64,
    left_log_index: i32,
    right_block: i64,
    right_log_index: i32,
) -> Ordering {
    left_block
        .cmp(&right_block)
        .then_with(|| left_log_index.cmp(&right_log_index))
}

fn share_from_raw_percent(amount: &BigDecimal, raw_percent: &BigDecimal) -> BigDecimal {
    amount.clone() * raw_percent.clone() / BigDecimal::from(REWARD_CUT_DENOMINATOR)
}

async fn load_existing_candidate_keys(
    pg: &PgPool,
    orchestrator_address: &str,
    event_id: i64,
) -> Result<Vec<AggregateKey>> {
    let rows = sqlx::query(
        r#"SELECT day_utc, orchestrator_address, valuation_version
             FROM orch_rewards_daily
            WHERE chain_id = $1
              AND orchestrator_address = $2
              AND source_max_event_id >= $3"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(orchestrator_address)
    .bind(event_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| AggregateKey {
            day_utc: row.get("day_utc"),
            orchestrator_address: row.get("orchestrator_address"),
            valuation_version: row.get("valuation_version"),
        })
        .collect())
}

async fn load_current_event_keys(
    pg: &PgPool,
    include_tentative: bool,
    event_id: i64,
) -> Result<Vec<AggregateKey>> {
    let finality_filter = if include_tentative {
        ""
    } else {
        "AND e.finality = 'finalized'"
    };
    let sql = format!(
        r#"SELECT DATE(e.block_timestamp) AS day_utc,
                  e.to_address AS orchestrator_address,
                  v.valuation_version
             FROM raw_protocol_events e
             JOIN event_valuations v
               ON v.event_id = e.id
              AND v.asset = 'LPT'
            WHERE e.id = $1
              AND e.chain_id = $2
              AND e.event_name = 'Reward'
              AND e.is_canonical = TRUE
              {finality_filter}
              AND e.to_address IS NOT NULL"#,
    );
    let rows = sqlx::query(&sql)
        .bind(event_id)
        .bind(ARBITRUM_CHAIN_ID)
        .fetch_all(pg)
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| AggregateKey {
            day_utc: row.get("day_utc"),
            orchestrator_address: row.get("orchestrator_address"),
            valuation_version: row.get("valuation_version"),
        })
        .collect())
}

async fn fetch_reward_rows_for_key(
    pg: &PgPool,
    include_tentative: bool,
    key: &AggregateKey,
) -> Result<Vec<RewardEventRow>> {
    let finality_filter = if include_tentative {
        ""
    } else {
        "AND e.finality = 'finalized'"
    };
    let sql = format!(
        r#"SELECT
               e.id AS event_id,
               e.block_number,
               e.log_index,
               e.block_timestamp,
               e.to_address AS orchestrator_address,
               v.valuation_version,
               v.amount_native AS total_tokens_native,
               v.amount_usd AS total_tokens_usd
           FROM raw_protocol_events e
           JOIN event_valuations v
             ON v.event_id = e.id
            AND v.asset = 'LPT'
          WHERE e.chain_id = $1
            AND e.event_name = 'Reward'
            AND e.is_canonical = TRUE
            {finality_filter}
            AND e.to_address = $2
            AND DATE(e.block_timestamp) = $3
            AND v.valuation_version = $4
       ORDER BY e.id ASC"#,
    );
    let rows = sqlx::query(&sql)
        .bind(ARBITRUM_CHAIN_ID)
        .bind(&key.orchestrator_address)
        .bind(key.day_utc)
        .bind(&key.valuation_version)
        .fetch_all(pg)
        .await?;

    rows.into_iter()
        .map(|row| {
            Ok(RewardEventRow {
                event_id: row.get("event_id"),
                block_number: row.get("block_number"),
                log_index: row.get("log_index"),
                block_timestamp: row.get("block_timestamp"),
                orchestrator_address: row.get("orchestrator_address"),
                valuation_version: row.get("valuation_version"),
                total_tokens_native: row.get("total_tokens_native"),
                total_tokens_usd: row.get("total_tokens_usd"),
            })
        })
        .collect()
}

async fn upsert_aggregate(pg: &PgPool, key: &AggregateKey, row: &AggregateRow) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO orch_rewards_daily (
               chain_id,
               day_utc,
               orchestrator_address,
               valuation_version,
               reward_event_count,
               sum_total_tokens,
               sum_total_tokens_usd,
               sum_orch_tokens,
               sum_orch_tokens_usd,
               sum_delegators_tokens,
               sum_delegators_tokens_usd,
               usd_rows_priced,
               source_max_event_id,
               updated_at
           ) VALUES (
               $1, $2, $3, $4,
               $5, $6, $7, $8, $9,
               $10, $11, $12, $13, now()
           )
           ON CONFLICT (
               chain_id,
               day_utc,
               orchestrator_address,
               valuation_version
           ) DO UPDATE
               SET reward_event_count = orch_rewards_daily.reward_event_count + EXCLUDED.reward_event_count,
                   sum_total_tokens = orch_rewards_daily.sum_total_tokens + EXCLUDED.sum_total_tokens,
                   sum_total_tokens_usd = orch_rewards_daily.sum_total_tokens_usd + EXCLUDED.sum_total_tokens_usd,
                   sum_orch_tokens = orch_rewards_daily.sum_orch_tokens + EXCLUDED.sum_orch_tokens,
                   sum_orch_tokens_usd = orch_rewards_daily.sum_orch_tokens_usd + EXCLUDED.sum_orch_tokens_usd,
                   sum_delegators_tokens = orch_rewards_daily.sum_delegators_tokens + EXCLUDED.sum_delegators_tokens,
                   sum_delegators_tokens_usd = orch_rewards_daily.sum_delegators_tokens_usd + EXCLUDED.sum_delegators_tokens_usd,
                   usd_rows_priced = orch_rewards_daily.usd_rows_priced + EXCLUDED.usd_rows_priced,
                   source_max_event_id = GREATEST(orch_rewards_daily.source_max_event_id, EXCLUDED.source_max_event_id),
                   updated_at = now()"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(key.day_utc)
    .bind(&key.orchestrator_address)
    .bind(&key.valuation_version)
    .bind(row.reward_event_count)
    .bind(&row.sum_total_tokens)
    .bind(&row.sum_total_tokens_usd)
    .bind(&row.sum_orch_tokens)
    .bind(&row.sum_orch_tokens_usd)
    .bind(&row.sum_delegators_tokens)
    .bind(&row.sum_delegators_tokens_usd)
    .bind(row.usd_rows_priced)
    .bind(row.source_max_event_id)
    .execute(pg)
    .await?;
    Ok(())
}

async fn replace_aggregate(pg: &PgPool, key: &AggregateKey, row: &AggregateRow) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO orch_rewards_daily (
               chain_id,
               day_utc,
               orchestrator_address,
               valuation_version,
               reward_event_count,
               sum_total_tokens,
               sum_total_tokens_usd,
               sum_orch_tokens,
               sum_orch_tokens_usd,
               sum_delegators_tokens,
               sum_delegators_tokens_usd,
               usd_rows_priced,
               source_max_event_id,
               updated_at
           ) VALUES (
               $1, $2, $3, $4,
               $5, $6, $7, $8, $9,
               $10, $11, $12, $13, now()
           )
           ON CONFLICT (
               chain_id,
               day_utc,
               orchestrator_address,
               valuation_version
           ) DO UPDATE
               SET reward_event_count = EXCLUDED.reward_event_count,
                   sum_total_tokens = EXCLUDED.sum_total_tokens,
                   sum_total_tokens_usd = EXCLUDED.sum_total_tokens_usd,
                   sum_orch_tokens = EXCLUDED.sum_orch_tokens,
                   sum_orch_tokens_usd = EXCLUDED.sum_orch_tokens_usd,
                   sum_delegators_tokens = EXCLUDED.sum_delegators_tokens,
                   sum_delegators_tokens_usd = EXCLUDED.sum_delegators_tokens_usd,
                   usd_rows_priced = EXCLUDED.usd_rows_priced,
                   source_max_event_id = EXCLUDED.source_max_event_id,
                   updated_at = now()"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(key.day_utc)
    .bind(&key.orchestrator_address)
    .bind(&key.valuation_version)
    .bind(row.reward_event_count)
    .bind(&row.sum_total_tokens)
    .bind(&row.sum_total_tokens_usd)
    .bind(&row.sum_orch_tokens)
    .bind(&row.sum_orch_tokens_usd)
    .bind(&row.sum_delegators_tokens)
    .bind(&row.sum_delegators_tokens_usd)
    .bind(row.usd_rows_priced)
    .bind(row.source_max_event_id)
    .execute(pg)
    .await?;
    Ok(())
}

async fn delete_aggregate(pg: &PgPool, key: &AggregateKey) -> Result<()> {
    sqlx::query(
        r#"DELETE FROM orch_rewards_daily
            WHERE chain_id = $1
              AND day_utc = $2
              AND orchestrator_address = $3
              AND valuation_version = $4"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(key.day_utc)
    .bind(&key.orchestrator_address)
    .bind(&key.valuation_version)
    .execute(pg)
    .await?;
    Ok(())
}

async fn load_checkpoint(pg: &PgPool) -> Result<Option<i64>> {
    load_named_checkpoint(pg, CHECKPOINT_NAME).await
}

async fn load_named_checkpoint(pg: &PgPool, name: &str) -> Result<Option<i64>> {
    let event_id = sqlx::query_scalar::<_, i64>(
        "SELECT last_processed_block FROM indexer_checkpoints WHERE name = $1",
    )
    .bind(name)
    .fetch_optional(pg)
    .await?;
    Ok(event_id)
}

async fn advance_checkpoint(pg: &PgPool, event_id: i64) -> Result<()> {
    advance_named_checkpoint(pg, CHECKPOINT_NAME, event_id).await
}

async fn advance_named_checkpoint(pg: &PgPool, name: &str, event_id: i64) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO indexer_checkpoints (name, chain_id, last_processed_block, updated_at)
           VALUES ($1, $2, $3, now())
           ON CONFLICT (name) DO UPDATE
              SET last_processed_block = GREATEST(indexer_checkpoints.last_processed_block, EXCLUDED.last_processed_block),
                  updated_at = now()"#,
    )
    .bind(name)
    .bind(ARBITRUM_CHAIN_ID)
    .bind(event_id)
    .execute(pg)
    .await?;
    Ok(())
}
