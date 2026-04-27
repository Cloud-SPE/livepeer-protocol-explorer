mod backfill;
mod events;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
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
    /// Backfill all known events from a contract over [from_block, to_block]. Idempotent.
    Backfill {
        #[arg(long, value_enum)]
        contract: ContractArg,
        #[arg(long)]
        from_block: u64,
        #[arg(long)]
        to_block: u64,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ContractArg {
    BondingManager,
    TicketBroker,
    LivepeerToken,
}

impl ContractArg {
    fn to_kind(self) -> backfill::ContractKind {
        match self {
            ContractArg::BondingManager => backfill::ContractKind::BondingManager,
            ContractArg::TicketBroker => backfill::ContractKind::TicketBroker,
            ContractArg::LivepeerToken => backfill::ContractKind::LivepeerToken,
        }
    }
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
        Command::Backfill {
            contract,
            from_block,
            to_block,
        } => {
            let archive_url = cfg.archive_rpc_url().context("CHAINSTACK_RPC_URL")?;
            let archive = Provider::new("chainstack", archive_url)?;
            let kind = contract.to_kind();
            let proxy = match kind {
                backfill::ContractKind::BondingManager => &cfg.static_.contracts.bonding_manager,
                backfill::ContractKind::TicketBroker => &cfg.static_.contracts.ticket_broker,
                backfill::ContractKind::LivepeerToken => &cfg.static_.contracts.livepeer_token,
            }
            .to_lowercase();
            let abi_hash: String = sqlx::query_scalar(
                "SELECT abi_hash FROM contract_abi_registry WHERE contract_name = $1",
            )
            .bind(kind.name())
            .fetch_one(&pg)
            .await
            .with_context(|| format!("loading {} abi_hash from registry", kind.name()))?;

            let inserted = backfill::backfill_contract(
                &pg,
                &archive,
                kind,
                &proxy,
                &abi_hash,
                from_block,
                to_block,
            )
            .await?;
            info!(
                inserted,
                contract = kind.name(),
                from_block,
                to_block,
                "backfill complete"
            );
        }
    }
    Ok(())
}
