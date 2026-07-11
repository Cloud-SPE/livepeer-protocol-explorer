//! rmcp server: exposes each diagnostic probe as an MCP tool. Tool bodies stay
//! thin — they delegate to `tools::*` (which are transport-agnostic) and adapt
//! the result into a JSON `CallToolResult`.

use crate::context::DiagContext;
use crate::tools;
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler,
};
use serde::Serialize;
use std::sync::Arc;

#[derive(Clone)]
pub struct DiagServer {
    ctx: Arc<DiagContext>,
    // Read by the #[tool_handler] macro, not by our code directly.
    #[allow(dead_code)]
    tool_router: ToolRouter<DiagServer>,
}

impl DiagServer {
    pub fn new(ctx: Arc<DiagContext>) -> Self {
        Self {
            ctx,
            tool_router: Self::tool_router(),
        }
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RawSqlParams {
    /// A single read-only `SELECT` or `WITH` statement. Writes and
    /// multi-statement input are rejected.
    pub sql: String,
    /// Max rows to return (default and hard cap: 200).
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ScrapeMetricsParams {
    /// Optional substring filter on metric names (e.g. "lag", "rpc").
    #[serde(default)]
    pub name_filter: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WorkerLogsParams {
    /// Container name (or id), e.g. "livepeer-rollups-payouts".
    pub container: String,
    /// Lines to tail from the end (default 200, max 1000).
    #[serde(default)]
    pub lines: Option<usize>,
    /// Only logs newer than this many seconds ago.
    #[serde(default)]
    pub since_secs: Option<i64>,
}

#[tool_router]
impl DiagServer {
    #[tool(
        description = "Start here. Walks the indexer→finality→valuation→rollups pipeline and reports the FIRST stage that is stalled or backlogged — the root cause behind indexing stalls, slow pricing, or late reports."
    )]
    async fn dependency_chain(&self) -> Result<CallToolResult, McpError> {
        json_ok(
            &tools::dependency_chain::run(&self.ctx)
                .await
                .map_err(to_mcp_err)?,
        )
    }

    #[tool(
        description = "Per-contract indexer checkpoint lag vs chain head and staleness (now - updated_at). A climbing age with a moving chain head means a wedged daemon task."
    )]
    async fn indexer_health(&self) -> Result<CallToolResult, McpError> {
        json_ok(
            &tools::indexer_health::run(&self.ctx)
                .await
                .map_err(to_mcp_err)?,
        )
    }

    #[tool(
        description = "Pricing/valuation backlog for the active version: not-yet-priced count, oldest unpriced event age, due retries, and terminal-failure breakdown by status."
    )]
    async fn pricing_backlog(&self) -> Result<CallToolResult, McpError> {
        json_ok(
            &tools::pricing_backlog::run(&self.ctx)
                .await
                .map_err(to_mcp_err)?,
        )
    }

    #[tool(
        description = "Daily rollup freshness (latest materialized day, checkpoint staleness, events behind priced frontier) and the upstream stage most likely blocking newer report days."
    )]
    async fn report_readiness(&self) -> Result<CallToolResult, McpError> {
        json_ok(
            &tools::report_readiness::run(&self.ctx)
                .await
                .map_err(to_mcp_err)?,
        )
    }

    #[tool(
        description = "Grouped error counts across decode dead-letters, reorgs, RPC cross-check divergences, and failed pricing attempts, with a small recent decode-failure sample."
    )]
    async fn recent_errors(&self) -> Result<CallToolResult, McpError> {
        json_ok(
            &tools::recent_errors::run(&self.ctx)
                .await
                .map_err(to_mcp_err)?,
        )
    }

    #[tool(
        description = "Raw escape hatch: scrape the configured Prometheus /metrics endpoints (daemon/enricher/api). Optionally filter metric names by substring. Dead endpoints are reported, not fatal."
    )]
    async fn scrape_metrics(
        &self,
        Parameters(p): Parameters<ScrapeMetricsParams>,
    ) -> Result<CallToolResult, McpError> {
        json_ok(
            &tools::scrape_metrics::run(&self.ctx, p.name_filter)
                .await
                .map_err(to_mcp_err)?,
        )
    }

    #[tool(
        description = "Container liveness (docker ps equivalent) via the read-only proxy. Surfaces silently-crashed standalone workers (rollups, enricher, tx-receipts) as running:false."
    )]
    async fn container_state(&self) -> Result<CallToolResult, McpError> {
        json_ok(
            &tools::container_state::run(&self.ctx)
                .await
                .map_err(to_mcp_err)?,
        )
    }

    #[tool(
        description = "Tail a container's logs via the read-only proxy — the 'why' behind a stall (stuck chunk, RPC error, retry backoff). Bounded, most-recent tail."
    )]
    async fn worker_logs(
        &self,
        Parameters(p): Parameters<WorkerLogsParams>,
    ) -> Result<CallToolResult, McpError> {
        json_ok(
            &tools::worker_logs::run(&self.ctx, &p.container, p.lines, p.since_secs)
                .await
                .map_err(to_mcp_err)?,
        )
    }

    #[tool(
        description = "Run an arbitrary READ-ONLY SQL query (SELECT/WITH only) against the database. Rows are capped at 200 and long cells truncated. Use for anything the curated tools don't cover."
    )]
    async fn raw_sql(
        &self,
        Parameters(p): Parameters<RawSqlParams>,
    ) -> Result<CallToolResult, McpError> {
        json_ok(
            &tools::raw_sql::run(&self.ctx, &p.sql, p.limit)
                .await
                .map_err(to_mcp_err)?,
        )
    }
}

#[tool_handler]
impl ServerHandler for DiagServer {
    fn get_info(&self) -> ServerInfo {
        // ServerInfo is #[non_exhaustive]; build from Default and assign.
        let mut info = ServerInfo::default();
        info.instructions = Some(
            "Read-only production diagnostics for the Livepeer protocol explorer. \
             Call dependency_chain first to localize where the pipeline is stalled, \
             then drill in with indexer_health / pricing_backlog / report_readiness."
                .to_string(),
        );
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }
}

fn json_ok<T: Serialize>(v: &T) -> Result<CallToolResult, McpError> {
    let text = serde_json::to_string_pretty(v)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

fn to_mcp_err(e: anyhow::Error) -> McpError {
    McpError::internal_error(e.to_string(), None)
}
