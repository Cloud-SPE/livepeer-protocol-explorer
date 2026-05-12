//! TD-018 Phase 1 — daily event-metrics rollup writer.
//!
//! Materializes `event_metrics_daily` from canonical, finalized, is_valuable
//! `raw_protocol_events` rows joined to their `event_valuations`. One row
//! per (chain_id, day_utc, contract_name, event_name, asset, valuation_version).
//!
//! Determinism: this writer never reads anything outside the DB. Every row
//! is reproducible from `raw_protocol_events` + `event_valuations`. The
//! `last_event_id` column powers the monotonic-upsert guard
//! (TD-017 acceptance criterion #3) so out-of-order replays are safe.
//!
//! Shape mirrors `orch_rewards.rs` for consistency: bounded batch fetch,
//! in-memory aggregation, idempotent upsert, reorg-mutation handler that
//! rebuilds affected (day, key) cells fully.

use anyhow::Result;
use bigdecimal::BigDecimal;
use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use sqlx::{PgPool, Row};
use std::collections::{HashMap, HashSet};
use tracing::info;

const ARBITRUM_CHAIN_ID: i64 = 42161;
const CHECKPOINT_NAME: &str = "rollup_event_metrics_daily";
const REORG_CHECKPOINT_NAME: &str = "rollup_event_metrics_daily_reorg";

#[derive(Debug, Default, Serialize)]
pub struct EventMetricsSummary {
    pub events_seen: u64,
    pub rows_written: u64,
    pub groups_touched: u64,
    pub checkpoint_event_id: Option<i64>,
}

