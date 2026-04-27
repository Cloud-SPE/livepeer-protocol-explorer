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
    /// Resumes from `indexer_checkpoints('main')` if it's past `--from-block` (use
    /// `--no-resume` to override). Halts on any strict-decode failure (§10.2.1).
    Backfill {
        #[arg(long, value_enum)]
        contract: ContractArg,
        #[arg(long)]
        from_block: u64,
        #[arg(long)]
        to_block: u64,
        /// Skip checkpoint resume; start exactly at `--from-block`.
        #[arg(long, default_value_t = false)]
        no_resume: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ContractArg {
    BondingManager,
    TicketBroker,
    LivepeerToken,
    RoundsManager,
    Governor,
}

impl ContractArg {
    fn to_kind(self) -> backfill::ContractKind {
        match self {
            ContractArg::BondingManager => backfill::ContractKind::BondingManager,
            ContractArg::TicketBroker => backfill::ContractKind::TicketBroker,
            ContractArg::LivepeerToken => backfill::ContractKind::LivepeerToken,
            ContractArg::RoundsManager => backfill::ContractKind::RoundsManager,
            ContractArg::Governor => backfill::ContractKind::Governor,
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
            no_resume,
        } => {
            let archive_url = cfg.archive_rpc_url().context("CHAINSTACK_RPC_URL")?;
            let archive = Provider::new("chainstack", archive_url)?;
            let kind = contract.to_kind();
            let proxy = match kind {
                backfill::ContractKind::BondingManager => &cfg.static_.contracts.bonding_manager,
                backfill::ContractKind::TicketBroker => &cfg.static_.contracts.ticket_broker,
                backfill::ContractKind::LivepeerToken => &cfg.static_.contracts.livepeer_token,
                backfill::ContractKind::RoundsManager => &cfg.static_.contracts.rounds_manager,
                backfill::ContractKind::Governor => &cfg.static_.contracts.governor,
            }
            .to_lowercase();
            let abi_hash: String = sqlx::query_scalar(
                "SELECT abi_hash FROM contract_abi_registry WHERE contract_name = $1",
            )
            .bind(kind.name())
            .fetch_one(&pg)
            .await
            .with_context(|| format!("loading {} abi_hash from registry", kind.name()))?;

            let actual_from = if no_resume {
                from_block
            } else {
                backfill::resume_from(&pg, kind, from_block).await?
            };
            if actual_from != from_block {
                info!(
                    requested_from = from_block,
                    resumed_from = actual_from,
                    "resuming from checkpoint"
                );
            }
            if actual_from > to_block {
                info!(actual_from, to_block, "checkpoint already past target — nothing to do");
                return Ok(());
            }

            let summary = backfill::drive_backfill(
                &pg,
                &archive,
                kind,
                &proxy,
                &abi_hash,
                actual_from,
                to_block,
            )
            .await?;
            info!(
                contract = kind.name(),
                chunks = summary.chunks,
                logs_seen = summary.logs_seen,
                events_inserted = summary.events_inserted,
                dead_lettered = summary.dead_lettered,
                final_batch_size = summary.final_batch_size,
                from_block = actual_from,
                to_block,
                "backfill complete"
            );
        }
    }
    Ok(())
}
