//! `worker_logs` — tail a container's logs through the read-only proxy. The
//! *why* behind a stall: which chunk, which RPC error, retry backoff. Capped
//! to a bounded, most-recent tail for token budget.

use crate::context::DiagContext;
use crate::output;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct WorkerLogs {
    pub container: String,
    pub line_count: usize,
    pub truncated: bool,
    pub since_secs: Option<i64>,
    pub lines: Vec<String>,
}

pub async fn run(
    ctx: &DiagContext,
    container: &str,
    lines: Option<usize>,
    since_secs: Option<i64>,
) -> anyhow::Result<WorkerLogs> {
    let want = lines.unwrap_or(output::DEFAULT_LOG_LINES);
    let (lines, truncated) = ctx
        .docker
        .container_logs(container, want, since_secs)
        .await?;
    Ok(WorkerLogs {
        container: container.to_string(),
        line_count: lines.len(),
        truncated,
        since_secs,
        lines,
    })
}
