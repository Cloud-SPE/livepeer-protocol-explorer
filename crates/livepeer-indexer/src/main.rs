mod backfill;
mod events;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use livepeer_core::{config::Config, db, rpc::Provider, tracing_init};
use std::path::PathBuf;
use tracing::info;

const SERVICE: &str = "livepeer-indexer";

#[derive(Parser, Debug)]
#[command(name = SERVICE, about = "Pulls logs from RPC, decodes against the ABI registry, writes raw_protocol_events.")]
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
    /// (S6.1) Backfill `Reward` events for a block range. Idempotent.
    BackfillRewards {
        #[arg(long)]
        from_block: u64,
        #[arg(long)]
        to_block: u64,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_init::init("info");

    let cli = Cli::parse();
    let cfg = Config::load(&cli.static_config, &cli.env_config).context("loading config")?;
    let database_url = cfg.database_url().context("DATABASE_URL")?;
    let pg = db::connect(&database_url, cfg.env.postgres.pool_max_connections)
        .await
        .context("connecting to Postgres")?;
    info!(service = SERVICE, "config + db ready");

    match cli.command {
        Command::BackfillRewards { from_block, to_block } => {
            let archive_url = cfg.archive_rpc_url().context("CHAINSTACK_RPC_URL")?;
            let archive = Provider::new("chainstack", archive_url)?;
            let bonding_manager = cfg.static_.contracts.bonding_manager.to_lowercase();
            let abi_hash: String = sqlx::query_scalar(
                "SELECT abi_hash FROM contract_abi_registry WHERE contract_name = 'BondingManager'",
            )
            .fetch_one(&pg)
            .await
            .context("loading BondingManager abi_hash from registry")?;
            let inserted = backfill::backfill_rewards(
                &pg,
                &archive,
                &bonding_manager,
                &abi_hash,
                from_block,
                to_block,
            )
            .await?;
            info!(inserted, from_block, to_block, "Reward backfill complete");
        }
    }
    Ok(())
}
