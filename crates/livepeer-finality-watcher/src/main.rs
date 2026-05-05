//! Finality watcher daemon. SPEC §9.1.
//!
//! v1.5 implementation (timestamp-heuristic):
//!   - Each iteration reads L1's `latest` and `finalized` block timestamps.
//!   - Marks L2 events as `l1_posted` once their L2 block_timestamp is older
//!     than `latest_l1_ts − POSTING_LAG_SECS` (≈10 min, the canonical Arbitrum
//!     batch-posting cadence per SPEC §9.1).
//!   - Marks L2 events as `finalized` once their L2 block_timestamp is older
//!     than `finalized_l1_ts − FINALITY_SAFETY_MARGIN_SECS` (≈1 min margin past
//!     L1 finality).
//!
//! Why heuristic vs the SPEC-true SequencerBatchDelivered walk:
//!   - This is correct enough for v1 (the staleness lower-bounds finality, so
//!     we never mark something finalized prematurely if L1 is healthy).
//!   - Real batch tracking is TD-008 — it requires SequencerInbox log decoding
//!     and the (batch → L2 block range) mapping, plus archive L1 depth for
//!     back-fill from Livepeer's Arbitrum genesis.
//!   - The valuator only consumes `finality = 'finalized'` rows (SPEC §9.1),
//!     so this watcher is what gates the production valuation flow.

use anyhow::{Context, Result};
use clap::Parser;
use livepeer_core::{config::Config, db, rpc::Provider, tracing_init};
use livepeer_finality_watcher::runner;
use std::path::PathBuf;
use std::time::Duration;
use tracing::{error, info};

const SERVICE: &str = "livepeer-finality-watcher";
const POSTING_LAG_SECS: i64 = 600; // ~10 min — typical Arbitrum batch-posting cadence
const FINALITY_SAFETY_MARGIN_SECS: i64 = 60; // 1 min past L1 finalized timestamp
const CADENCE_SECS: u64 = 60; // L1 advances slowly; no need to poll fast

#[derive(Parser, Debug)]
#[command(name = SERVICE, about = "Drives raw_protocol_events.finality through tentative → l1_posted → finalized.")]
struct Cli {
    #[arg(long, env = "STATIC_CONFIG", default_value = "config/arbitrum.yaml")]
    static_config: PathBuf,
    #[arg(long, env = "ENV_CONFIG", default_value = "config/env/dev.yaml")]
    env_config: PathBuf,
    /// Run a single iteration and exit (testing aid).
    #[arg(long, default_value_t = false)]
    once: bool,
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

    let l1_url = std::env::var("L1_RPC_URL").context("L1_RPC_URL not set in environment")?;
    let l1 = Provider::new("l1-eth", l1_url)?;
    info!(
        service = SERVICE,
        cadence_secs = CADENCE_SECS,
        posting_lag_secs = POSTING_LAG_SECS,
        finality_safety_margin_secs = FINALITY_SAFETY_MARGIN_SECS,
        "starting"
    );

    if cli.once {
        let summary = runner::run_once(&pg, &l1).await?;
        info!(?summary, "iteration");
        return Ok(());
    }
    loop {
        match runner::run_once(&pg, &l1).await {
            Ok(summary) => info!(?summary, "iteration"),
            Err(e) => error!(error = %e, "iteration failed; will retry on next tick"),
        }
        tokio::time::sleep(Duration::from_secs(CADENCE_SECS)).await;
    }
}
