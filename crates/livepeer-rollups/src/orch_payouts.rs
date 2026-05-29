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
const CHECKPOINT_NAME: &str = "rollup_orch_payouts_daily";
const REORG_CHECKPOINT_NAME: &str = "rollup_orch_payouts_daily_reorg";
const FEE_SHARE_DENOMINATOR: i64 = 1_000_000;
const ZERO_RAW_FEE_SHARE: &str = "0";

#[derive(Debug, Default, Serialize)]
pub struct OrchPayoutsSummary {
    pub events_seen: u64,
    pub rows_written: u64,
    pub groups_touched: u64,
    pub checkpoint_event_id: Option<i64>,
}

#[derive(Debug, Clone)]
struct TicketEventRow {
    event_id: i64,
    block_number: i64,
    log_index: i32,
    block_timestamp: DateTime<Utc>,
    gateway_address: String,
    orchestrator_address: String,
    valuation_version: String,
    broadcaster_kind: String,
    face_value_native: BigDecimal,
    // NULL for terminal-failure valuations (migration 017): events whose ETH
    // USD price was unavailable (failed_missing_oracle / pool / sequencer).
    // amount_native stays NOT NULL, so only the USD side is optional.
    face_value_usd: Option<BigDecimal>,
}

#[derive(Debug, Clone)]
struct FeeSharePoint {
    block_number: i64,
    log_index: i32,
    fee_share_raw: BigDecimal,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct AggregateKey {
    day_utc: NaiveDate,
    orchestrator_address: String,
    valuation_version: String,
    broadcaster_kind: String,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct GatewaySeenKey {
    aggregate: AggregateKey,
    gateway_address: String,
}

#[derive(Debug, Clone)]
struct AggregateRow {
    ticket_count: i64,
    sum_face_value_native: BigDecimal,
    sum_face_value_usd: BigDecimal,
    sum_commission_native: BigDecimal,
    sum_commission_usd: BigDecimal,
    sum_delegators_share_native: BigDecimal,
    sum_delegators_share_usd: BigDecimal,
    distinct_gateways: i32,
    usd_rows_priced: i64,
    source_max_event_id: i64,
}

impl AggregateRow {
    fn zero() -> Self {
        Self {
            ticket_count: 0,
            sum_face_value_native: BigDecimal::from(0),
            sum_face_value_usd: BigDecimal::from(0),
            sum_commission_native: BigDecimal::from(0),
            sum_commission_usd: BigDecimal::from(0),
            sum_delegators_share_native: BigDecimal::from(0),
            sum_delegators_share_usd: BigDecimal::from(0),
            distinct_gateways: 0,
            usd_rows_priced: 0,
            source_max_event_id: 0,
        }
    }
}

pub async fn run_once(
    pg: &PgPool,
    include_tentative: bool,
    batch_limit: i64,
) -> Result<OrchPayoutsSummary> {
    let mut fee_share_cache: HashMap<String, Vec<FeeSharePoint>> = HashMap::new();
    let reorg_rows_written =
        process_reorg_mutations(pg, include_tentative, &mut fee_share_cache).await?;
    let checkpoint = load_checkpoint(pg).await?;
    let rows = fetch_ticket_rows(pg, include_tentative, checkpoint, batch_limit).await?;
    if rows.is_empty() {
        // TD-023: tick updated_at on empty polls (heartbeat); GREATEST
        // upsert prevents block regression.
        advance_checkpoint(pg, checkpoint.unwrap_or(0)).await?;
        return Ok(OrchPayoutsSummary {
            rows_written: reorg_rows_written,
            checkpoint_event_id: checkpoint,
            ..Default::default()
        });
    }

    let mut batch_seen_gateways = HashSet::new();
    let mut aggregates: HashMap<AggregateKey, AggregateRow> = HashMap::new();
    let mut max_event_id = checkpoint.unwrap_or(0);

    for row in &rows {
        let key = AggregateKey {
            day_utc: row.block_timestamp.date_naive(),
            orchestrator_address: row.orchestrator_address.clone(),
            valuation_version: row.valuation_version.clone(),
            broadcaster_kind: row.broadcaster_kind.clone(),
        };
        let fee_share_raw = fee_share_raw_at_block(
            pg,
            &mut fee_share_cache,
            &row.orchestrator_address,
            row.block_number,
            row.log_index,
        )
        .await?;
        let commission_native = commission_from_fee_share(&row.face_value_native, &fee_share_raw);
        let delegators_native = &row.face_value_native - &commission_native;
        let gateway_seen_key = GatewaySeenKey {
            aggregate: key.clone(),
            gateway_address: row.gateway_address.clone(),
        };
        let distinct_increment = if batch_seen_gateways.contains(&gateway_seen_key) {
            0
        } else if prior_gateway_seen(pg, row, &key).await? {
            batch_seen_gateways.insert(gateway_seen_key);
            0
        } else {
            batch_seen_gateways.insert(gateway_seen_key);
            1
        };

        let agg = aggregates.entry(key).or_insert_with(AggregateRow::zero);
        agg.ticket_count += 1;
        agg.sum_face_value_native += row.face_value_native.clone();
        agg.sum_commission_native += commission_native;
        agg.sum_delegators_share_native += delegators_native;
        // USD side only for priced rows; terminal-failure valuations
        // (NULL amount_usd) still count toward ticket_count + native sums.
        if let Some(face_value_usd) = row.face_value_usd.as_ref() {
            let commission_usd = commission_from_fee_share(face_value_usd, &fee_share_raw);
            let delegators_usd = face_value_usd - &commission_usd;
            agg.sum_face_value_usd += face_value_usd.clone();
            agg.sum_commission_usd += commission_usd;
            agg.sum_delegators_share_usd += delegators_usd;
            agg.usd_rows_priced += 1;
        }
        agg.distinct_gateways += distinct_increment;
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

    let summary = OrchPayoutsSummary {
        events_seen: rows.len() as u64,
        rows_written,
        groups_touched,
        checkpoint_event_id: Some(max_event_id),
    };
    info!(?summary, "orch payouts rollup complete");
    Ok(summary)
}

async fn process_reorg_mutations(
    pg: &PgPool,
    include_tentative: bool,
    fee_share_cache: &mut HashMap<String, Vec<FeeSharePoint>>,
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
              AND e.event_name = 'WinningTicketRedeemed'
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
            rebuild_aggregate_for_key(pg, include_tentative, fee_share_cache, &key).await?
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

async fn fetch_ticket_rows(
    pg: &PgPool,
    include_tentative: bool,
    resume_from_event_id: Option<i64>,
    limit: i64,
) -> Result<Vec<TicketEventRow>> {
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
               e.from_address AS gateway_address,
               e.to_address AS orchestrator_address,
               v.valuation_version,
               COALESCE(bc.kind, 'transcoding') AS broadcaster_kind,
               v.amount_native AS face_value_native,
               v.amount_usd AS face_value_usd
           FROM raw_protocol_events e
           JOIN event_valuations v
             ON v.event_id = e.id
            AND v.asset = 'ETH'
           LEFT JOIN broadcaster_classifications bc
             ON bc.chain_id = e.chain_id
            AND bc.address = e.from_address
          WHERE e.chain_id = $1
            AND e.event_name = 'WinningTicketRedeemed'
            AND e.is_canonical = TRUE
            {finality_filter}
            AND e.from_address IS NOT NULL
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
            Ok(TicketEventRow {
                event_id: row.get("event_id"),
                block_number: row.get("block_number"),
                log_index: row.get("log_index"),
                block_timestamp: row.get("block_timestamp"),
                gateway_address: row.get("gateway_address"),
                orchestrator_address: row.get("orchestrator_address"),
                valuation_version: row.get("valuation_version"),
                broadcaster_kind: row.get("broadcaster_kind"),
                face_value_native: row.get("face_value_native"),
                face_value_usd: row.try_get("face_value_usd").ok(),
            })
        })
        .collect()
}

async fn fee_share_raw_at_block(
    pg: &PgPool,
    cache: &mut HashMap<String, Vec<FeeSharePoint>>,
    orchestrator: &str,
    block_number: i64,
    log_index: i32,
) -> Result<BigDecimal> {
    if !cache.contains_key(orchestrator) {
        let rows = sqlx::query(
            r#"SELECT
                   block_number,
                   log_index,
                   COALESCE(raw_event -> 'decoded' ->> 'feeShare', $3) AS fee_share_raw
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
        .bind(ZERO_RAW_FEE_SHARE)
        .fetch_all(pg)
        .await?;

        let points = rows
            .into_iter()
            .map(|row| {
                Ok(FeeSharePoint {
                    block_number: row.get("block_number"),
                    log_index: row.get("log_index"),
                    fee_share_raw: BigDecimal::from_str(&row.get::<String, _>("fee_share_raw"))
                        .context("parsing feeShare raw")?,
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
        selected = point.fee_share_raw.clone();
    }
    Ok(selected)
}

async fn rebuild_aggregate_for_key(
    pg: &PgPool,
    include_tentative: bool,
    fee_share_cache: &mut HashMap<String, Vec<FeeSharePoint>>,
    key: &AggregateKey,
) -> Result<Option<AggregateRow>> {
    let source_rows = fetch_ticket_rows_for_key(pg, include_tentative, key).await?;
    if source_rows.is_empty() {
        return Ok(None);
    }

    let mut seen_gateways = HashSet::new();
    let mut aggregate = AggregateRow::zero();
    for row in source_rows {
        let fee_share_raw = fee_share_raw_at_block(
            pg,
            fee_share_cache,
            &row.orchestrator_address,
            row.block_number,
            row.log_index,
        )
        .await?;
        let commission_native = commission_from_fee_share(&row.face_value_native, &fee_share_raw);
        let delegators_native = &row.face_value_native - &commission_native;

        aggregate.ticket_count += 1;
        aggregate.sum_face_value_native += row.face_value_native.clone();
        aggregate.sum_commission_native += commission_native;
        aggregate.sum_delegators_share_native += delegators_native;
        // USD side only for priced rows; terminal-failure valuations
        // (NULL amount_usd) still count toward ticket_count + native sums.
        if let Some(face_value_usd) = row.face_value_usd.as_ref() {
            let commission_usd = commission_from_fee_share(face_value_usd, &fee_share_raw);
            let delegators_usd = face_value_usd - &commission_usd;
            aggregate.sum_face_value_usd += face_value_usd.clone();
            aggregate.sum_commission_usd += commission_usd;
            aggregate.sum_delegators_share_usd += delegators_usd;
            aggregate.usd_rows_priced += 1;
        }
        aggregate.source_max_event_id = aggregate.source_max_event_id.max(row.event_id);
        if seen_gateways.insert(row.gateway_address) {
            aggregate.distinct_gateways += 1;
        }
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

fn commission_from_fee_share(amount: &BigDecimal, fee_share_raw: &BigDecimal) -> BigDecimal {
    let denominator = BigDecimal::from(FEE_SHARE_DENOMINATOR);
    let fee_cut_fraction = (denominator.clone() - fee_share_raw.clone()) / denominator;
    amount.clone() * fee_cut_fraction
}

async fn prior_gateway_seen(pg: &PgPool, row: &TicketEventRow, key: &AggregateKey) -> Result<bool> {
    let exists = sqlx::query_scalar::<_, i32>(
        r#"SELECT 1
             FROM raw_protocol_events e
             JOIN event_valuations v
               ON v.event_id = e.id
              AND v.asset = 'ETH'
            WHERE e.chain_id = $1
              AND e.event_name = 'WinningTicketRedeemed'
              AND e.is_canonical = TRUE
              AND e.id < $2
              AND e.from_address = $3
              AND e.to_address = $4
              AND e.block_timestamp >= $5::date
              AND e.block_timestamp < ($5::date + INTERVAL '1 day')
              AND v.valuation_version = $6
            LIMIT 1"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(row.event_id)
    .bind(&row.gateway_address)
    .bind(&key.orchestrator_address)
    .bind(key.day_utc)
    .bind(&key.valuation_version)
    .fetch_optional(pg)
    .await?;
    Ok(exists.is_some())
}

async fn load_existing_candidate_keys(
    pg: &PgPool,
    orchestrator_address: &str,
    event_id: i64,
) -> Result<Vec<AggregateKey>> {
    let rows = sqlx::query(
        r#"SELECT day_utc, orchestrator_address, valuation_version, broadcaster_kind
             FROM orch_payouts_daily
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
            broadcaster_kind: row.get("broadcaster_kind"),
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
        r#"SELECT
               DATE(e.block_timestamp) AS day_utc,
               e.to_address AS orchestrator_address,
               v.valuation_version,
               COALESCE(bc.kind, 'transcoding') AS broadcaster_kind
           FROM raw_protocol_events e
           JOIN event_valuations v
             ON v.event_id = e.id
            AND v.asset = 'ETH'
      LEFT JOIN broadcaster_classifications bc
             ON bc.chain_id = e.chain_id
            AND bc.address = e.from_address
          WHERE e.id = $1
            AND e.chain_id = $2
            AND e.event_name = 'WinningTicketRedeemed'
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
            broadcaster_kind: row.get("broadcaster_kind"),
        })
        .collect())
}

async fn fetch_ticket_rows_for_key(
    pg: &PgPool,
    include_tentative: bool,
    key: &AggregateKey,
) -> Result<Vec<TicketEventRow>> {
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
               e.from_address AS gateway_address,
               e.to_address AS orchestrator_address,
               v.valuation_version,
               COALESCE(bc.kind, 'transcoding') AS broadcaster_kind,
               v.amount_native AS face_value_native,
               v.amount_usd AS face_value_usd
           FROM raw_protocol_events e
           JOIN event_valuations v
             ON v.event_id = e.id
            AND v.asset = 'ETH'
      LEFT JOIN broadcaster_classifications bc
             ON bc.chain_id = e.chain_id
            AND bc.address = e.from_address
          WHERE e.chain_id = $1
            AND e.event_name = 'WinningTicketRedeemed'
            AND e.is_canonical = TRUE
            {finality_filter}
            AND e.from_address IS NOT NULL
            AND e.to_address = $2
            AND DATE(e.block_timestamp) = $3
            AND v.valuation_version = $4
            AND COALESCE(bc.kind, 'transcoding') = $5
       ORDER BY e.id ASC"#,
    );
    let rows = sqlx::query(&sql)
        .bind(ARBITRUM_CHAIN_ID)
        .bind(&key.orchestrator_address)
        .bind(key.day_utc)
        .bind(&key.valuation_version)
        .bind(&key.broadcaster_kind)
        .fetch_all(pg)
        .await?;

    rows.into_iter()
        .map(|row| {
            Ok(TicketEventRow {
                event_id: row.get("event_id"),
                block_number: row.get("block_number"),
                log_index: row.get("log_index"),
                block_timestamp: row.get("block_timestamp"),
                gateway_address: row.get("gateway_address"),
                orchestrator_address: row.get("orchestrator_address"),
                valuation_version: row.get("valuation_version"),
                broadcaster_kind: row.get("broadcaster_kind"),
                face_value_native: row.get("face_value_native"),
                face_value_usd: row.try_get("face_value_usd").ok(),
            })
        })
        .collect()
}

async fn upsert_aggregate(pg: &PgPool, key: &AggregateKey, row: &AggregateRow) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO orch_payouts_daily (
               chain_id,
               day_utc,
               orchestrator_address,
               valuation_version,
               broadcaster_kind,
               ticket_count,
               sum_face_value_native,
               sum_face_value_usd,
               sum_commission_native,
               sum_commission_usd,
               sum_delegators_share_native,
               sum_delegators_share_usd,
               distinct_gateways,
               usd_rows_priced,
               source_max_event_id,
               updated_at
           ) VALUES (
               $1, $2, $3, $4, $5,
               $6, $7, $8, $9, $10,
               $11, $12, $13, $14, $15, now()
           )
           ON CONFLICT (
               chain_id,
               day_utc,
               orchestrator_address,
               valuation_version,
               broadcaster_kind
           ) DO UPDATE
               SET ticket_count = orch_payouts_daily.ticket_count + EXCLUDED.ticket_count,
                   sum_face_value_native = orch_payouts_daily.sum_face_value_native + EXCLUDED.sum_face_value_native,
                   sum_face_value_usd = orch_payouts_daily.sum_face_value_usd + EXCLUDED.sum_face_value_usd,
                   sum_commission_native = orch_payouts_daily.sum_commission_native + EXCLUDED.sum_commission_native,
                   sum_commission_usd = orch_payouts_daily.sum_commission_usd + EXCLUDED.sum_commission_usd,
                   sum_delegators_share_native = orch_payouts_daily.sum_delegators_share_native + EXCLUDED.sum_delegators_share_native,
                   sum_delegators_share_usd = orch_payouts_daily.sum_delegators_share_usd + EXCLUDED.sum_delegators_share_usd,
                   distinct_gateways = orch_payouts_daily.distinct_gateways + EXCLUDED.distinct_gateways,
                   usd_rows_priced = orch_payouts_daily.usd_rows_priced + EXCLUDED.usd_rows_priced,
                   source_max_event_id = GREATEST(orch_payouts_daily.source_max_event_id, EXCLUDED.source_max_event_id),
                   updated_at = now()"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(key.day_utc)
    .bind(&key.orchestrator_address)
    .bind(&key.valuation_version)
    .bind(&key.broadcaster_kind)
    .bind(row.ticket_count)
    .bind(&row.sum_face_value_native)
    .bind(&row.sum_face_value_usd)
    .bind(&row.sum_commission_native)
    .bind(&row.sum_commission_usd)
    .bind(&row.sum_delegators_share_native)
    .bind(&row.sum_delegators_share_usd)
    .bind(row.distinct_gateways)
    .bind(row.usd_rows_priced)
    .bind(row.source_max_event_id)
    .execute(pg)
    .await?;
    Ok(())
}

async fn replace_aggregate(pg: &PgPool, key: &AggregateKey, row: &AggregateRow) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO orch_payouts_daily (
               chain_id,
               day_utc,
               orchestrator_address,
               valuation_version,
               broadcaster_kind,
               ticket_count,
               sum_face_value_native,
               sum_face_value_usd,
               sum_commission_native,
               sum_commission_usd,
               sum_delegators_share_native,
               sum_delegators_share_usd,
               distinct_gateways,
               usd_rows_priced,
               source_max_event_id,
               updated_at
           ) VALUES (
               $1, $2, $3, $4, $5,
               $6, $7, $8, $9, $10,
               $11, $12, $13, $14, $15, now()
           )
           ON CONFLICT (
               chain_id,
               day_utc,
               orchestrator_address,
               valuation_version,
               broadcaster_kind
           ) DO UPDATE
               SET ticket_count = EXCLUDED.ticket_count,
                   sum_face_value_native = EXCLUDED.sum_face_value_native,
                   sum_face_value_usd = EXCLUDED.sum_face_value_usd,
                   sum_commission_native = EXCLUDED.sum_commission_native,
                   sum_commission_usd = EXCLUDED.sum_commission_usd,
                   sum_delegators_share_native = EXCLUDED.sum_delegators_share_native,
                   sum_delegators_share_usd = EXCLUDED.sum_delegators_share_usd,
                   distinct_gateways = EXCLUDED.distinct_gateways,
                   usd_rows_priced = EXCLUDED.usd_rows_priced,
                   source_max_event_id = EXCLUDED.source_max_event_id,
                   updated_at = now()"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(key.day_utc)
    .bind(&key.orchestrator_address)
    .bind(&key.valuation_version)
    .bind(&key.broadcaster_kind)
    .bind(row.ticket_count)
    .bind(&row.sum_face_value_native)
    .bind(&row.sum_face_value_usd)
    .bind(&row.sum_commission_native)
    .bind(&row.sum_commission_usd)
    .bind(&row.sum_delegators_share_native)
    .bind(&row.sum_delegators_share_usd)
    .bind(row.distinct_gateways)
    .bind(row.usd_rows_priced)
    .bind(row.source_max_event_id)
    .execute(pg)
    .await?;
    Ok(())
}

async fn delete_aggregate(pg: &PgPool, key: &AggregateKey) -> Result<()> {
    sqlx::query(
        r#"DELETE FROM orch_payouts_daily
            WHERE chain_id = $1
              AND day_utc = $2
              AND orchestrator_address = $3
              AND valuation_version = $4
              AND broadcaster_kind = $5"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(key.day_utc)
    .bind(&key.orchestrator_address)
    .bind(&key.valuation_version)
    .bind(&key.broadcaster_kind)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commission_inverts_protocol_fee_share_into_orch_keep() {
        let amount = BigDecimal::from(10);
        let fee_share_raw = BigDecimal::from(250_000);
        let commission = commission_from_fee_share(&amount, &fee_share_raw);
        assert_eq!(commission, BigDecimal::from(15) / BigDecimal::from(2));
    }

    #[test]
    fn compare_event_position_uses_block_then_log_index() {
        assert_eq!(compare_event_position(10, 1, 11, 0), Ordering::Less);
        assert_eq!(compare_event_position(10, 3, 10, 3), Ordering::Equal);
        assert_eq!(compare_event_position(10, 4, 10, 3), Ordering::Greater);
    }
}