#[derive(Debug, Clone)]
struct EventValuationRow {
    event_id: i64,
    block_timestamp: DateTime<Utc>,
    contract_name: String,
    event_name: String,
    asset: String,
    valuation_version: String,
    amount_native: Option<BigDecimal>,
    amount_usd: Option<BigDecimal>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct AggregateKey {
    day_utc: NaiveDate,
    contract_name: String,
    event_name: String,
    asset: String,
    valuation_version: String,
}

#[derive(Debug, Clone)]
struct AggregateRow {
    event_count: i64,
    sum_amount_native: BigDecimal,
    sum_amount_usd: BigDecimal,
    usd_rows_priced: i64,
    last_event_id: i64,
}

impl AggregateRow {
    fn zero() -> Self {
        Self {
            event_count: 0,
            sum_amount_native: BigDecimal::from(0),
            sum_amount_usd: BigDecimal::from(0),
            usd_rows_priced: 0,
            last_event_id: 0,
        }
    }
}

pub async fn run_once(
    pg: &PgPool,
    include_tentative: bool,
    batch_limit: i64,
) -> Result<EventMetricsSummary> {
    let reorg_rows_written = process_reorg_mutations(pg, include_tentative).await?;
    let checkpoint = load_checkpoint(pg).await?;
    let rows = fetch_valuation_rows(pg, include_tentative, checkpoint, batch_limit).await?;
    if rows.is_empty() {
        // TD-023: tick updated_at on empty polls (heartbeat); GREATEST
        // upsert prevents block regression.
        advance_checkpoint(pg, checkpoint.unwrap_or(0)).await?;
        return Ok(EventMetricsSummary {
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
            contract_name: row.contract_name.clone(),
            event_name: row.event_name.clone(),
            asset: row.asset.clone(),
            valuation_version: row.valuation_version.clone(),
        };
        let agg = aggregates.entry(key).or_insert_with(AggregateRow::zero);
        agg.event_count += 1;
        if let Some(native) = row.amount_native.as_ref() {
            agg.sum_amount_native += native.clone();
        }
        if let Some(usd) = row.amount_usd.as_ref() {
            agg.sum_amount_usd += usd.clone();
            agg.usd_rows_priced += 1;
        }
        agg.last_event_id = agg.last_event_id.max(row.event_id);
        max_event_id = max_event_id.max(row.event_id);
    }

    let groups_touched = aggregates.len() as u64;
    let mut rows_written = reorg_rows_written;
    for (key, agg) in aggregates {
        upsert_aggregate(pg, &key, &agg).await?;
        rows_written += 1;
    }
    advance_checkpoint(pg, max_event_id).await?;

    let summary = EventMetricsSummary {
        events_seen: rows.len() as u64,
        rows_written,
        groups_touched,
        checkpoint_event_id: Some(max_event_id),
    };
    info!(?summary, "event metrics rollup complete");
    Ok(summary)
}

async fn process_reorg_mutations(pg: &PgPool, include_tentative: bool) -> Result<u64> {
    let checkpoint = load_named_checkpoint(pg, REORG_CHECKPOINT_NAME).await?;
    let rows = sqlx::query(
        r#"SELECT m.id AS mutation_id,
                  m.raw_event_id
             FROM reorg_mutations m
             JOIN raw_protocol_events e
               ON e.id = m.raw_event_id
            WHERE m.id > COALESCE($1, 0)
              AND e.chain_id = $2
              AND e.is_valuable = TRUE
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
        max_mutation_id = max_mutation_id.max(mutation_id);

        for key in load_existing_candidate_keys(pg, event_id).await? {
            keys.insert(key);
        }
        for key in load_current_event_keys(pg, include_tentative, event_id).await? {
            keys.insert(key);
        }
    }

    let mut rows_written = 0_u64;
    for key in keys {
        if let Some(aggregate) = rebuild_aggregate_for_key(pg, include_tentative, &key).await? {
            replace_aggregate(pg, &key, &aggregate).await?;
            rows_written += 1;
        } else {
            delete_aggregate(pg, &key).await?;
        }
    }
    advance_named_checkpoint(pg, REORG_CHECKPOINT_NAME, max_mutation_id).await?;
    Ok(rows_written)
}

async fn fetch_valuation_rows(
    pg: &PgPool,
    include_tentative: bool,
    resume_from_event_id: Option<i64>,
    limit: i64,
) -> Result<Vec<EventValuationRow>> {
    let finality_filter = if include_tentative {
        ""
    } else {
        "AND e.finality = 'finalized'"
    };
    let sql = format!(
        r#"SELECT
               e.id AS event_id,
               e.block_timestamp,
               e.contract_name,
               e.event_name,
               e.asset,
               v.valuation_version,
               v.amount_native,
               v.amount_usd
           FROM raw_protocol_events e
           JOIN event_valuations v
             ON v.event_id = e.id
            AND v.asset IS NOT DISTINCT FROM e.asset
          WHERE e.chain_id = $1
            AND e.is_valuable = TRUE
            AND e.is_canonical = TRUE
            {finality_filter}
            AND e.asset IS NOT NULL
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
            Ok(EventValuationRow {
                event_id: row.get("event_id"),
                block_timestamp: row.get("block_timestamp"),
                contract_name: row.get("contract_name"),
                event_name: row.get("event_name"),
                asset: row.get("asset"),
                valuation_version: row.get("valuation_version"),
                amount_native: row.try_get("amount_native").ok(),
                amount_usd: row.try_get("amount_usd").ok(),
            })
        })
        .collect()
}

async fn rebuild_aggregate_for_key(
    pg: &PgPool,
    include_tentative: bool,
    key: &AggregateKey,
) -> Result<Option<AggregateRow>> {
    let source_rows = fetch_valuation_rows_for_key(pg, include_tentative, key).await?;
    if source_rows.is_empty() {
        return Ok(None);
    }

    let mut aggregate = AggregateRow::zero();
    for row in source_rows {
        aggregate.event_count += 1;
        if let Some(native) = row.amount_native.as_ref() {
            aggregate.sum_amount_native += native.clone();
        }
        if let Some(usd) = row.amount_usd.as_ref() {
            aggregate.sum_amount_usd += usd.clone();
            aggregate.usd_rows_priced += 1;
        }
        aggregate.last_event_id = aggregate.last_event_id.max(row.event_id);
    }
    Ok(Some(aggregate))
}

async fn fetch_valuation_rows_for_key(
    pg: &PgPool,
    include_tentative: bool,
    key: &AggregateKey,
) -> Result<Vec<EventValuationRow>> {
    let finality_filter = if include_tentative {
        ""
    } else {
        "AND e.finality = 'finalized'"
    };
    let sql = format!(
        r#"SELECT
               e.id AS event_id,
               e.block_timestamp,
               e.contract_name,
               e.event_name,
               e.asset,
               v.valuation_version,
               v.amount_native,
               v.amount_usd
           FROM raw_protocol_events e
           JOIN event_valuations v
             ON v.event_id = e.id
            AND v.asset IS NOT DISTINCT FROM e.asset
          WHERE e.chain_id = $1
            AND e.is_valuable = TRUE
            AND e.is_canonical = TRUE
            {finality_filter}
            AND e.contract_name = $2
            AND e.event_name = $3
            AND e.asset = $4
            AND v.valuation_version = $5
            AND DATE(e.block_timestamp) = $6
       ORDER BY e.id ASC"#,
    );
    let rows = sqlx::query(&sql)
        .bind(ARBITRUM_CHAIN_ID)
        .bind(&key.contract_name)
        .bind(&key.event_name)
        .bind(&key.asset)
        .bind(&key.valuation_version)
        .bind(key.day_utc)
        .fetch_all(pg)
        .await?;

    rows.into_iter()
        .map(|row| {
            Ok(EventValuationRow {
                event_id: row.get("event_id"),
                block_timestamp: row.get("block_timestamp"),
                contract_name: row.get("contract_name"),
                event_name: row.get("event_name"),
                asset: row.get("asset"),
                valuation_version: row.get("valuation_version"),
                amount_native: row.try_get("amount_native").ok(),
                amount_usd: row.try_get("amount_usd").ok(),
            })
        })
        .collect()
}

async fn load_existing_candidate_keys(pg: &PgPool, event_id: i64) -> Result<Vec<AggregateKey>> {
    let rows = sqlx::query(
        r#"SELECT day_utc, contract_name, event_name, asset, valuation_version
             FROM event_metrics_daily
            WHERE chain_id = $1
              AND last_event_id >= $2"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(event_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| AggregateKey {
            day_utc: row.get("day_utc"),
            contract_name: row.get("contract_name"),
            event_name: row.get("event_name"),
            asset: row.get("asset"),
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
                  e.contract_name,
                  e.event_name,
                  e.asset,
                  v.valuation_version
             FROM raw_protocol_events e
             JOIN event_valuations v
               ON v.event_id = e.id
              AND v.asset IS NOT DISTINCT FROM e.asset
            WHERE e.id = $1
              AND e.chain_id = $2
              AND e.is_valuable = TRUE
              AND e.is_canonical = TRUE
              {finality_filter}
              AND e.asset IS NOT NULL"#,
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
            contract_name: row.get("contract_name"),
            event_name: row.get("event_name"),
            asset: row.get("asset"),
            valuation_version: row.get("valuation_version"),
        })
        .collect())
}

