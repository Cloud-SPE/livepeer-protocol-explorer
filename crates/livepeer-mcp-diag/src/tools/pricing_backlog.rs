//! `pricing_backlog` — how far behind the valuator is, and why.
//! Distinguishes not-yet-priced (no valuation row) from terminally-failed
//! (row with `status LIKE 'failed_%'`, `amount_usd` NULL). Counting
//! `amount_usd IS NULL` would conflate the two and overcount massively.

use crate::context::DiagContext;
use crate::queries;
use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, sqlx::FromRow)]
struct BacklogRow {
    backlog: i64,
    oldest_ts: Option<DateTime<Utc>>,
    oldest_age_secs: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct TerminalFailure {
    pub status: String,
    pub count: i64,
}

#[derive(Debug, Serialize)]
pub struct PricingBacklog {
    pub valuation_version: String,
    /// Valuable+canonical+finalized events with no valuation row yet.
    pub not_priced_backlog: i64,
    pub oldest_unpriced_ts: Option<DateTime<Utc>>,
    pub oldest_unpriced_age_secs: Option<i64>,
    /// Retries whose `next_retry_at` is already due — a proxy for stuck
    /// transient failures / backpressure.
    pub pending_retries_due: i64,
    /// Permanently-failed valuations by status (excluded from the backlog).
    pub terminal_failures: Vec<TerminalFailure>,
    pub backlog_threshold: i64,
    pub over_threshold: bool,
}

pub async fn run(ctx: &DiagContext) -> anyhow::Result<PricingBacklog> {
    let version = ctx.valuation_version().to_string();

    let backlog: BacklogRow = sqlx::query_as(queries::PRICING_BACKLOG)
        .bind(&version)
        .bind(queries::DEGRADED_VALUATION_VERSION)
        .fetch_one(ctx.db.pool())
        .await?;

    let failures: Vec<(String, i64)> = sqlx::query_as(queries::TERMINAL_FAILURES)
        .bind(&version)
        .fetch_all(ctx.db.pool())
        .await?;

    let pending_retries_due: i64 = sqlx::query_scalar(queries::PENDING_RETRIES)
        .fetch_one(ctx.db.pool())
        .await?;

    let threshold = ctx.cfg.thresholds.pricing_backlog;

    Ok(PricingBacklog {
        valuation_version: version,
        not_priced_backlog: backlog.backlog,
        oldest_unpriced_ts: backlog.oldest_ts,
        oldest_unpriced_age_secs: backlog.oldest_age_secs,
        pending_retries_due,
        terminal_failures: failures
            .into_iter()
            .map(|(status, count)| TerminalFailure { status, count })
            .collect(),
        backlog_threshold: threshold,
        over_threshold: backlog.backlog > threshold,
    })
}
