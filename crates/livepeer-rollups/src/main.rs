use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use livepeer_core::{config::Config, db, tracing_init};
use livepeer_rollups::runner;
use std::{path::PathBuf, time::Duration};
use tracing::{error, info};

const SERVICE: &str = "livepeer-rollups";
const DEFAULT_CADENCE_SECS: u64 = 300;
const DEFAULT_BATCH_LIMIT: i64 = 2_000;

#[derive(Parser, Debug)]
#[command(name = SERVICE, about = "Materialized rollup writers for old-API parity analytics.")]
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
    #[arg(long, default_value_t = false, global = true)]
    include_tentative: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// TD-017 Phase 2: materialize daily orchestrator payout aggregates from
    /// finalized WinningTicketRedeemed rows plus valuation data.
    OrchPayoutsDaily {
        #[arg(long, default_value_t = DEFAULT_BATCH_LIMIT)]
        batch_limit: i64,
        #[arg(long, default_value_t = DEFAULT_CADENCE_SECS)]
        cadence_secs: u64,
        #[arg(long, default_value_t = false)]
        follow: bool,
    },
    /// TD-017 Phase 3: materialize daily orchestrator reward aggregates from
    /// finalized Reward rows plus valuation data.
    OrchRewardsDaily {
        #[arg(long, default_value_t = DEFAULT_BATCH_LIMIT)]
        batch_limit: i64,
        #[arg(long, default_value_t = DEFAULT_CADENCE_SECS)]
        cadence_secs: u64,
        #[arg(long, default_value_t = false)]
        follow: bool,
    },
    /// TD-017 Phase 3: materialize daily ticket counts and distinct entity
    /// counts split by broadcaster kind.
    TicketsDaily {
        #[arg(long, default_value_t = DEFAULT_BATCH_LIMIT)]
        batch_limit: i64,
        #[arg(long, default_value_t = DEFAULT_CADENCE_SECS)]
        cadence_secs: u64,
        #[arg(long, default_value_t = false)]
        follow: bool,
    },
    /// TD-018 Phase 1: materialize daily event metrics (count, sum_amount_native,
    /// sum_amount_usd) per (contract, event_name, asset, valuation_version).
    /// Backs /aggregations/events broad-window queries.
    EventMetricsDaily {
        #[arg(long, default_value_t = DEFAULT_BATCH_LIMIT)]
        batch_limit: i64,
        #[arg(long, default_value_t = DEFAULT_CADENCE_SECS)]
        cadence_secs: u64,
        #[arg(long, default_value_t = false)]
        follow: bool,
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
        Command::OrchPayoutsDaily {
            batch_limit,
            cadence_secs,
            follow,
        } => {
            if follow {
                loop {
                    match runner::run_orch_payouts_daily(&pg, cli.include_tentative, batch_limit)
                        .await
                    {
                        Ok(summary) => info!(
                            events_seen = summary.events_seen,
                            rows_written = summary.rows_written,
                            groups_touched = summary.groups_touched,
                            checkpoint_event_id = summary.checkpoint_event_id,
                            "orch payouts rollup summary"
                        ),
                        Err(e) => {
                            error!(error = %e, "orch payouts rollup iteration failed")
                        }
                    }
                    tokio::time::sleep(Duration::from_secs(cadence_secs)).await;
                }
            } else {
                let summary =
                    runner::run_orch_payouts_daily(&pg, cli.include_tentative, batch_limit).await?;
                info!(
                    events_seen = summary.events_seen,
                    rows_written = summary.rows_written,
                    groups_touched = summary.groups_touched,
                    checkpoint_event_id = summary.checkpoint_event_id,
                    "orch payouts rollup summary"
                );
            }
        }
        Command::OrchRewardsDaily {
            batch_limit,
            cadence_secs,
            follow,
        } => {
            if follow {
                loop {
                    match runner::run_orch_rewards_daily(&pg, cli.include_tentative, batch_limit)
                        .await
                    {
                        Ok(summary) => info!(
                            events_seen = summary.events_seen,
                            rows_written = summary.rows_written,
                            groups_touched = summary.groups_touched,
                            checkpoint_event_id = summary.checkpoint_event_id,
                            "orch rewards rollup summary"
                        ),
                        Err(e) => {
                            error!(error = %e, "orch rewards rollup iteration failed")
                        }
                    }
                    tokio::time::sleep(Duration::from_secs(cadence_secs)).await;
                }
            } else {
                let summary =
                    runner::run_orch_rewards_daily(&pg, cli.include_tentative, batch_limit).await?;
                info!(
                    events_seen = summary.events_seen,
                    rows_written = summary.rows_written,
                    groups_touched = summary.groups_touched,
                    checkpoint_event_id = summary.checkpoint_event_id,
                    "orch rewards rollup summary"
                );
            }
        }
        Command::TicketsDaily {
            batch_limit,
            cadence_secs,
            follow,
        } => {
            if follow {
                loop {
                    match runner::run_tickets_daily(&pg, cli.include_tentative, batch_limit).await {
                        Ok(summary) => info!(
                            events_seen = summary.events_seen,
                            rows_written = summary.rows_written,
                            groups_touched = summary.groups_touched,
                            checkpoint_event_id = summary.checkpoint_event_id,
                            "tickets daily rollup summary"
                        ),
                        Err(e) => {
                            error!(error = %e, "tickets daily rollup iteration failed")
                        }
                    }
                    tokio::time::sleep(Duration::from_secs(cadence_secs)).await;
                }
            } else {
                let summary =
                    runner::run_tickets_daily(&pg, cli.include_tentative, batch_limit).await?;
                info!(
                    events_seen = summary.events_seen,
                    rows_written = summary.rows_written,
                    groups_touched = summary.groups_touched,
                    checkpoint_event_id = summary.checkpoint_event_id,
                    "tickets daily rollup summary"
                );
            }
        }
        Command::EventMetricsDaily {
            batch_limit,
            cadence_secs,
            follow,
        } => {
            if follow {
                loop {
                    match runner::run_event_metrics_daily(&pg, cli.include_tentative, batch_limit)
                        .await
                    {
                        Ok(summary) => info!(
                            events_seen = summary.events_seen,
                            rows_written = summary.rows_written,
                            groups_touched = summary.groups_touched,
                            checkpoint_event_id = summary.checkpoint_event_id,
                            "event metrics rollup summary"
                        ),
                        Err(e) => {
                            error!(error = %e, "event metrics rollup iteration failed")
                        }
                    }
                    tokio::time::sleep(Duration::from_secs(cadence_secs)).await;
                }
            } else {
                let summary =
                    runner::run_event_metrics_daily(&pg, cli.include_tentative, batch_limit).await?;
                info!(
                    events_seen = summary.events_seen,
                    rows_written = summary.rows_written,
                    groups_touched = summary.groups_touched,
                    checkpoint_event_id = summary.checkpoint_event_id,
                    "event metrics rollup summary"
                );
            }
        }
    }

    Ok(())
}
