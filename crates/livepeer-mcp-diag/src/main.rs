//! livepeer-mcp-diag — a read-only MCP server for debugging the Livepeer
//! protocol explorer in production. Exposes curated diagnostic probes (plus a
//! SELECT-only SQL escape hatch) over stdio (local dev) or Streamable HTTP
//! (co-deployed prod container). Read-only is enforced at the DB role/session
//! layer; this binary never writes.

mod adapters;
mod config;
mod context;
mod output;
mod queries;
mod server;
mod tools;
mod transport;

use crate::adapters::db::RoDb;
use crate::adapters::metrics::MetricsClient;
use crate::config::{DiagConfig, Transport};
use crate::context::DiagContext;
use anyhow::{Context, Result};
use clap::Parser;
use livepeer_core::config::Config;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

const SERVICE: &str = "livepeer-mcp-diag";

#[derive(Parser, Debug)]
#[command(name = SERVICE, about = "Read-only production diagnostics MCP server for the Livepeer protocol explorer.")]
pub struct Cli {
    #[arg(long, env = "STATIC_CONFIG", default_value = "config/arbitrum.yaml")]
    pub static_config: PathBuf,

    #[arg(long, env = "ENV_CONFIG", default_value = "config/env/dev.yaml")]
    pub env_config: PathBuf,

    /// Read-only DSN (the `diag_ro` role). Falls back to DATABASE_URL if unset.
    #[arg(long, env = "DIAG_DATABASE_URL")]
    pub database_url: Option<String>,

    #[arg(long, env = "DIAG_DB_MAX_CONNECTIONS", default_value_t = 3)]
    pub db_max_connections: u32,

    /// `stdio` (local dev) or `http` (co-deployed prod).
    #[arg(long, env = "DIAG_TRANSPORT", default_value = "stdio")]
    pub transport: String,

    #[arg(long, env = "DIAG_BIND", default_value = "0.0.0.0:9200")]
    pub bind: String,

    /// Required when transport=http. Bearer token for the /mcp endpoint.
    #[arg(long, env = "DIAG_BEARER_TOKEN")]
    pub bearer_token: Option<String>,

    /// Comma-separated Prometheus /metrics URLs. Only daemon/enricher/api
    /// expose one; rollups are DB-only.
    #[arg(
        long,
        env = "DIAG_METRICS_ENDPOINTS",
        default_value = "http://livepeer-daemon:9107/metrics,http://livepeer-enricher:9112/metrics,http://livepeer-api:8080/metrics"
    )]
    pub metrics_endpoints: String,

    /// Docker Engine API endpoint (the read-only socket proxy in prod).
    #[arg(long, env = "DOCKER_HOST", default_value = "tcp://docker-proxy:2375")]
    pub docker_host: String,

    /// Host-header allowlist for the /mcp endpoint (rmcp DNS-rebinding guard).
    /// Behind a reverse proxy / Cloudflare tunnel, set this to your public
    /// hostname (comma-separated for several) or `*` to disable the check.
    /// Unset keeps rmcp's localhost-only default.
    #[arg(long, env = "DIAG_ALLOWED_HOSTS")]
    pub allowed_hosts: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // stdio transport owns stdout for the JSON-RPC stream — logs must go to
    // stderr or they corrupt the protocol. http transport logs to stdout like
    // the other services.
    let to_stderr = cli.transport.eq_ignore_ascii_case("stdio");
    init_tracing(to_stderr);

    let core = Config::load(&cli.static_config, &cli.env_config).context("loading core config")?;
    let cfg = DiagConfig::resolve(&cli, &core)?;

    if cfg.using_fallback_db(&cli) {
        warn!(
            "DIAG_DATABASE_URL not set — using DATABASE_URL. For real read-only safety, \
             point this at a dedicated diag_ro role (SELECT-only, default_transaction_read_only)."
        );
    }

    let db = RoDb::connect(&cfg).await?;
    let metrics = MetricsClient::new(cfg.metrics_endpoints.clone());
    let docker = adapters::docker::DockerClient::new(&cfg.docker_host)?;
    let ctx = Arc::new(DiagContext {
        db,
        metrics,
        docker,
        cfg: cfg.clone(),
    });

    info!(
        transport = ?cfg.transport,
        endpoints = cfg.metrics_endpoints.len(),
        version = %cfg.valuation_version,
        "diagnostics server ready"
    );

    match cfg.transport {
        Transport::Stdio => transport::serve_stdio(ctx).await?,
        Transport::Http => {
            let bearer = cfg
                .bearer_token
                .clone()
                .expect("bearer token presence validated in DiagConfig::resolve");
            transport::serve_http(ctx, &cfg.bind, bearer, cfg.allowed_hosts.clone()).await?
        }
    }
    Ok(())
}

fn init_tracing(to_stderr: bool) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let layer = fmt::layer().json();
    if to_stderr {
        tracing_subscriber::registry()
            .with(filter)
            .with(layer.with_writer(std::io::stderr))
            .init();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(layer)
            .init();
    }
}
