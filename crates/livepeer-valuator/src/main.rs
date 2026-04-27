mod onchain;
mod persist;
mod seed;
mod tick_math;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use livepeer_core::{config::Config, db, rpc::Provider, tracing_init};
use std::path::PathBuf;
use tracing::info;

const SERVICE: &str = "livepeer-valuator";

#[derive(Parser, Debug)]
#[command(name = SERVICE, about = "Prices finalized events into event_valuations under a named valuation_version.")]
struct Cli {
    #[arg(long, env = "STATIC_CONFIG", default_value = "config/arbitrum.yaml", global = true)]
    static_config: PathBuf,

    #[arg(long, env = "ENV_CONFIG", default_value = "config/env/dev.yaml", global = true)]
    env_config: PathBuf,

    /// Override the configured default valuation version.
    #[arg(long, global = true)]
    version: Option<String>,

    /// Allow tentative events. SPEC §9.1 requires finalized in production; without
    /// a finality watcher all events are tentative, so this is the dev override.
    #[arg(long, default_value_t = false, global = true)]
    include_tentative: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// (S8.1) Seed-hit pass — values events whose `(tx_hash, asset)` match a seed row.
    BackfillFromSeed,
    /// (S8.2.a) On-chain pass for ETH-valued events — Chainlink ETH/USD at event block.
    BackfillEthOnchain,
    /// (S8.2.b) On-chain pass for LPT-valued events — Uniswap V3 TWAP × Chainlink,
    /// with degraded-spot fallback when pool cardinality < 144 (Q-OD-9).
    BackfillLptOnchain,
    /// Run seed → ETH on-chain → LPT on-chain in sequence.
    BackfillAll,
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
    let valuation_version = cli
        .version
        .clone()
        .unwrap_or_else(|| cfg.static_.pricing.default_valuation_version.clone());
    info!(service = SERVICE, valuation_version, "config + db ready");

    match cli.command {
        Command::BackfillFromSeed => {
            let s = seed::run_seed_pass(&pg, &valuation_version, cli.include_tentative).await?;
            info!(
                events_considered = s.events_considered,
                seed_hits = s.seed_hits,
                seed_misses = s.seed_misses,
                priced_this_run = s.priced_this_run,
                multi_asset_skipped = s.multi_asset_skipped,
                "seed pass summary"
            );
        }
        Command::BackfillEthOnchain => {
            let archive = Provider::new(
                "chainstack",
                cfg.archive_rpc_url().context("CHAINSTACK_RPC_URL")?,
            )?;
            let s = onchain::run_onchain_pass_eth(&pg, &archive, &cfg, &valuation_version, cli.include_tentative).await?;
            info!(
                events_considered = s.events_considered,
                priced = s.priced,
                failed_sequencer_outage = s.failed_sequencer_outage,
                failed_missing_oracle = s.failed_missing_oracle,
                other_skipped = s.other_skipped,
                "on-chain ETH pass summary"
            );
        }
        Command::BackfillLptOnchain => {
            let archive = Provider::new(
                "chainstack",
                cfg.archive_rpc_url().context("CHAINSTACK_RPC_URL")?,
            )?;
            let s = onchain::run_onchain_pass_lpt(&pg, &archive, &cfg, &valuation_version, cli.include_tentative).await?;
            info!(
                events_considered = s.events_considered,
                priced_twap = s.priced_twap,
                priced_degraded = s.priced_degraded,
                failed_sequencer_outage = s.failed_sequencer_outage,
                failed_missing_oracle = s.failed_missing_oracle,
                failed_missing_pool = s.failed_missing_pool,
                other_skipped = s.other_skipped,
                "on-chain LPT pass summary"
            );
        }
        Command::BackfillAll => {
            let s = seed::run_seed_pass(&pg, &valuation_version, cli.include_tentative).await?;
            info!(
                events_considered = s.events_considered,
                priced_this_run = s.priced_this_run,
                seed_misses = s.seed_misses,
                multi_asset_skipped = s.multi_asset_skipped,
                "seed pass summary"
            );
            let archive = Provider::new(
                "chainstack",
                cfg.archive_rpc_url().context("CHAINSTACK_RPC_URL")?,
            )?;
            let o = onchain::run_onchain_pass_eth(&pg, &archive, &cfg, &valuation_version, cli.include_tentative).await?;
            info!(
                events_considered = o.events_considered,
                priced = o.priced,
                failed_sequencer_outage = o.failed_sequencer_outage,
                failed_missing_oracle = o.failed_missing_oracle,
                other_skipped = o.other_skipped,
                "on-chain ETH pass summary"
            );
            let l = onchain::run_onchain_pass_lpt(&pg, &archive, &cfg, &valuation_version, cli.include_tentative).await?;
            info!(
                events_considered = l.events_considered,
                priced_twap = l.priced_twap,
                priced_degraded = l.priced_degraded,
                failed_sequencer_outage = l.failed_sequencer_outage,
                failed_missing_oracle = l.failed_missing_oracle,
                failed_missing_pool = l.failed_missing_pool,
                other_skipped = l.other_skipped,
                "on-chain LPT pass summary"
            );
        }
    }
    Ok(())
}
