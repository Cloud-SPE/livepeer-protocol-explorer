//! `indexer_health` — per-contract indexer checkpoint lag + staleness.
//! Catches the "daemon stalled, needs restart" symptom: a wedged tokio task
//! stops advancing its checkpoint, so `age_secs` climbs even while the chain
//! head moves on.

use super::CheckpointRow;
use crate::context::DiagContext;
use crate::queries;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct IndexerEntry {
    pub name: String,
    pub last_processed_block: i64,
    pub age_secs: i64,
    pub stale: bool,
    /// chain_head - last_processed_block, when the chain head is known.
    pub lag_blocks: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct IndexerHealth {
    pub chain_head_block: Option<i64>,
    pub min_processed_block: Option<i64>,
    pub max_age_secs: i64,
    pub any_stale: bool,
    pub stale_threshold_secs: i64,
    pub contracts: Vec<IndexerEntry>,
}

pub async fn run(ctx: &DiagContext) -> anyhow::Result<IndexerHealth> {
    let rows: Vec<CheckpointRow> = sqlx::query_as(queries::CHECKPOINTS)
        .fetch_all(ctx.db.pool())
        .await?;

    let chain_head = ctx.metrics.chain_head().await;
    let threshold = ctx.cfg.thresholds.indexer_stale_secs;

    let mut contracts = Vec::new();
    let mut min_block: Option<i64> = None;
    let mut max_age = 0i64;
    let mut any_stale = false;

    for r in rows
        .into_iter()
        .filter(|r| queries::INDEXER_CHECKPOINT_NAMES.contains(&r.name.as_str()))
    {
        let stale = r.age_secs > threshold;
        any_stale |= stale;
        max_age = max_age.max(r.age_secs);
        min_block =
            Some(min_block.map_or(r.last_processed_block, |m| m.min(r.last_processed_block)));
        let lag_blocks = chain_head.map(|h| h - r.last_processed_block);
        contracts.push(IndexerEntry {
            name: r.name,
            last_processed_block: r.last_processed_block,
            age_secs: r.age_secs,
            stale,
            lag_blocks,
        });
    }

    Ok(IndexerHealth {
        chain_head_block: chain_head,
        min_processed_block: min_block,
        max_age_secs: max_age,
        any_stale,
        stale_threshold_secs: threshold,
        contracts,
    })
}
