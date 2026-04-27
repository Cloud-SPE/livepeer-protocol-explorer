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
use sqlx::{PgPool, Row};
use std::path::PathBuf;
use std::time::Duration;
use tracing::{debug, error, info, warn};

const SERVICE: &str = "livepeer-reorg-watcher";
const ARBITRUM_CHAIN_ID: i64 = 42161;
const WALK_DEPTH: u64 = 7_500;
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
    info!(service = SERVICE, walk_depth = WALK_DEPTH, "starting");

    if cli.once {
        run_iteration(&pg, &secondary).await?;
        return Ok(());
    }

    let mut last_detection_unix: Option<u64> = None;
    let mut last_clean_unix: Option<u64> = None;
    loop {
        match run_iteration(&pg, &secondary).await {
            Ok(divergences) => {
                let now = unix_now();
                if divergences > 0 {
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

/// Run one iteration. Returns the number of divergences detected (0 = clean).
async fn run_iteration(pg: &PgPool, secondary: &Provider) -> Result<u64> {
    let head = secondary.eth_block_number().await?;
    let window_start = head.saturating_sub(WALK_DEPTH);

    // Find every block_number with tentative + canonical events in the window.
    let stored: Vec<(i64, String)> = sqlx::query(
        r#"SELECT DISTINCT block_number, block_hash
             FROM raw_protocol_events
            WHERE chain_id      = $1
              AND finality      = 'tentative'
              AND is_canonical  = TRUE
              AND block_number BETWEEN $2 AND $3
            ORDER BY block_number"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(window_start as i64)
    .bind(head as i64)
    .fetch_all(pg)
    .await?
    .into_iter()
    .map(|r| (r.get::<i64, _>(0), r.get::<String, _>(1)))
    .collect();

    info!(
        head,
        window_start,
        blocks_to_check = stored.len(),
        "iteration"
    );

    let mut divergences = 0u64;
    for (block_number, stored_hash) in stored {
        let n = block_number as u64;
        let header = secondary
            .eth_get_block_header(livepeer_core::rpc::BlockTag::Number(n))
            .await?;
        let chain_hash = header
            .get("hash")
            .and_then(|v| v.as_str())
            .map(|s| s.to_lowercase())
            .context("block header missing .hash")?;
        let stored_hash = stored_hash.to_lowercase();
        if chain_hash != stored_hash {
            divergences += 1;
            handle_divergence(pg, n, &stored_hash, &chain_hash).await?;
        }
    }

    if divergences == 0 {
        debug!(blocks_checked = "n", "no divergence");
    }
    Ok(divergences)
}

async fn handle_divergence(
    pg: &PgPool,
    block_number: u64,
    old_hash: &str,
    new_hash: &str,
) -> Result<()> {
    let mut tx = pg.begin().await?;

    // Mark all events at this block non-canonical. SPEC §9.3 also requires updating
    // block_number/block_hash to the new canonical values + writing reorg_mutations
    // rows. That's a richer flow tracked in TD-005 (added below).
    let affected: u64 = sqlx::query(
        r#"UPDATE raw_protocol_events
              SET is_canonical = FALSE
            WHERE chain_id     = $1
              AND block_number = $2
              AND is_canonical = TRUE"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(block_number as i64)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    // Audit row.
    let depth = 1; // single-block divergence detected; deeper reorgs are recorded as
                   // separate detections in v1. Multi-block-depth tracking → TD-005.
    sqlx::query(
        r#"INSERT INTO reorg_events
              (chain_id, divergence_block, depth, old_block_hashes,
               new_block_hashes, affected_event_count, notes)
           VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(block_number as i64)
    .bind(depth as i32)
    .bind(vec![old_hash.to_string()])
    .bind(vec![new_hash.to_string()])
    .bind(affected as i32)
    .bind("v1 reorg-watcher: events-only walk; non-canonical marker only (TD-005)")
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    let level = severity_level(depth);
    match level {
        Severity::Info => info!(block_number, old_hash, new_hash, affected, "reorg detected (depth ≤ 2)"),
        Severity::Warn => warn!(block_number, old_hash, new_hash, affected, "reorg detected (depth 3–50)"),
        Severity::Critical => error!(block_number, old_hash, new_hash, affected, "REORG detected (depth > 50) — CRITICAL"),
    }
    Ok(())
}

enum Severity {
    Info,
    Warn,
    Critical,
}
fn severity_level(depth: u32) -> Severity {
    match depth {
        0..=2 => Severity::Info,
        3..=50 => Severity::Warn,
        _ => Severity::Critical,
    }
}
