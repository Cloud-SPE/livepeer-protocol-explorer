use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use livepeer_core::{config::Config, db, rpc::Provider, tracing_init};
use livepeer_enricher::{http, metrics::Metrics, runner};
use std::sync::Arc;
use std::{path::PathBuf, time::Duration};
use tracing::{error, info};

const SERVICE: &str = "livepeer-enricher";
const DEFAULT_CADENCE_SECS: u64 = 300;

#[derive(Parser, Debug)]
#[command(
    name = SERVICE,
    about = "Populates external ENS projection tables for orchestrator and gateway profiles."
)]
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
        env = "ENRICHER_METRICS_BIND",
        default_value = "0.0.0.0:9112",
        global = true
    )]
    metrics_bind: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// One-shot sweep of unresolved or stale ENS rows.
    Backfill {
        #[arg(long, default_value_t = runner::DEFAULT_BATCH_LIMIT)]
        batch_limit: i64,
    },
    /// Periodic sweep loop. Stays isolated from the daemon supervisor by design.
    Follow {
        #[arg(long, default_value_t = runner::DEFAULT_BATCH_LIMIT)]
        batch_limit: i64,
        #[arg(long, default_value_t = DEFAULT_CADENCE_SECS)]
        cadence_secs: u64,
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
    let l1_url = cfg
        .env
        .l1
        .as_ref()
        .context("L1 RPC config missing for livepeer-enricher")?
        .url_env
        .clone();
    let l1 = Provider::new(
        "l1-eth",
        std::env::var(&l1_url).with_context(|| format!("{l1_url} not set in environment"))?,
    )?;
    let metrics = Arc::new(Metrics::new());
    let metrics_bind = cli.metrics_bind.clone();
    let metrics_task = tokio::spawn({
        let metrics = metrics.clone();
        async move {
            if let Err(e) = http::serve(&metrics_bind, metrics).await {
                error!(error = %e, bind = %metrics_bind, "enricher metrics server exited");
            }
        }
    });
    info!(service = SERVICE, "config + db + l1 ready");
    info!(bind = %cli.metrics_bind, "enricher metrics server starting");

    match cli.command {
        Command::Backfill { batch_limit } => {
            let summary = runner::run_once(
                &pg,
                &l1,
                cfg.static_.chain.chain_id as i64,
                batch_limit,
                &metrics,
            )
            .await?;
            info!(
                orchestrators_seen = summary.orchestrators_seen,
                orchestrators_updated = summary.orchestrators_updated,
                gateways_seen = summary.gateways_seen,
                gateways_updated = summary.gateways_updated,
                named_rows = summary.named_rows,
                avatar_rows = summary.avatar_rows,
                failures = summary.failures,
                "ens enricher summary"
            );
        }
        Command::Follow {
            batch_limit,
            cadence_secs,
        } => loop {
            match runner::watch_once(&pg, &l1, cfg.static_.chain.chain_id as i64, &metrics).await {
                Ok(summary) => {
                    if summary.logs_seen > 0 || summary.addresses_refreshed > 0 {
                        info!(
                            latest_l1_block = summary.latest_l1_block,
                            logs_seen = summary.logs_seen,
                            addresses_refreshed = summary.addresses_refreshed,
                            "ens watcher summary"
                        );
                    }
                }
                Err(e) => {
                    error!(error = %e, "ens watcher iteration failed; will retry on next tick")
                }
            }
            match runner::run_once(
                &pg,
                &l1,
                cfg.static_.chain.chain_id as i64,
                batch_limit,
                &metrics,
            )
            .await
            {
                Ok(summary) => info!(
                    orchestrators_seen = summary.orchestrators_seen,
                    orchestrators_updated = summary.orchestrators_updated,
                    gateways_seen = summary.gateways_seen,
                    gateways_updated = summary.gateways_updated,
                    named_rows = summary.named_rows,
                    avatar_rows = summary.avatar_rows,
                    failures = summary.failures,
                    "ens enricher summary"
                ),
                Err(e) => {
                    error!(error = %e, "ens enricher iteration failed; will retry on next tick")
                }
            }
            tokio::time::sleep(Duration::from_secs(cadence_secs)).await;
        },
    }

    metrics_task.abort();
    Ok(())
}
