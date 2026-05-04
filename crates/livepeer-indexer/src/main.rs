use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use livepeer_core::{config::Config, db, rpc::Provider, tracing_init};
use livepeer_indexer::{backfill, runner};
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
        /// Optional non-empty suffix to namespace the checkpoint
        /// (e.g. "patch" → indexer_BondingManager_patch). Used to run a
        /// parallel patch indexer against the same contract without
        /// colliding with the live run's checkpoint.
        #[arg(long, default_value = "")]
        checkpoint_suffix: String,
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
            checkpoint_suffix,
        } => {
            let archive_url = cfg.archive_rpc_url().context("CHAINSTACK_RPC_URL")?;
            let archive = Provider::new("chainstack", archive_url)?;
            let kind = contract.to_kind();
            let actual_from = if no_resume {
                from_block
            } else {
                backfill::resume_from(&pg, kind, &checkpoint_suffix, from_block).await?
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

            let summary = runner::run_backfill(
                &pg,
                &archive,
                &cfg,
                kind,
                from_block,
                to_block,
                no_resume,
                &checkpoint_suffix,
            )
            .await?;
            info!(
                contract = summary.contract_name,
                checkpoint_suffix = %checkpoint_suffix,
                chunks = summary.inner.chunks,
                logs_seen = summary.inner.logs_seen,
                events_inserted = summary.inner.events_inserted,
                dead_lettered = summary.inner.dead_lettered,
                final_batch_size = summary.inner.final_batch_size,
                from_block = summary.actual_from,
                to_block = summary.to_block,
                "backfill complete"
            );
        }
    }
    Ok(())
}
