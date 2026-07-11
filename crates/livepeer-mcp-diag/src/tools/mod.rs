//! Diagnostic tool implementations. Each `run(...)` takes `&DiagContext`,
//! returns a `Serialize` result, and is free of rmcp/transport concerns — the
//! `server` module adapts these into MCP tools.

pub mod container_state;
pub mod dependency_chain;
pub mod indexer_health;
pub mod pricing_backlog;
pub mod raw_sql;
pub mod recent_errors;
pub mod report_readiness;
pub mod scrape_metrics;
pub mod worker_logs;

use chrono::{DateTime, Utc};
use serde::Serialize;

/// A row of `indexer_checkpoints` with computed age. Shared by the checkpoint
/// probes.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct CheckpointRow {
    pub name: String,
    pub last_processed_block: i64,
    pub updated_at: DateTime<Utc>,
    pub age_secs: i64,
}
