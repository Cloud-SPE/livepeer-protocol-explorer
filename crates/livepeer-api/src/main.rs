mod cursor;
mod error;
mod metrics;
mod openapi;
mod routes;
mod state;
mod abi;

use anyhow::{Context, Result};
use axum::{routing::get, Router};
use clap::Parser;
use livepeer_core::{config::Config, db, tracing_init};
use state::AppState;
use std::{net::SocketAddr, path::PathBuf};
use tower_http::trace::TraceLayer;
use tracing::info;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

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
        metrics: std::sync::Arc::new(metrics::Metrics::new()),
    };

    let router = Router::new()
        // Operational
        .route("/health", get(routes::operational::health))
        .route("/metrics", get(routes::operational::metrics))
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
        .route("/prices/{asset}/{quote}/range", get(routes::prices::range))
        // Stake
        .route("/stake/{delegator}/block/{block}", get(routes::stake::at_block))
        .route("/stake/{delegator}/range", get(routes::stake::range))
        .route("/transcoders/{transcoder}/delegators/block/{block}", get(routes::stake::delegators_at_block))
        // Transcoders
        .route("/transcoders/{transcoder}/params/latest", get(routes::transcoders::latest))
        .route("/transcoders/{transcoder}/params/block/{block}", get(routes::transcoders::at_block))
        .route("/transcoders/{transcoder}/params/history", get(routes::transcoders::history))
        .route("/transcoders/{transcoder}/lifecycle/latest", get(routes::transcoders::lifecycle_latest))
        .route("/transcoders/{transcoder}/lifecycle/block/{block}", get(routes::transcoders::lifecycle_at_block))
        .route("/transcoders/{transcoder}/lifecycle/history", get(routes::transcoders::lifecycle_history))
        .route("/transcoders/{transcoder}/profile/block/{block}", get(routes::transcoders::profile_at_block))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .merge(SwaggerUi::new("/docs").url("/openapi.json", openapi::ApiDoc::openapi()));

    let addr: SocketAddr = cli.bind.parse().context("parsing bind address")?;
    info!(service = SERVICE, %addr, "listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;
    Ok(())
}