async fn upsert_aggregate(pg: &PgPool, key: &AggregateKey, row: &AggregateRow) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO event_metrics_daily (
               chain_id,
               day_utc,
               contract_name,
               event_name,
               asset,
               valuation_version,
               event_count,
               sum_amount_native,
               sum_amount_usd,
               usd_rows_priced,
               last_event_id,
               updated_at
           ) VALUES (
               $1, $2, $3, $4, $5, $6,
               $7, $8, $9, $10, $11, now()
           )
           ON CONFLICT (
               chain_id,
               day_utc,
               contract_name,
               event_name,
               asset,
               valuation_version
           ) DO UPDATE
               SET event_count = event_metrics_daily.event_count + EXCLUDED.event_count,
                   sum_amount_native = COALESCE(event_metrics_daily.sum_amount_native, 0)
                                       + COALESCE(EXCLUDED.sum_amount_native, 0),
                   sum_amount_usd = COALESCE(event_metrics_daily.sum_amount_usd, 0)
                                    + COALESCE(EXCLUDED.sum_amount_usd, 0),
                   usd_rows_priced = event_metrics_daily.usd_rows_priced + EXCLUDED.usd_rows_priced,
                   last_event_id = GREATEST(event_metrics_daily.last_event_id, EXCLUDED.last_event_id),
                   updated_at = now()"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(key.day_utc)
    .bind(&key.contract_name)
    .bind(&key.event_name)
    .bind(&key.asset)
    .bind(&key.valuation_version)
    .bind(row.event_count)
    .bind(&row.sum_amount_native)
    .bind(&row.sum_amount_usd)
    .bind(row.usd_rows_priced)
    .bind(row.last_event_id)
    .execute(pg)
    .await?;
    Ok(())
}

async fn replace_aggregate(pg: &PgPool, key: &AggregateKey, row: &AggregateRow) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO event_metrics_daily (
               chain_id,
               day_utc,
               contract_name,
               event_name,
               asset,
               valuation_version,
               event_count,
               sum_amount_native,
               sum_amount_usd,
               usd_rows_priced,
               last_event_id,
               updated_at
           ) VALUES (
               $1, $2, $3, $4, $5, $6,
               $7, $8, $9, $10, $11, now()
           )
           ON CONFLICT (
               chain_id,
               day_utc,
               contract_name,
               event_name,
               asset,
               valuation_version
           ) DO UPDATE
               SET event_count = EXCLUDED.event_count,
                   sum_amount_native = EXCLUDED.sum_amount_native,
                   sum_amount_usd = EXCLUDED.sum_amount_usd,
                   usd_rows_priced = EXCLUDED.usd_rows_priced,
                   last_event_id = EXCLUDED.last_event_id,
                   updated_at = now()"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(key.day_utc)
    .bind(&key.contract_name)
    .bind(&key.event_name)
    .bind(&key.asset)
    .bind(&key.valuation_version)
    .bind(row.event_count)
    .bind(&row.sum_amount_native)
    .bind(&row.sum_amount_usd)
    .bind(row.usd_rows_priced)
    .bind(row.last_event_id)
    .execute(pg)
    .await?;
    Ok(())
}

async fn delete_aggregate(pg: &PgPool, key: &AggregateKey) -> Result<()> {
    sqlx::query(
        r#"DELETE FROM event_metrics_daily
            WHERE chain_id = $1
              AND day_utc = $2
              AND contract_name = $3
              AND event_name = $4
              AND asset = $5
              AND valuation_version = $6"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(key.day_utc)
    .bind(&key.contract_name)
    .bind(&key.event_name)
    .bind(&key.asset)
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
