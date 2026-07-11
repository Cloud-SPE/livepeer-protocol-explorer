//! `report_readiness` — daily rollup freshness + the upstream that blocks it.
//! Rollups expose no metrics endpoint, so this is entirely DB-driven. Weekly /
//! monthly summaries are aggregated on read from these same daily tables, so
//! daily coverage is the readiness signal for all three cadences.

use super::CheckpointRow;
use crate::context::DiagContext;
use crate::queries;
use chrono::{DateTime, NaiveDate, Utc};
use std::collections::HashMap;

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct RollupEntry {
    pub checkpoint: String,
    pub table: String,
    pub max_day: Option<NaiveDate>,
    /// Whole days between today (UTC) and the latest materialized day.
    pub days_behind: Option<i64>,
    pub checkpoint_age_secs: Option<i64>,
    pub stale: bool,
    /// Max source event_id folded into the rollup (checkpoint value).
    pub max_source_event_id: Option<i64>,
    /// max_priced_event_id - max_source_event_id: how many priced events the
    /// rollup has yet to fold. Negative/zero ≈ caught up.
    pub events_behind_priced: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct Upstream {
    pub finality_age_secs: Option<i64>,
    pub finality_lagging: bool,
    pub pricing_not_priced_backlog: i64,
    pub pricing_backlogged: bool,
    /// Best-guess stage blocking newer rollup days.
    pub likely_blocker: String,
}

#[derive(Debug, Serialize)]
pub struct ReportReadiness {
    pub today_utc: NaiveDate,
    pub max_priced_event_id: Option<i64>,
    pub rollups: Vec<RollupEntry>,
    pub upstream: Upstream,
}

#[derive(Debug, sqlx::FromRow)]
struct FinalityRow {
    #[allow(dead_code)]
    frontier_block: Option<i64>,
    #[allow(dead_code)]
    finalized_at: Option<DateTime<Utc>>,
    age_secs: Option<i64>,
}

pub async fn run(ctx: &DiagContext) -> anyhow::Result<ReportReadiness> {
    let version = ctx.valuation_version().to_string();
    let pool = ctx.db.pool();
    let today = Utc::now().date_naive();

    let checkpoints: Vec<CheckpointRow> =
        sqlx::query_as(queries::CHECKPOINTS).fetch_all(pool).await?;
    let cp_by_name: HashMap<&str, &CheckpointRow> =
        checkpoints.iter().map(|c| (c.name.as_str(), c)).collect();

    let max_days: Vec<(String, Option<NaiveDate>)> = sqlx::query_as(queries::ROLLUP_MAX_DAYS)
        .fetch_all(pool)
        .await?;
    let max_day_by_table: HashMap<String, Option<NaiveDate>> = max_days.into_iter().collect();

    let max_priced_event_id: Option<i64> = sqlx::query_scalar(queries::MAX_PRICED_EVENT_ID)
        .bind(&version)
        .bind(queries::DEGRADED_VALUATION_VERSION)
        .fetch_one(pool)
        .await?;

    let finality: FinalityRow = sqlx::query_as(queries::FINALITY_FRONTIER)
        .fetch_one(pool)
        .await?;

    let backlog: i64 = {
        let row: (i64, Option<DateTime<Utc>>, Option<i64>) =
            sqlx::query_as(queries::PRICING_BACKLOG)
                .bind(&version)
                .bind(queries::DEGRADED_VALUATION_VERSION)
                .fetch_one(pool)
                .await?;
        row.0
    };

    let rollup_stale = ctx.cfg.thresholds.rollup_stale_secs;
    let mut rollups = Vec::new();
    for (checkpoint, table) in queries::ROLLUPS {
        let cp = cp_by_name.get(checkpoint);
        let max_day = max_day_by_table.get(table).copied().flatten();
        let checkpoint_age_secs = cp.map(|c| c.age_secs);
        let max_source_event_id = cp.map(|c| c.last_processed_block);
        let events_behind_priced = match (max_priced_event_id, max_source_event_id) {
            (Some(p), Some(s)) => Some(p - s),
            _ => None,
        };
        rollups.push(RollupEntry {
            checkpoint: checkpoint.to_string(),
            table: table.to_string(),
            max_day,
            days_behind: max_day.map(|d| (today - d).num_days()),
            checkpoint_age_secs,
            stale: checkpoint_age_secs
                .map(|a| a > rollup_stale)
                .unwrap_or(true),
            max_source_event_id,
            events_behind_priced,
        });
    }

    let finality_lagging = finality
        .age_secs
        .map(|a| a > ctx.cfg.thresholds.finality_lag_secs)
        .unwrap_or(false);
    let pricing_backlogged = backlog > ctx.cfg.thresholds.pricing_backlog;
    let likely_blocker = if finality_lagging {
        "finality"
    } else if pricing_backlogged {
        "pricing"
    } else if rollups.iter().any(|r| r.stale) {
        "rollup_worker"
    } else {
        "none"
    }
    .to_string();

    Ok(ReportReadiness {
        today_utc: today,
        max_priced_event_id,
        rollups,
        upstream: Upstream {
            finality_age_secs: finality.age_secs,
            finality_lagging,
            pricing_not_priced_backlog: backlog,
            pricing_backlogged,
            likely_blocker,
        },
    })
}
