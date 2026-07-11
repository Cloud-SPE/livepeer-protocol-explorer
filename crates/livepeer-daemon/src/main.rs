mod http;
mod metrics;
mod rpc_manager;
mod supervisor;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use livepeer_core::{config::Config, db, tracing_init};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info};

const SERVICE: &str = "livepeer-daemon";

#[derive(Parser, Debug)]
#[command(name = SERVICE, about = "Near-head follow-mode daemon for the Livepeer pipeline.")]
struct Cli {
    #[arg(
        long,
        env = "STATIC_CONFIG",
        default_value = "config/arbitrum.yaml",
        global = true
    )]
    static_config: PathBuf,

    #[arg(
        long,
        env = "ENV_CONFIG",
        default_value = "config/env/dev.yaml",
        global = true
    )]
    env_config: PathBuf,

    #[arg(
        long,
        env = "DAEMON_METRICS_BIND",
        default_value = "0.0.0.0:9107",
        global = true
    )]
    metrics_bind: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Follow {
        #[arg(long, default_value_t = 50_000)]
        max_start_lag_blocks: u64,
        #[arg(long)]
        version: Option<String>,
        #[arg(long, default_value_t = false)]
        include_tentative: bool,
        /// How often to REFRESH the profile materialized views, in seconds.
        /// Each refresh takes seconds on a large DB and contends with the
        /// valuator — raise this (e.g. 300) to cut DB load at the cost of
        /// profile-page freshness.
        #[arg(long, env = "DAEMON_MATVIEW_REFRESH_SECS", default_value_t = 30)]
        matview_refresh_secs: u64,
    },
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
    let metrics = Arc::new(metrics::Metrics::new());
    let metrics_bind = cli.metrics_bind.clone();
    let metrics_task = tokio::spawn({
        let metrics = metrics.clone();
        async move {
            if let Err(e) = http::serve(&metrics_bind, metrics).await {
                error!(error = %e, bind = %metrics_bind, "daemon metrics server exited");
                return Err(e);
            }
            Ok::<(), anyhow::Error>(())
        }
    });
    info!(bind = %cli.metrics_bind, "daemon metrics server starting");

    match cli.command {
        Command::Follow {
            max_start_lag_blocks,
            version,
            include_tentative,
            matview_refresh_secs,
        } => {
            let version =
                version.unwrap_or_else(|| cfg.static_.pricing.default_valuation_version.clone());
            let rpc = rpc_manager::RpcManager::new(&cfg).await?;
            supervisor::run_follow(
                &pg,
                &cfg,
                rpc,
                metrics.clone(),
                supervisor::FollowConfig {
                    max_start_lag_blocks,
                    valuation_version: version,
                    include_tentative,
                    matview_refresh_secs,
                },
            )
            .await?;
        }
    }
    metrics_task.abort();
    Ok(())
}
