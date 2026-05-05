//! Reorg watcher daemon. SPEC §9.2.
//!
//! v1 algorithm (events-only walk):
//!   loop {
//!     head = secondary.eth_blockNumber()
//!     window = [head - WALK_DEPTH, head]
//!     for each unique `block_number` in raw_protocol_events ∩ window where
//!         finality = 'tentative' AND is_canonical = TRUE:
//!         chain_hash  = secondary.eth_getBlockByNumber(n).hash
//!         stored_hash = raw_protocol_events.block_hash for n
//!         if chain_hash != stored_hash:
//!             mark all events at block_number = n non-canonical
//!             insert reorg_events row (audit)
//!     sleep(cadence)
//!   }
//!
//! Cheaper than SPEC's "walk every block" because we only check blocks where we
//! have stored events. Same correctness profile in practice — empty blocks are
//! invisible to us regardless of reorgs. Added a TODO/v2 to broaden if needed.
//!
//! Cadence per SPEC §9.2.2: 15s normal, 5s heightened (after a recent reorg),
//! 60s backoff (after 1 hour clean).
//! Severity per SPEC §9.4: 0–2 INFO, 3–50 WARN, >50 CRITICAL.
//!
//! v1 mutation policy: marks rows non-canonical and writes the reorg_events
//! audit row. The richer flow (block_number/block_hash mutation +
//! reorg_mutations rows + indexer reindex) lands in a follow-up slice.

use anyhow::{Context, Result};
use clap::Parser;
use livepeer_core::{config::Config, db, rpc::Provider, tracing_init};
use livepeer_reorg_watcher::runner;
use std::path::PathBuf;
use std::time::Duration;
use tracing::{debug, error, info};

const SERVICE: &str = runner::SERVICE;
const CADENCE_NORMAL_SECS: u64 = 15;
const CADENCE_HEIGHTENED_SECS: u64 = 5;
const CADENCE_BACKOFF_SECS: u64 = 60;
const HEIGHTENED_WINDOW_SECS: u64 = 300; // 5 min after last detection
const BACKOFF_AFTER_CLEAN_SECS: u64 = 3_600; // 1h clean → drop to 60s

#[derive(Parser, Debug)]
#[command(name = SERVICE, about = "Validates parent-hash chain continuity in the tentative window; marks reorg'd rows non-canonical.")]
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
    // Reorg checks use the secondary (non-archive) provider — recent state only,
    // cheaper, and per SPEC §13.2 routing matrix `eth_getBlockByNumber` defaults to local.
    let secondary = Provider::new(
        "liveinfraspe",
        cfg.secondary_rpc_url().context("SECONDARY_RPC_URL")?,
    )?;
    info!(
        service = SERVICE,
        walk_depth = runner::WALK_DEPTH,
        "starting"
    );

    if cli.once {
        runner::run_once(&pg, &secondary).await?;
        return Ok(());
    }

    let mut last_detection_unix: Option<u64> = None;
    let mut last_clean_unix: Option<u64> = None;
    loop {
        match runner::run_once(&pg, &secondary).await {
            Ok(summary) => {
                let now = unix_now();
                if summary.divergences > 0 {
                    last_detection_unix = Some(now);
                    last_clean_unix = None;
                } else if last_clean_unix.is_none() {
                    last_clean_unix = Some(now);
                }
            }
            Err(e) => {
                error!(error = %e, "iteration failed; will retry on next tick");
            }
        }

        let cadence = pick_cadence(last_detection_unix, last_clean_unix);
        debug!(cadence_secs = cadence, "sleeping");
        tokio::time::sleep(Duration::from_secs(cadence)).await;
    }
}

fn pick_cadence(last_detection: Option<u64>, last_clean: Option<u64>) -> u64 {
    let now = unix_now();
    if let Some(t) = last_detection {
        if now.saturating_sub(t) <= HEIGHTENED_WINDOW_SECS {
            return CADENCE_HEIGHTENED_SECS;
        }
    }
    if let Some(t) = last_clean {
        if now.saturating_sub(t) >= BACKOFF_AFTER_CLEAN_SECS {
            return CADENCE_BACKOFF_SECS;
        }
    }
    CADENCE_NORMAL_SECS
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
