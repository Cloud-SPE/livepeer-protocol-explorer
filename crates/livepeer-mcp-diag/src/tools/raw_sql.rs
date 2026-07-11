//! `raw_sql` — SELECT-only escape hatch for anything the curated probes miss.
//! Guarded by `sql_guard` (defense-in-depth), executed by the read-only pool
//! inside a READ ONLY transaction, wrapped with an enforced `LIMIT`, and
//! per-cell truncated for token budget. The real safety boundary is the
//! `diag_ro` DB role, not this code.

use crate::adapters::sql_guard;
use crate::context::DiagContext;
use crate::output;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
pub struct RawSqlResult {
    pub row_count: usize,
    pub row_limit: i64,
    pub truncated: bool,
    pub rows: Vec<Value>,
}

pub async fn run(ctx: &DiagContext, sql: &str, limit: Option<i64>) -> anyhow::Result<RawSqlResult> {
    let stmt = sql_guard::validate(sql).map_err(|e| anyhow::anyhow!("rejected: {e}"))?;

    let requested = limit.unwrap_or(output::MAX_RAW_ROWS).clamp(1, output::MAX_RAW_ROWS);
    // Fetch one extra row to detect truncation without a second query.
    let mut rows = ctx.db.raw_select(&stmt, requested + 1).await?;
    let truncated = rows.len() as i64 > requested;
    rows.truncate(requested as usize);
    rows.iter_mut().for_each(output::truncate_cells);

    Ok(RawSqlResult {
        row_count: rows.len(),
        row_limit: requested,
        truncated,
        rows,
    })
}
