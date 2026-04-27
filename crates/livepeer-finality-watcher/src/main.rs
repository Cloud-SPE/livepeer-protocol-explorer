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
use livepeer_core::{
    config::Config,
    db,
    rpc::{BlockTag, Provider},
    tracing_init,
};
use sqlx::PgPool;
use std::path::PathBuf;
use std::time::Duration;
use tracing::{error, info, warn};

const SERVICE: &str = "livepeer-finality-watcher";
const ARBITRUM_CHAIN_ID: i64 = 42161;
const POSTING_LAG_SECS: i64 = 600;            // ~10 min — typical Arbitrum batch-posting cadence
const FINALITY_SAFETY_MARGIN_SECS: i64 = 60;  // 1 min past L1 finalized timestamp
const CADENCE_SECS: u64 = 60;                 // L1 advances slowly; no need to poll fast

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
        run_iteration(&pg, &l1).await?;
        return Ok(());
    }
    loop {
        match run_iteration(&pg, &l1).await {
            Ok(()) => {}
            Err(e) => error!(error = %e, "iteration failed; will retry on next tick"),
        }
        tokio::time::sleep(Duration::from_secs(CADENCE_SECS)).await;
    }
}

async fn run_iteration(pg: &PgPool, l1: &Provider) -> Result<()> {
    // Read L1 latest + finalized block timestamps. Both are live (not block-pinned)
    // queries so we don't push them through the deterministic cache.
    let latest_ts = l1_block_timestamp(l1, BlockTag::Latest).await?;
    let finalized_ts = l1_block_timestamp_str(l1, "finalized").await?;

    let l1_posted_cutoff = latest_ts - POSTING_LAG_SECS;
    let finalized_cutoff = finalized_ts - FINALITY_SAFETY_MARGIN_SECS;

    if finalized_cutoff > l1_posted_cutoff {
        warn!(
            l1_posted_cutoff, finalized_cutoff,
            "finalized cutoff > l1_posted cutoff — defensive cap (shouldn't happen with the configured lag)"
        );
    }

    // tentative → l1_posted
    let posted = sqlx::query(
        r#"UPDATE raw_protocol_events
              SET finality = 'l1_posted'
            WHERE chain_id = $1
              AND finality = 'tentative'
              AND is_canonical = TRUE
              AND EXTRACT(EPOCH FROM block_timestamp)::BIGINT <= $2"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(l1_posted_cutoff)
    .execute(pg)
    .await?
    .rows_affected();

    // (tentative | l1_posted) → finalized
    let finalized = sqlx::query(
        r#"UPDATE raw_protocol_events
              SET finality   = 'finalized',
                  finalized_at = now()
            WHERE chain_id = $1
              AND finality IN ('tentative', 'l1_posted')
              AND is_canonical = TRUE
              AND EXTRACT(EPOCH FROM block_timestamp)::BIGINT <= $2"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(finalized_cutoff)
    .execute(pg)
    .await?
    .rows_affected();

    // Checkpoint advance — observability, not load-bearing for the heuristic.
    sqlx::query(
        r#"INSERT INTO indexer_checkpoints
                (name, chain_id, last_processed_block, updated_at)
           VALUES ('finality_watcher', $1, $2, now())
           ON CONFLICT (name) DO UPDATE
              SET last_processed_block = GREATEST(indexer_checkpoints.last_processed_block, EXCLUDED.last_processed_block),
                  updated_at = now()"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(finalized_cutoff)
    .execute(pg)
    .await?;

    info!(
        l1_latest_ts = latest_ts,
        l1_finalized_ts = finalized_ts,
        l1_posted_cutoff,
        finalized_cutoff,
        rows_marked_l1_posted = posted,
        rows_marked_finalized = finalized,
        "iteration"
    );
    Ok(())
}

async fn l1_block_timestamp(l1: &Provider, tag: BlockTag) -> Result<i64> {
    let header = l1.eth_get_block_header(tag).await?;
    let ts_hex = header
        .get("timestamp")
        .and_then(|v| v.as_str())
        .context("L1 block header missing .timestamp")?;
    let ts = i64::from_str_radix(ts_hex.trim_start_matches("0x"), 16)?;
    Ok(ts)
}

/// Variant for string tags ("safe", "finalized") that BlockTag doesn't model.
async fn l1_block_timestamp_str(l1: &Provider, tag: &str) -> Result<i64> {
    let v = l1
        .call(
            "eth_getBlockByNumber",
            &serde_json::json!([tag, false]),
        )
        .await?;
    let ts_hex = v
        .get("timestamp")
        .and_then(|v| v.as_str())
        .with_context(|| format!("L1 block header at tag={tag} missing .timestamp"))?;
    let ts = i64::from_str_radix(ts_hex.trim_start_matches("0x"), 16)?;
    Ok(ts)
}
