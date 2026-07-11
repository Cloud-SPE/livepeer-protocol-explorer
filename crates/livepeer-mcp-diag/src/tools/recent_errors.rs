//! `recent_errors` — grouped counts across the pipeline's error surfaces
//! (decode dead-letters, reorgs, RPC cross-check divergences, failed pricing
//! attempts), plus a small recent-decode-failure sample. Counts, not dumps.

use crate::context::DiagContext;
use crate::queries;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct DecodeFailureSample {
    pub block_number: i64,
    pub tx_hash: String,
    pub topics: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct RecentErrors {
    pub counts: BTreeMap<String, i64>,
    pub recent_decode_failures: Vec<DecodeFailureSample>,
}

pub async fn run(ctx: &DiagContext) -> anyhow::Result<RecentErrors> {
    let pool = ctx.db.pool();

    let count_rows: Vec<(String, i64)> = sqlx::query_as(queries::ERROR_COUNTS).fetch_all(pool).await?;
    let counts: BTreeMap<String, i64> = count_rows.into_iter().collect();

    let recent_decode_failures: Vec<DecodeFailureSample> =
        sqlx::query_as(queries::RECENT_DECODE_FAILURES).fetch_all(pool).await?;

    Ok(RecentErrors {
        counts,
        recent_decode_failures,
    })
}
