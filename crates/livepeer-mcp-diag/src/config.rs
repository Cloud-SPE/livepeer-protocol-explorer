//! Diagnostic-server configuration. Wraps `livepeer_core::config::Config`
//! (for the DB URL + pool sizing + current valuation version) and layers the
//! diag-only settings on top from CLI flags / env vars.
//!
//! Read-only enforcement is a DB-role concern (`diag_ro`), not a config
//! concern — see `adapters::db`. This module only decides *where* to connect
//! and *what* to scrape.

use crate::Cli;
use anyhow::{Context, Result};
use livepeer_core::config::Config;

/// Which MCP transport the server exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// stdio JSON-RPC — for local dev (the client spawns the process).
    Stdio,
    /// Streamable HTTP — for the co-deployed prod container.
    Http,
}

impl std::str::FromStr for Transport {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "stdio" => Ok(Transport::Stdio),
            "http" => Ok(Transport::Http),
            other => anyhow::bail!("unknown transport '{other}' (expected 'stdio' or 'http')"),
        }
    }
}

/// Staleness / backlog thresholds used by `indexer_health` and
/// `dependency_chain` to decide when a stage counts as "behind". Seconds and
/// counts; tuned to each loop's cadence in `livepeer-daemon` with slack.
#[derive(Debug, Clone)]
pub struct Thresholds {
    /// Indexer checkpoint considered stalled if `now() - updated_at` exceeds
    /// this. Indexer tick is 12s; allow generous slack for a slow chunk.
    pub indexer_stale_secs: i64,
    /// Rollup checkpoint considered stalled if `now() - updated_at` exceeds
    /// this. Rollup cadence is 300s.
    pub rollup_stale_secs: i64,
    /// Finality considered lagging if `now() - max(finalized_at)` exceeds this.
    pub finality_lag_secs: i64,
    /// Pricing considered backlogged if this many valuable+finalized events
    /// have no valuation row for the current version.
    pub pricing_backlog: i64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            indexer_stale_secs: 120,
            rollup_stale_secs: 1_800,
            finality_lag_secs: 3_600,
            pricing_backlog: 500,
        }
    }
}

/// Fully-resolved diagnostic configuration.
#[derive(Debug, Clone)]
pub struct DiagConfig {
    pub database_url: String,
    pub db_max_connections: u32,
    pub transport: Transport,
    pub bind: String,
    pub bearer_token: Option<String>,
    /// Prometheus `/metrics` URLs to scrape. Only three prod containers expose
    /// one (daemon/enricher/api); rollups have none — see `report_readiness`.
    pub metrics_endpoints: Vec<String>,
    /// Docker Engine API endpoint — the read-only socket proxy in prod.
    pub docker_host: String,
    /// rmcp Host-header allowlist for `/mcp` (None = localhost-only default).
    pub allowed_hosts: Option<String>,
    /// Current pricing version, from static config; used by the pricing/report
    /// probes to key the "unpriced" backlog on the version actually in use.
    pub valuation_version: String,
    pub thresholds: Thresholds,
}

impl DiagConfig {
    /// Resolve from parsed CLI + loaded core config. `DIAG_DATABASE_URL` takes
    /// precedence over the core `DATABASE_URL`; falling back to the workers'
    /// read-write URL is allowed but discouraged (log a warning at startup).
    pub fn resolve(cli: &Cli, core: &Config) -> Result<Self> {
        let transport: Transport = cli.transport.parse()?;

        let database_url = match &cli.database_url {
            Some(url) => url.clone(),
            None => core
                .database_url()
                .context("resolving DATABASE_URL (set DIAG_DATABASE_URL to the diag_ro role)")?,
        };

        if transport == Transport::Http && cli.bearer_token.is_none() {
            anyhow::bail!(
                "DIAG_BEARER_TOKEN is required when --transport=http (refusing to serve an unauthenticated MCP endpoint)"
            );
        }

        let metrics_endpoints = cli
            .metrics_endpoints
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        Ok(Self {
            database_url,
            db_max_connections: cli.db_max_connections,
            transport,
            bind: cli.bind.clone(),
            bearer_token: cli.bearer_token.clone(),
            metrics_endpoints,
            docker_host: cli.docker_host.clone(),
            allowed_hosts: cli.allowed_hosts.clone(),
            valuation_version: core.static_.pricing.default_valuation_version.clone(),
            thresholds: Thresholds::default(),
        })
    }

    /// True when the diag binary is pointed at the fallback RW URL rather than
    /// a dedicated `diag_ro` DSN. Startup logs a warning in that case.
    pub fn using_fallback_db(&self, cli: &Cli) -> bool {
        cli.database_url.is_none()
    }
}
