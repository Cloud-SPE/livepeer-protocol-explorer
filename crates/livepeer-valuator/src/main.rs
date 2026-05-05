use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use livepeer_core::{config::Config, db, rpc::Provider, tracing_init};
use livepeer_valuator::runner;
use std::path::PathBuf;
use tracing::info;

const SERVICE: &str = "livepeer-valuator";

#[derive(Parser, Debug)]
#[command(name = SERVICE, about = "Prices finalized events into event_valuations under a named valuation_version.")]
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
    /// (S8.3) Multi-asset pass — splits each EarningsClaimed into LPT (rewards) + ETH (fees).
    BackfillMultiAsset,
    /// Run seed → ETH on-chain → LPT on-chain → multi-asset in sequence.
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
            let s = runner::run_seed(&pg, &valuation_version, cli.include_tentative).await?;
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
            let s = runner::run_eth(
                &pg,
                &archive,
                &cfg,
                &valuation_version,
                cli.include_tentative,
            )
            .await?;
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
            let s = runner::run_lpt(
                &pg,
                &archive,
                &cfg,
                &valuation_version,
                cli.include_tentative,
            )
            .await?;
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
        Command::BackfillMultiAsset => {
            let archive = Provider::new(
                "chainstack",
                cfg.archive_rpc_url().context("CHAINSTACK_RPC_URL")?,
            )?;
            let s = runner::run_multi_asset(
                &pg,
                &archive,
                &cfg,
                &valuation_version,
                cli.include_tentative,
            )
            .await?;
            info!(
                events_considered = s.events_considered,
                lpt_rows_priced = s.lpt_rows_priced,
                eth_rows_priced = s.eth_rows_priced,
                lpt_zero_amount_rows = s.lpt_zero_amount_rows,
                eth_zero_amount_rows = s.eth_zero_amount_rows,
                failures = s.failures,
                "multi-asset pass summary"
            );
        }
        Command::BackfillAll => {
            let archive = Provider::new(
                "chainstack",
                cfg.archive_rpc_url().context("CHAINSTACK_RPC_URL")?,
            )?;
            let all = runner::run_all(
                &pg,
                &archive,
                &cfg,
                &valuation_version,
                cli.include_tentative,
            )
            .await?;
            info!(
                events_considered = all.seed.events_considered,
                priced_this_run = all.seed.priced_this_run,
                seed_misses = all.seed.seed_misses,
                multi_asset_skipped = all.seed.multi_asset_skipped,
                "seed pass summary"
            );
            info!(
                events_considered = all.eth.events_considered,
                priced = all.eth.priced,
                failed_sequencer_outage = all.eth.failed_sequencer_outage,
                failed_missing_oracle = all.eth.failed_missing_oracle,
                other_skipped = all.eth.other_skipped,
                "on-chain ETH pass summary"
            );
            info!(
                events_considered = all.lpt.events_considered,
                priced_twap = all.lpt.priced_twap,
                priced_degraded = all.lpt.priced_degraded,
                failed_sequencer_outage = all.lpt.failed_sequencer_outage,
                failed_missing_oracle = all.lpt.failed_missing_oracle,
                failed_missing_pool = all.lpt.failed_missing_pool,
                other_skipped = all.lpt.other_skipped,
                "on-chain LPT pass summary"
            );
            info!(
                events_considered = all.multi_asset.events_considered,
                lpt_rows_priced = all.multi_asset.lpt_rows_priced,
                eth_rows_priced = all.multi_asset.eth_rows_priced,
                lpt_zero_amount_rows = all.multi_asset.lpt_zero_amount_rows,
                eth_zero_amount_rows = all.multi_asset.eth_zero_amount_rows,
                failures = all.multi_asset.failures,
                "multi-asset pass summary"
            );
        }
    }
    Ok(())
}
