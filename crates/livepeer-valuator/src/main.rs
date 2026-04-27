mod seed;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use livepeer_core::{config::Config, db, tracing_init};
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

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// (S8.1) Run the seed-hit pricing pass over all unvalued events.
    /// Writes `event_valuations` for events whose `(tx_hash, asset)` matches a
    /// seed row. Skips multi-asset events (asset=NULL); skips events without
    /// seed coverage (those need on-chain pricing — S8.2).
    BackfillFromSeed {
        /// Override the configured default valuation version.
        #[arg(long)]
        version: Option<String>,
        /// Allow tentative events (development override). SPEC §9.1 requires
        /// finality='finalized' in production; without a finality watcher all
        /// events stay tentative, so this flag exists for end-to-end testing.
        #[arg(long, default_value_t = false)]
        include_tentative: bool,
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
    info!(service = SERVICE, "config + db ready");

    match cli.command {
        Command::BackfillFromSeed { version, include_tentative } => {
            let valuation_version =
                version.unwrap_or_else(|| cfg.static_.pricing.default_valuation_version.clone());
            let summary =
                seed::run_seed_pass(&pg, &valuation_version, include_tentative).await?;
            info!(
                events_considered = summary.events_considered,
                seed_hits = summary.seed_hits,
                seed_misses = summary.seed_misses,
                priced_this_run = summary.priced_this_run,
                multi_asset_skipped = summary.multi_asset_skipped,
                "seed-pass summary"
            );
        }
    }
    Ok(())
}
