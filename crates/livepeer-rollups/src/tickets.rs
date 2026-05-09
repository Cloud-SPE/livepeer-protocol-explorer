use anyhow::Result;
use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use sqlx::{PgPool, Row};
use std::collections::{HashMap, HashSet};
use tracing::info;

const ARBITRUM_CHAIN_ID: i64 = 42161;
const CHECKPOINT_NAME: &str = "rollup_tickets_daily";
const REORG_CHECKPOINT_NAME: &str = "rollup_tickets_daily_reorg";

#[derive(Debug, Default, Serialize)]
pub struct TicketsSummary {
    pub events_seen: u64,
    pub rows_written: u64,
    pub groups_touched: u64,
    pub checkpoint_event_id: Option<i64>,
}

#[derive(Debug, Clone)]
struct TicketRow {
    event_id: i64,
    block_timestamp: DateTime<Utc>,
    broadcaster_kind: String,
    gateway_address: String,
    orchestrator_address: String,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct AggregateKey {
    day_utc: NaiveDate,
    broadcaster_kind: String,
}

#[derive(Debug, Clone)]
struct AggregateRow {
    ticket_count: i64,
    distinct_orchestrators: i32,
    distinct_gateways: i32,
    source_max_event_id: i64,
}

impl AggregateRow {
    fn zero() -> Self {
        Self {
            ticket_count: 0,
            distinct_orchestrators: 0,
            distinct_gateways: 0,
            source_max_event_id: 0,
        }
    }
}

pub async fn run_once(
    pg: &PgPool,
    include_tentative: bool,
    batch_limit: i64,
) -> Result<TicketsSummary> {
    let reorg_rows_written = process_reorg_mutations(pg, include_tentative).await?;
    let checkpoint = load_checkpoint(pg).await?;
    let rows = fetch_ticket_rows(pg, include_tentative, checkpoint, batch_limit).await?;
    if rows.is_empty() {
        // TD-023: tick updated_at on empty polls (heartbeat); GREATEST
        // upsert prevents block regression.
        advance_checkpoint(pg, checkpoint.unwrap_or(0)).await?;
        return Ok(TicketsSummary {
            rows_written: reorg_rows_written,
            checkpoint_event_id: checkpoint,
            ..Default::default()
        });
    }

    let mut aggregates: HashMap<AggregateKey, AggregateRow> = HashMap::new();
    let mut seen_gateways = HashSet::new();
    let mut seen_orchestrators = HashSet::new();
    let mut max_event_id = checkpoint.unwrap_or(0);

    for row in &rows {
        let key = AggregateKey {
            day_utc: row.block_timestamp.date_naive(),
            broadcaster_kind: row.broadcaster_kind.clone(),
        };
        let agg = aggregates
            .entry(key.clone())
            .or_insert_with(AggregateRow::zero);
        agg.ticket_count += 1;
        if !seen_gateways.contains(&(key.clone(), row.gateway_address.clone()))
            && !prior_gateway_seen(pg, row, &key).await?
        {
            seen_gateways.insert((key.clone(), row.gateway_address.clone()));
            agg.distinct_gateways += 1;
        } else {
            seen_gateways.insert((key.clone(), row.gateway_address.clone()));
        }
        if !seen_orchestrators.contains(&(key.clone(), row.orchestrator_address.clone()))
            && !prior_orchestrator_seen(pg, row, &key).await?
        {
            seen_orchestrators.insert((key.clone(), row.orchestrator_address.clone()));
            agg.distinct_orchestrators += 1;
        } else {
            seen_orchestrators.insert((key.clone(), row.orchestrator_address.clone()));
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

    let summary = TicketsSummary {
        events_seen: rows.len() as u64,
        rows_written,
        groups_touched,
        checkpoint_event_id: Some(max_event_id),
    };
    info!(?summary, "tickets daily rollup complete");
    Ok(summary)
}

async fn process_reorg_mutations(pg: &PgPool, include_tentative: bool) -> Result<u64> {
    let checkpoint = load_named_checkpoint(pg, REORG_CHECKPOINT_NAME).await?;
    let rows = sqlx::query(
        r#"SELECT m.id AS mutation_id, m.raw_event_id
             FROM reorg_mutations m
             JOIN raw_protocol_events e
               ON e.id = m.raw_event_id
            WHERE m.id > COALESCE($1, 0)
              AND e.chain_id = $2
              AND e.event_name = 'WinningTicketRedeemed'
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

async fn fetch_ticket_rows(
    pg: &PgPool,
    include_tentative: bool,
    resume_from_event_id: Option<i64>,
    limit: i64,
) -> Result<Vec<TicketRow>> {
    let finality_filter = if include_tentative {
        ""
    } else {
        "AND e.finality = 'finalized'"
    };
    let sql = format!(
        r#"SELECT
               e.id AS event_id,
               e.block_timestamp,
               COALESCE(bc.kind, 'transcoding') AS broadcaster_kind,
               e.from_address AS gateway_address,
               e.to_address AS orchestrator_address
           FROM raw_protocol_events e
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
            Ok(TicketRow {
                event_id: row.get("event_id"),
                block_timestamp: row.get("block_timestamp"),
                broadcaster_kind: row.get("broadcaster_kind"),
                gateway_address: row.get("gateway_address"),
                orchestrator_address: row.get("orchestrator_address"),
            })
        })
        .collect()
}

async fn rebuild_aggregate_for_key(
    pg: &PgPool,
    include_tentative: bool,
    key: &AggregateKey,
) -> Result<Option<AggregateRow>> {
    let source_rows = fetch_ticket_rows_for_key(pg, include_tentative, key).await?;
    if source_rows.is_empty() {
        return Ok(None);
    }

    let mut aggregate = AggregateRow::zero();
    let mut gateways = HashSet::new();
    let mut orchestrators = HashSet::new();
    for row in source_rows {
        aggregate.ticket_count += 1;
        aggregate.source_max_event_id = aggregate.source_max_event_id.max(row.event_id);
        if gateways.insert(row.gateway_address) {
            aggregate.distinct_gateways += 1;
        }
        if orchestrators.insert(row.orchestrator_address) {
            aggregate.distinct_orchestrators += 1;
        }
    }
    Ok(Some(aggregate))
}

async fn prior_gateway_seen(pg: &PgPool, row: &TicketRow, key: &AggregateKey) -> Result<bool> {
    let exists = sqlx::query_scalar::<_, i32>(
        r#"SELECT 1
             FROM raw_protocol_events e
        LEFT JOIN broadcaster_classifications bc
               ON bc.chain_id = e.chain_id
              AND bc.address = e.from_address
            WHERE e.chain_id = $1
              AND e.event_name = 'WinningTicketRedeemed'
              AND e.is_canonical = TRUE
              AND e.id < $2
              AND e.from_address = $3
              AND e.block_timestamp >= $4::date
              AND e.block_timestamp < ($4::date + INTERVAL '1 day')
              AND COALESCE(bc.kind, 'transcoding') = $5
            LIMIT 1"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(row.event_id)
    .bind(&row.gateway_address)
    .bind(key.day_utc)
    .bind(&key.broadcaster_kind)
    .fetch_optional(pg)
    .await?;
    Ok(exists.is_some())
}

async fn prior_orchestrator_seen(pg: &PgPool, row: &TicketRow, key: &AggregateKey) -> Result<bool> {
    let exists = sqlx::query_scalar::<_, i32>(
        r#"SELECT 1
             FROM raw_protocol_events e
        LEFT JOIN broadcaster_classifications bc
               ON bc.chain_id = e.chain_id
              AND bc.address = e.from_address
            WHERE e.chain_id = $1
              AND e.event_name = 'WinningTicketRedeemed'
              AND e.is_canonical = TRUE
              AND e.id < $2
              AND e.to_address = $3
              AND e.block_timestamp >= $4::date
              AND e.block_timestamp < ($4::date + INTERVAL '1 day')
              AND COALESCE(bc.kind, 'transcoding') = $5
            LIMIT 1"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(row.event_id)
    .bind(&row.orchestrator_address)
    .bind(key.day_utc)
    .bind(&key.broadcaster_kind)
    .fetch_optional(pg)
    .await?;
    Ok(exists.is_some())
}

async fn load_existing_candidate_keys(pg: &PgPool, event_id: i64) -> Result<Vec<AggregateKey>> {
    let rows = sqlx::query(
        r#"SELECT day_utc, broadcaster_kind
             FROM tickets_daily
            WHERE chain_id = $1
              AND source_max_event_id >= $2"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(event_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| AggregateKey {
            day_utc: row.get("day_utc"),
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
        r#"SELECT DATE(e.block_timestamp) AS day_utc,
                  COALESCE(bc.kind, 'transcoding') AS broadcaster_kind
             FROM raw_protocol_events e
        LEFT JOIN broadcaster_classifications bc
               ON bc.chain_id = e.chain_id
              AND bc.address = e.from_address
            WHERE e.id = $1
              AND e.chain_id = $2
              AND e.event_name = 'WinningTicketRedeemed'
              AND e.is_canonical = TRUE
              {finality_filter}
              AND e.from_address IS NOT NULL
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
            broadcaster_kind: row.get("broadcaster_kind"),
        })
        .collect())
}

async fn fetch_ticket_rows_for_key(
    pg: &PgPool,
    include_tentative: bool,
    key: &AggregateKey,
) -> Result<Vec<TicketRow>> {
    let finality_filter = if include_tentative {
        ""
    } else {
        "AND e.finality = 'finalized'"
    };
    let sql = format!(
        r#"SELECT
               e.id AS event_id,
               e.block_timestamp,
               COALESCE(bc.kind, 'transcoding') AS broadcaster_kind,
               e.from_address AS gateway_address,
               e.to_address AS orchestrator_address
           FROM raw_protocol_events e
      LEFT JOIN broadcaster_classifications bc
             ON bc.chain_id = e.chain_id
            AND bc.address = e.from_address
          WHERE e.chain_id = $1
            AND e.event_name = 'WinningTicketRedeemed'
            AND e.is_canonical = TRUE
            {finality_filter}
            AND e.from_address IS NOT NULL
            AND e.to_address IS NOT NULL
            AND DATE(e.block_timestamp) = $2
            AND COALESCE(bc.kind, 'transcoding') = $3
       ORDER BY e.id ASC"#,
    );
    let rows = sqlx::query(&sql)
        .bind(ARBITRUM_CHAIN_ID)
        .bind(key.day_utc)
        .bind(&key.broadcaster_kind)
        .fetch_all(pg)
        .await?;
    rows.into_iter()
        .map(|row| {
            Ok(TicketRow {
                event_id: row.get("event_id"),
                block_timestamp: row.get("block_timestamp"),
                broadcaster_kind: row.get("broadcaster_kind"),
                gateway_address: row.get("gateway_address"),
                orchestrator_address: row.get("orchestrator_address"),
            })
        })
        .collect()
}

async fn upsert_aggregate(pg: &PgPool, key: &AggregateKey, row: &AggregateRow) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO tickets_daily (
               chain_id,
               day_utc,
               broadcaster_kind,
               ticket_count,
               distinct_orchestrators,
               distinct_gateways,
               source_max_event_id,
               updated_at
           ) VALUES ($1, $2, $3, $4, $5, $6, $7, now())
           ON CONFLICT (chain_id, day_utc, broadcaster_kind) DO UPDATE
               SET ticket_count = tickets_daily.ticket_count + EXCLUDED.ticket_count,
                   distinct_orchestrators = tickets_daily.distinct_orchestrators + EXCLUDED.distinct_orchestrators,
                   distinct_gateways = tickets_daily.distinct_gateways + EXCLUDED.distinct_gateways,
                   source_max_event_id = GREATEST(tickets_daily.source_max_event_id, EXCLUDED.source_max_event_id),
                   updated_at = now()"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(key.day_utc)
    .bind(&key.broadcaster_kind)
    .bind(row.ticket_count)
    .bind(row.distinct_orchestrators)
    .bind(row.distinct_gateways)
    .bind(row.source_max_event_id)
    .execute(pg)
    .await?;
    Ok(())
}

async fn replace_aggregate(pg: &PgPool, key: &AggregateKey, row: &AggregateRow) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO tickets_daily (
               chain_id,
               day_utc,
               broadcaster_kind,
               ticket_count,
               distinct_orchestrators,
               distinct_gateways,
               source_max_event_id,
               updated_at
           ) VALUES ($1, $2, $3, $4, $5, $6, $7, now())
           ON CONFLICT (chain_id, day_utc, broadcaster_kind) DO UPDATE
               SET ticket_count = EXCLUDED.ticket_count,
                   distinct_orchestrators = EXCLUDED.distinct_orchestrators,
                   distinct_gateways = EXCLUDED.distinct_gateways,
                   source_max_event_id = EXCLUDED.source_max_event_id,
                   updated_at = now()"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(key.day_utc)
    .bind(&key.broadcaster_kind)
    .bind(row.ticket_count)
    .bind(row.distinct_orchestrators)
    .bind(row.distinct_gateways)
    .bind(row.source_max_event_id)
    .execute(pg)
    .await?;
    Ok(())
}

async fn delete_aggregate(pg: &PgPool, key: &AggregateKey) -> Result<()> {
    sqlx::query(
        r#"DELETE FROM tickets_daily
            WHERE chain_id = $1
              AND day_utc = $2
              AND broadcaster_kind = $3"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(key.day_utc)
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
