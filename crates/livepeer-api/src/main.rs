mod cursor;
mod error;
mod routes;
mod state;

use anyhow::{Context, Result};
use axum::{routing::get, Router};
use clap::Parser;
use livepeer_core::{config::Config, db, tracing_init};
use state::AppState;
use std::{net::SocketAddr, path::PathBuf};
use tower_http::trace::TraceLayer;
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

    let state = AppState {
        pg,
        default_version: cfg.static_.pricing.default_valuation_version.clone(),
        chain_id: cfg.static_.chain.chain_id as i64,
    };

    let router = Router::new()
        // Operational
        .route("/health", get(routes::operational::health))
        .route("/backfills/status", get(routes::operational::backfill_status))
        // Events
        .route("/events", get(routes::events::list))
        .route("/events/{id}", get(routes::events::get_one))
        .route("/events/{id}/valuation", get(routes::valuations::for_event))
        // Valuations
        .route("/valuations", get(routes::valuations::list))
        // Aggregations
        .route("/aggregations/events", get(routes::aggregations::events))
        // Governance
        .route("/governance/proposals", get(routes::governance::list))
        .route("/governance/proposals/{proposal_id}", get(routes::governance::get_one))
        // Prices
        .route("/prices/{asset}/{quote}/block/{block}", get(routes::prices::at_block))
        .route("/prices/{asset}/{quote}/latest", get(routes::prices::latest))
        .with_state(state)
        .layer(TraceLayer::new_for_http());

    let addr: SocketAddr = cli.bind.parse().context("parsing bind address")?;
    info!(service = SERVICE, %addr, "listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;
    Ok(())
}
