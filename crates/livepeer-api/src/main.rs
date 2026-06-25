use anyhow::{Context, Result};
use clap::Parser;
use livepeer_api::{build_router, metrics, state::AppState};
use livepeer_core::{config::Config, db, rpc::Provider, tracing_init};
use std::{net::SocketAddr, path::PathBuf};
use tracing::info;

const SERVICE: &str = "livepeer-api";

#[derive(Parser, Debug)]
#[command(name = SERVICE, about = "HTTP API exposing events, valuations, prices, and stake snapshots.")]
struct Cli {
    #[arg(long, env = "STATIC_CONFIG", default_value = "config/arbitrum.yaml")]
    static_config: PathBuf,
    #[arg(long, env = "ENV_CONFIG", default_value = "config/env/dev.yaml")]
    env_config: PathBuf,
    #[arg(long, env = "API_BIND", default_value = "0.0.0.0:8080")]
    bind: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_init::init("info");
    let cli = Cli::parse();
    let cfg = Config::load(&cli.static_config, &cli.env_config).context("loading config")?;
    let pg = db::connect(
        &cfg.database_url().context("DATABASE_URL")?,
        cfg.env.postgres.pool_max_connections,
    )
    .await
    .context("connecting to Postgres")?;
    let archive = Provider::new(
        "archive",
        cfg.archive_rpc_url().context("CHAINSTACK_RPC_URL")?,
    )
    .context("building archive provider")?;

    let state = AppState {
        pg,
        default_version: cfg.static_.pricing.default_valuation_version.clone(),
        chain_id: cfg.static_.chain.chain_id as i64,
        ticket_broker_address: cfg.static_.contracts.ticket_broker.to_lowercase(),
        archive,
        metrics: std::sync::Arc::new(metrics::Metrics::new()),
        avatar_dir: std::env::var_os("AVATAR_STORE_DIR").map(PathBuf::from),
    };

    let router = build_router(state);

    let addr: SocketAddr = cli.bind.parse().context("parsing bind address")?;
    info!(service = SERVICE, %addr, "listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;
    Ok(())
}
