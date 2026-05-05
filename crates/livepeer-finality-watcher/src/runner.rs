use anyhow::{Context, Result};
use livepeer_core::rpc::{BlockTag, Provider};
use sqlx::PgPool;

pub const SERVICE: &str = "livepeer-finality-watcher";
pub const ARBITRUM_CHAIN_ID: i64 = 42161;
pub const POSTING_LAG_SECS: i64 = 600;
pub const FINALITY_SAFETY_MARGIN_SECS: i64 = 60;
pub const CADENCE_SECS: u64 = 60;
const REPLAY_LATEST_TS_CHECKPOINT: &str = "replay_finality_latest_l1_ts";
const REPLAY_FINALIZED_TS_CHECKPOINT: &str = "replay_finality_finalized_l1_ts";

#[derive(Debug, Default)]
pub struct FinalityIterationSummary {
    pub latest_l1_ts: i64,
    pub finalized_l1_ts: i64,
    pub l1_posted_cutoff: i64,
    pub finalized_cutoff: i64,
    pub rows_marked_l1_posted: u64,
    pub rows_marked_finalized: u64,
}

pub async fn run_once(pg: &PgPool, l1: &Provider) -> Result<FinalityIterationSummary> {
    let latest_ts = l1_block_timestamp(l1, BlockTag::Latest).await?;
    let finalized_ts = l1_block_timestamp_str(l1, "finalized").await?;
    persist_replay_inputs(pg, latest_ts, finalized_ts).await?;
    run_with_timestamps(pg, latest_ts, finalized_ts).await
}

pub async fn run_once_replay(pg: &PgPool) -> Result<FinalityIterationSummary> {
    let latest_ts = replay_input(pg, REPLAY_LATEST_TS_CHECKPOINT).await?;
    let finalized_ts = replay_input(pg, REPLAY_FINALIZED_TS_CHECKPOINT).await?;
    run_with_timestamps(pg, latest_ts, finalized_ts).await
}

async fn run_with_timestamps(
    pg: &PgPool,
    latest_ts: i64,
    finalized_ts: i64,
) -> Result<FinalityIterationSummary> {
    let l1_posted_cutoff = latest_ts - POSTING_LAG_SECS;
    let finalized_cutoff = finalized_ts - FINALITY_SAFETY_MARGIN_SECS;

    let posted = sqlx::query(
        r#"UPDATE raw_protocol_events
              SET finality = 'l1_posted'
            WHERE chain_id = $1
              AND finality = 'tentative'
              AND is_canonical = TRUE
              AND block_timestamp <= to_timestamp($2)"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(l1_posted_cutoff as f64)
    .execute(pg)
    .await?
    .rows_affected();

    let finalized = sqlx::query(
        r#"UPDATE raw_protocol_events
              SET finality   = 'finalized',
                  finalized_at = now()
            WHERE chain_id = $1
              AND finality IN ('tentative', 'l1_posted')
              AND is_canonical = TRUE
              AND block_timestamp <= to_timestamp($2)"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(finalized_cutoff as f64)
    .execute(pg)
    .await?
    .rows_affected();

    sqlx::query(
        r#"INSERT INTO indexer_checkpoints
                (name, chain_id, last_processed_block, updated_at)
           VALUES ('finality_watcher', $1, $2, now())
           ON CONFLICT (name) DO UPDATE
              SET last_processed_block = GREATEST(indexer_checkpoints.last_processed_block, EXCLUDED.last_processed_block),
                  updated_at = now()"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(finalized_cutoff)
    .execute(pg)
    .await?;

    Ok(FinalityIterationSummary {
        latest_l1_ts: latest_ts,
        finalized_l1_ts: finalized_ts,
        l1_posted_cutoff,
        finalized_cutoff,
        rows_marked_l1_posted: posted,
        rows_marked_finalized: finalized,
    })
}

async fn l1_block_timestamp(l1: &Provider, tag: BlockTag) -> Result<i64> {
    let header = l1.eth_get_block_header(tag).await?;
    let ts_hex = header
        .get("timestamp")
        .and_then(|v| v.as_str())
        .context("L1 block header missing .timestamp")?;
    let ts = i64::from_str_radix(ts_hex.trim_start_matches("0x"), 16)?;
    Ok(ts)
}

async fn l1_block_timestamp_str(l1: &Provider, tag: &str) -> Result<i64> {
    let v = l1
        .call("eth_getBlockByNumber", &serde_json::json!([tag, false]))
        .await?;
    let ts_hex = v
        .get("timestamp")
        .and_then(|v| v.as_str())
        .with_context(|| format!("L1 block header at tag={tag} missing .timestamp"))?;
    let ts = i64::from_str_radix(ts_hex.trim_start_matches("0x"), 16)?;
    Ok(ts)
}

async fn persist_replay_inputs(pg: &PgPool, latest_ts: i64, finalized_ts: i64) -> Result<()> {
    for (name, value) in [
        (REPLAY_LATEST_TS_CHECKPOINT, latest_ts),
        (REPLAY_FINALIZED_TS_CHECKPOINT, finalized_ts),
    ] {
        sqlx::query(
            r#"INSERT INTO indexer_checkpoints
                    (name, chain_id, last_processed_block, updated_at)
               VALUES ($1, $2, $3, now())
               ON CONFLICT (name) DO UPDATE
                  SET last_processed_block = EXCLUDED.last_processed_block,
                      updated_at = now()"#,
        )
        .bind(name)
        .bind(ARBITRUM_CHAIN_ID)
        .bind(value)
        .execute(pg)
        .await?;
    }
    Ok(())
}

async fn replay_input(pg: &PgPool, name: &str) -> Result<i64> {
    sqlx::query_scalar("SELECT last_processed_block FROM indexer_checkpoints WHERE name = $1")
        .bind(name)
        .fetch_optional(pg)
        .await?
        .with_context(|| format!("missing replay finality input checkpoint {name}"))
}
