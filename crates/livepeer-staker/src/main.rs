use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use livepeer_core::{config::Config, db, rpc::Provider, tracing_init};
use livepeer_staker::runner;
use std::path::PathBuf;
use tracing::info;

const SERVICE: &str = "livepeer-staker";

#[derive(Parser, Debug)]
#[command(name = SERVICE, about = "Computes and persists delegator stake balances at event-touching blocks (Scope 2).")]
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
    /// Allow tentative events. SPEC §9.1 requires finalized in production; without
    /// a finality watcher running, all rows are tentative — this is the dev override.
    #[arg(long, default_value_t = false, global = true)]
    include_tentative: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// (S9.1) Flow-derived `bonded_principal` per delegator per event-touching block.
    /// Walks Bond / Unbond / Rebond / WithdrawStake / EarningsClaimed / TransferBond
    /// events in (block, log_index) order; populates stake_balances_by_block and
    /// delegator_registry. Idempotent.
    Backfill,
    /// (S9.x) Materialize historical TicketBroker gateway sender balances at
    /// gateway-touching blocks into gateway_balances_by_block.
    GatewayBackfill,
    /// (S9.2) Refresh pending_stake / pending_fees on existing stake rows by
    /// calling BondingManager.pendingStake / pendingFees at each EarningsClaimed
    /// event block.
    RefreshPending,
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
        Command::Backfill => {
            let summary = runner::run_backfill(&pg, cli.include_tentative).await?;
            info!(
                events_seen = summary.events_seen,
                bond_events = summary.bond_events,
                stake_rows_written = summary.stake_rows_written,
                delegators_registered = summary.delegators_registered,
                skipped_unregistered = summary.skipped_unregistered,
                "staker flow backfill summary"
            );
        }
        Command::GatewayBackfill => {
            let archive_url = cfg.archive_rpc_url().context("CHAINSTACK_RPC_URL")?;
            let archive = Provider::new("chainstack", archive_url)?;
            let summary =
                runner::run_gateway_backfill(&pg, &archive, &cfg, cli.include_tentative).await?;
            info!(
                balance_candidates_seen = summary.balance_candidates_seen,
                balance_rows_written = summary.balance_rows_written,
                flow_candidates_seen = summary.flow_candidates_seen,
                flow_rows_written = summary.flow_rows_written,
                claimant_rows_written = summary.claimant_rows_written,
                gateways_touched = summary.gateways_touched,
                claimants_touched = summary.claimants_touched,
                "gateway backfill summary"
            );
        }
        Command::RefreshPending => {
            let archive_url = cfg.archive_rpc_url().context("CHAINSTACK_RPC_URL")?;
            let archive = Provider::new("chainstack", archive_url)?;
            let summary =
                runner::run_refresh_pending(&pg, &archive, &cfg, cli.include_tentative).await?;
            info!(
                reconciled_rows = summary.reconciled_rows,
                events_considered = summary.events_considered,
                refreshed = summary.refreshed,
                failed_decode = summary.failed_decode,
                no_stake_row = summary.no_stake_row,
                "staker pending refresh summary"
            );
        }
    }
    Ok(())
}
