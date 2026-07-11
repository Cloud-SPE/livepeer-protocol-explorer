//! `dependency_chain` — the "why is everything late" button.
//!
//! All three production symptoms are the same failure observed at different
//! points on one ladder: indexer → finality → valuation → rollups. Each stage
//! feeds the next, so this walks the ladder top-down and reports the FIRST
//! stage that breaches its threshold — the root cause, not the downstream
//! symptom. Signals are heterogeneous by stage (block staleness, timestamp
//! lag, backlog count) because the pipeline is not a single block cursor.

use super::CheckpointRow;
use crate::context::DiagContext;
use crate::queries;
use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct IndexerStage {
    pub min_processed_block: Option<i64>,
    pub chain_head_block: Option<i64>,
    pub lag_blocks: Option<i64>,
    pub max_age_secs: i64,
    pub stalled: bool,
}

#[derive(Debug, Serialize)]
pub struct FinalityStage {
    pub frontier_block: Option<i64>,
    pub age_secs: Option<i64>,
    pub lagging: bool,
}

#[derive(Debug, Serialize)]
pub struct ValuationStage {
    pub not_priced_backlog: i64,
    pub oldest_unpriced_age_secs: Option<i64>,
    pub backlogged: bool,
}

#[derive(Debug, Serialize)]
pub struct RollupStageEntry {
    pub checkpoint: String,
    pub age_secs: Option<i64>,
    pub stale: bool,
}

#[derive(Debug, Serialize)]
pub struct RollupStage {
    pub worst_age_secs: Option<i64>,
    pub any_stale: bool,
    pub rollups: Vec<RollupStageEntry>,
}

#[derive(Debug, Serialize)]
pub struct DependencyChain {
    /// First breached stage: "indexer" | "finality" | "pricing" | "rollup" |
    /// "ok". This is the thing to investigate first.
    pub blocked_at: String,
    pub explanation: String,
    pub indexer: IndexerStage,
    pub finality: FinalityStage,
    pub valuation: ValuationStage,
    pub rollups: RollupStage,
}

#[derive(Debug, sqlx::FromRow)]
struct FinalityRow {
    frontier_block: Option<i64>,
    #[allow(dead_code)]
    finalized_at: Option<DateTime<Utc>>,
    age_secs: Option<i64>,
}

pub async fn run(ctx: &DiagContext) -> anyhow::Result<DependencyChain> {
    let pool = ctx.db.pool();
    let version = ctx.valuation_version().to_string();
    let th = &ctx.cfg.thresholds;

    let checkpoints: Vec<CheckpointRow> = sqlx::query_as(queries::CHECKPOINTS).fetch_all(pool).await?;
    let chain_head = ctx.metrics.chain_head().await;

    // ── Stage 1: indexer ────────────────────────────────────────────────
    let indexer_cps: Vec<&CheckpointRow> = checkpoints
        .iter()
        .filter(|c| queries::INDEXER_CHECKPOINT_NAMES.contains(&c.name.as_str()))
        .collect();
    let min_block = indexer_cps.iter().map(|c| c.last_processed_block).min();
    let indexer_max_age = indexer_cps.iter().map(|c| c.age_secs).max().unwrap_or(0);
    let lag_blocks = match (chain_head, min_block) {
        (Some(h), Some(b)) => Some(h - b),
        _ => None,
    };
    let indexer_stalled = indexer_max_age > th.indexer_stale_secs;

    // ── Stage 2: finality ───────────────────────────────────────────────
    let finality: FinalityRow = sqlx::query_as(queries::FINALITY_FRONTIER).fetch_one(pool).await?;
    let finality_lagging = finality
        .age_secs
        .map(|a| a > th.finality_lag_secs)
        .unwrap_or(false);

    // ── Stage 3: valuation ──────────────────────────────────────────────
    let backlog_row: (i64, Option<DateTime<Utc>>, Option<i64>) =
        sqlx::query_as(queries::PRICING_BACKLOG)
            .bind(&version)
            .bind(queries::DEGRADED_VALUATION_VERSION)
            .fetch_one(pool)
            .await?;
    let backlogged = backlog_row.0 > th.pricing_backlog;

    // ── Stage 4: rollups ────────────────────────────────────────────────
    let mut rollup_entries = Vec::new();
    let mut worst_age: Option<i64> = None;
    let mut any_stale = false;
    for (checkpoint, _table) in queries::ROLLUPS {
        let cp = checkpoints.iter().find(|c| c.name == checkpoint);
        let age = cp.map(|c| c.age_secs);
        let stale = age.map(|a| a > th.rollup_stale_secs).unwrap_or(true);
        any_stale |= stale;
        if let Some(a) = age {
            worst_age = Some(worst_age.map_or(a, |w| w.max(a)));
        }
        rollup_entries.push(RollupStageEntry {
            checkpoint: checkpoint.to_string(),
            age_secs: age,
            stale,
        });
    }

    // ── Verdict: first breached stage, top-down ─────────────────────────
    let (blocked_at, explanation) = if indexer_stalled {
        (
            "indexer",
            format!(
                "indexer checkpoints stale for up to {indexer_max_age}s (> {}s); a daemon task is likely wedged",
                th.indexer_stale_secs
            ),
        )
    } else if finality_lagging {
        (
            "finality",
            format!(
                "no new finalized events for {:?}s (> {}s)",
                finality.age_secs, th.finality_lag_secs
            ),
        )
    } else if backlogged {
        (
            "pricing",
            format!(
                "{} finalized events unpriced (> {}); valuator is behind",
                backlog_row.0, th.pricing_backlog
            ),
        )
    } else if any_stale {
        (
            "rollup",
            "a rollup checkpoint is stale — the standalone rollup container may be down".to_string(),
        )
    } else {
        ("ok", "all stages within thresholds".to_string())
    };

    Ok(DependencyChain {
        blocked_at: blocked_at.to_string(),
        explanation,
        indexer: IndexerStage {
            min_processed_block: min_block,
            chain_head_block: chain_head,
            lag_blocks,
            max_age_secs: indexer_max_age,
            stalled: indexer_stalled,
        },
        finality: FinalityStage {
            frontier_block: finality.frontier_block,
            age_secs: finality.age_secs,
            lagging: finality_lagging,
        },
        valuation: ValuationStage {
            not_priced_backlog: backlog_row.0,
            oldest_unpriced_age_secs: backlog_row.2,
            backlogged,
        },
        rollups: RollupStage {
            worst_age_secs: worst_age,
            any_stale,
            rollups: rollup_entries,
        },
    })
}
