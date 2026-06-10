use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use livepeer_core::{config::Config, db, rpc::Provider, tracing_init};
use livepeer_staker::runner;
use std::{path::PathBuf, time::Duration};
use tracing::{error, info};

const SERVICE: &str = "livepeer-staker";
const DEFAULT_PROFILE_FOLLOW_CADENCE_SECS: u64 = 300;
const DEFAULT_TX_RECEIPTS_FOLLOW_CADENCE_SECS: u64 = 300;

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
    /// Round-anchored current-stake refresh. Once per protocol round, at the
    /// latest finalized NewRound block, reads getDelegator + pendingStake for
    /// every delegator whose latest stake row claims a positive bonded
    /// principal, and writes truthful rows (source 'round_refresh'). Keeps
    /// passive delegators' stake current between their own events and
    /// self-heals stale delegate/balance state.
    RefreshCurrentStake,
    /// (TD-017 Phase 1) Materialize deterministic orchestrator and broadcaster
    /// profile rows from indexed trigger events plus cached point-in-time RPC reads.
    ProfileBackfill,
    /// (TD-019) Live-mode profile refresh. Wraps `profile-backfill` in a bounded
    /// poll loop so `orchestrator_profile` and `broadcaster_profile` stay current
    /// once history is caught up. Sleeps `cadence_secs` between iterations.
    ProfileFollow {
        #[arg(long, default_value_t = DEFAULT_PROFILE_FOLLOW_CADENCE_SECS)]
        cadence_secs: u64,
    },
    /// (TD-020) One-shot bounded backfill of `tx_receipts`. Walks finalized
    /// canonical events in `raw_protocol_events`, fetches each unique
    /// tx_hash's `eth_getTransactionReceipt` via `single_call_cached`, and
    /// writes a typed projection. Idempotent on `(chain_id, tx_hash)`.
    TxReceiptsBackfill {
        #[arg(long, default_value_t = livepeer_staker::tx_receipts::DEFAULT_BATCH_LIMIT)]
        batch_limit: i64,
        #[arg(long, default_value_t = livepeer_staker::tx_receipts::DEFAULT_CONCURRENCY)]
        concurrency: usize,
    },
    /// (TD-020) Live-mode wrapper around `tx-receipts-backfill`. Skip-sleeps
    /// while there are still candidates; sleeps `cadence_secs` once caught up.
    TxReceiptsFollow {
        #[arg(long, default_value_t = DEFAULT_TX_RECEIPTS_FOLLOW_CADENCE_SECS)]
        cadence_secs: u64,
        #[arg(long, default_value_t = livepeer_staker::tx_receipts::DEFAULT_BATCH_LIMIT)]
        batch_limit: i64,
        #[arg(long, default_value_t = livepeer_staker::tx_receipts::DEFAULT_CONCURRENCY)]
        concurrency: usize,
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
        Command::Backfill => {
            let summary = runner::run_backfill(&pg, cli.include_tentative).await?;
            info!(
                events_seen = summary.events_seen,
                delegators_replayed = summary.delegators_replayed,
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
        Command::RefreshCurrentStake => {
            let archive_url = cfg.archive_rpc_url().context("CHAINSTACK_RPC_URL")?;
            let archive = Provider::new("chainstack", archive_url)?;
            let summary =
                runner::run_refresh_current_stake(&pg, &archive, &cfg, cli.include_tentative)
                    .await?;
            info!(
                round = summary.round,
                anchor_block = summary.anchor_block,
                candidates = summary.candidates,
                rows_refreshed = summary.rows_refreshed,
                zeroed = summary.zeroed,
                "staker current-stake refresh summary"
            );
        }
        Command::ProfileBackfill => {
            let archive_url = cfg.archive_rpc_url().context("CHAINSTACK_RPC_URL")?;
            let archive = Provider::new("chainstack", archive_url)?;
            let summary =
                runner::run_profile_backfill(&pg, &archive, &cfg, cli.include_tentative).await?;
            info!(
                orch_events_seen = summary.orch_events_seen,
                orch_rows_written = summary.orch_rows_written,
                orchestrators_touched = summary.orchestrators_touched,
                "staker profile backfill summary"
            );
        }
        Command::ProfileFollow { cadence_secs } => {
            let archive_url = cfg.archive_rpc_url().context("CHAINSTACK_RPC_URL")?;
            let archive = Provider::new("chainstack", archive_url)?;
            info!(cadence_secs, "staker profile follow loop starting");
            loop {
                // Skip the cadence sleep while there's still work to do
                // (events_seen > 0 means we processed a non-empty batch and
                // there are likely more candidates waiting). The sleep is
                // only needed once we're caught up. On error we sleep so a
                // failing iteration doesn't tight-loop against the RPC.
                let mut should_sleep = true;
                match runner::run_profile_backfill(&pg, &archive, &cfg, cli.include_tentative).await
                {
                    Ok(summary) => {
                        info!(
                            orch_events_seen = summary.orch_events_seen,
                            orch_rows_written = summary.orch_rows_written,
                            orchestrators_touched = summary.orchestrators_touched,
                            "staker profile follow iteration summary"
                        );
                        if summary.orch_events_seen > 0 {
                            should_sleep = false;
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "staker profile follow iteration failed")
                    }
                }
                if should_sleep {
                    tokio::time::sleep(Duration::from_secs(cadence_secs)).await;
                }
            }
        }
        Command::TxReceiptsBackfill {
            batch_limit,
            concurrency,
        } => {
            let archive_url = cfg.archive_rpc_url().context("CHAINSTACK_RPC_URL")?;
            let archive = Provider::new("chainstack", archive_url)?;
            let summary = runner::run_tx_receipts_backfill(
                &pg,
                &archive,
                cli.include_tentative,
                batch_limit,
                concurrency,
            )
            .await?;
            info!(
                candidates_seen = summary.candidates_seen,
                rows_written = summary.rows_written,
                rows_skipped_missing_receipt = summary.rows_skipped_missing_receipt,
                last_processed_block = ?summary.last_processed_block,
                elapsed_ms = summary.elapsed_ms,
                "tx-receipts backfill summary"
            );
        }
        Command::TxReceiptsFollow {
            cadence_secs,
            batch_limit,
            concurrency,
        } => {
            let archive_url = cfg.archive_rpc_url().context("CHAINSTACK_RPC_URL")?;
            let archive = Provider::new("chainstack", archive_url)?;
            info!(
                cadence_secs,
                batch_limit, concurrency, "staker tx-receipts follow loop starting"
            );
            loop {
                let mut should_sleep = true;
                match runner::run_tx_receipts_backfill(
                    &pg,
                    &archive,
                    cli.include_tentative,
                    batch_limit,
                    concurrency,
                )
                .await
                {
                    Ok(summary) => {
                        info!(
                            candidates_seen = summary.candidates_seen,
                            rows_written = summary.rows_written,
                            rows_skipped_missing_receipt = summary.rows_skipped_missing_receipt,
                            last_processed_block = ?summary.last_processed_block,
                            elapsed_ms = summary.elapsed_ms,
                            "tx-receipts follow iteration summary"
                        );
                        if summary.candidates_seen > 0 {
                            should_sleep = false;
                        }
                    }
                    Err(e) => error!(error = %e, "tx-receipts follow iteration failed"),
                }
                if should_sleep {
                    tokio::time::sleep(Duration::from_secs(cadence_secs)).await;
                }
            }
        }
    }
    Ok(())
}
