use crate::metrics::Metrics;
use crate::rpc_manager::RpcManager;
use anyhow::{bail, Result};
use livepeer_core::rpc::{with_rpc_task_label, Provider};
use livepeer_core::Config;
use livepeer_finality_watcher::runner as finality_runner;
use livepeer_indexer::{backfill::ContractKind, runner as indexer_runner};
use livepeer_reorg_watcher::runner as reorg_runner;
use livepeer_staker::runner as staker_runner;
use livepeer_valuator::runner as valuator_runner;
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::time::{sleep, Duration};
use tracing::info;

const INDEXER_INTERVAL_SECS: u64 = 12;
const REORG_INTERVAL_SECS: u64 = 60;
const FINALITY_INTERVAL_SECS: u64 = 60;
const VALUATOR_INTERVAL_SECS: u64 = 60;
const STAKER_INTERVAL_SECS: u64 = 300;
const MATVIEW_REFRESH_INTERVAL_SECS: u64 = 30;
const INDEXER_HEAD_DEPTH_BLOCKS: u64 = 10;
const INDEXER_PER_TICK_BLOCKS: u64 = 1_000;

#[derive(Clone, Debug)]
pub struct FollowConfig {
    pub max_start_lag_blocks: u64,
    pub valuation_version: String,
    pub include_tentative: bool,
}

pub async fn run_follow(
    pg: &PgPool,
    cfg: &Config,
    rpc: RpcManager,
    metrics: Arc<Metrics>,
    follow: FollowConfig,
) -> Result<()> {
    let head = rpc.archive.eth_block_number().await?;
    metrics.chain_head_block.set(head as i64);
    let lag = current_indexer_lag(pg, head).await?;
    metrics
        .task_lag_blocks
        .with_label_values(&["indexer"])
        .set(lag as i64);
    for task in ["indexer", "finality", "reorg", "valuator", "staker"] {
        update_task_rpc_metrics(&metrics, task);
    }
    if lag > follow.max_start_lag_blocks {
        bail!(
            "follow-mode startup refused: current lag {lag} exceeds max_start_lag_blocks {}",
            follow.max_start_lag_blocks
        );
    }
    info!(
        head,
        lag,
        max_start_lag_blocks = follow.max_start_lag_blocks,
        "follow-mode startup gate passed"
    );

    let shutdown = Arc::new(Notify::new());
    tokio::spawn({
        let shutdown = shutdown.clone();
        async move {
            let _ = tokio::signal::ctrl_c().await;
            shutdown.notify_waiters();
        }
    });

    let pg = pg.clone();

    let indexer_task = tokio::spawn(indexer_loop(
        pg.clone(),
        cfg.clone(),
        rpc.archive.clone(),
        metrics.clone(),
        shutdown.clone(),
    ));
    let finality_task = tokio::spawn(finality_loop(
        pg.clone(),
        rpc.l1.clone(),
        metrics.clone(),
        shutdown.clone(),
    ));
    let reorg_task = tokio::spawn(reorg_loop(
        pg.clone(),
        rpc.secondary.clone(),
        metrics.clone(),
        shutdown.clone(),
    ));
    let valuator_task = tokio::spawn(valuator_loop(
        pg.clone(),
        cfg.clone(),
        rpc.archive.clone(),
        metrics.clone(),
        shutdown.clone(),
        follow.clone(),
    ));
    let staker_task = tokio::spawn(staker_loop(
        pg.clone(),
        cfg.clone(),
        rpc.archive.clone(),
        metrics.clone(),
        shutdown.clone(),
        follow.include_tentative,
    ));
    let matview_task = tokio::spawn(matview_refresh_loop(
        pg.clone(),
        metrics.clone(),
        shutdown.clone(),
    ));

    let (a, b, c, d, e, f) = tokio::join!(
        indexer_task,
        finality_task,
        reorg_task,
        valuator_task,
        staker_task,
        matview_task
    );
    for r in [a, b, c, d, e, f] {
        match r {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// TD-025/TD-026: refresh derived materialized views on a tight cadence
/// so API reads against `broadcaster_profile` and `orchestrator_profile`
/// track upstream writes to their source tables within
/// ~`MATVIEW_REFRESH_INTERVAL_SECS`. CONCURRENTLY uses the unique index
/// on (chain_id, address) and does not block readers.
async fn matview_refresh_loop(
    pg: PgPool,
    metrics: Arc<Metrics>,
    shutdown: Arc<Notify>,
) -> Result<()> {
    /// (matview name, source table) — source name is informational only,
    /// used for log/metric labels and to keep this list grep-able.
    const VIEWS: &[&str] = &["broadcaster_profile", "orchestrator_profile"];

    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            _ = sleep(Duration::from_secs(MATVIEW_REFRESH_INTERVAL_SECS)) => {}
        }
        for view in VIEWS {
            let started = std::time::Instant::now();
            let sql = format!("REFRESH MATERIALIZED VIEW CONCURRENTLY {}", view);
            match sqlx::query(&sql).execute(&pg).await {
                Ok(_) => {
                    let elapsed = started.elapsed().as_secs_f64();
                    metrics.record_matview_refresh(view, elapsed, true);
                }
                Err(e) => {
                    tracing::warn!(target: "livepeer_daemon::supervisor",
                        view = %view, error = %e, "matview refresh failed");
                    metrics.record_matview_refresh(view, started.elapsed().as_secs_f64(), false);
                }
            }
        }
    }
    Ok(())
}

async fn indexer_loop(
    pg: PgPool,
    cfg: Config,
    archive: Arc<livepeer_core::rpc::Provider>,
    metrics: Arc<Metrics>,
    shutdown: Arc<Notify>,
) -> Result<()> {
    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            _ = sleep(Duration::from_secs(INDEXER_INTERVAL_SECS)) => {}
        }
        let started = std::time::Instant::now();
        update_task_rpc_metrics(&metrics, "indexer");
        let head = with_rpc_task_label("indexer", archive.eth_block_number()).await?;
        metrics.chain_head_block.set(head as i64);
        let to_block = head.saturating_sub(INDEXER_HEAD_DEPTH_BLOCKS);
        for contract in [
            ContractKind::BondingManager,
            ContractKind::TicketBroker,
            ContractKind::LivepeerToken,
            ContractKind::RoundsManager,
            ContractKind::Governor,
        ] {
            let from_block = livepeer_indexer::backfill::resume_from(
                &pg,
                contract,
                "",
                cfg.static_.chain.livepeer_arbitrum_genesis_block,
            )
            .await?;
            if from_block > to_block {
                continue;
            }
            let bounded_to = from_block
                .saturating_add(INDEXER_PER_TICK_BLOCKS - 1)
                .min(to_block);
            let result = with_rpc_task_label(
                "indexer",
                indexer_runner::run_backfill(indexer_runner::RunBackfillArgs {
                    pg: &pg,
                    archive: archive.as_ref(),
                    cfg: &cfg,
                    contract,
                    from_block,
                    to_block: bounded_to,
                    no_resume: true,
                    checkpoint_suffix: "",
                }),
            )
            .await;
            match result {
                Ok(summary) => {
                    metrics
                        .events_indexed_total
                        .with_label_values(&[summary.contract_name])
                        .inc_by(summary.inner.events_inserted);
                    metrics
                        .decode_failures_total
                        .with_label_values(&[summary.contract_name])
                        .inc_by(summary.inner.dead_lettered);
                    metrics
                        .task_checkpoint_block
                        .with_label_values(&["indexer"])
                        .set(bounded_to as i64);
                    metrics
                        .task_lag_blocks
                        .with_label_values(&["indexer"])
                        .set(head.saturating_sub(bounded_to) as i64);
                    info!(?summary, "daemon: indexer iteration complete");
                }
                Err(e) => {
                    metrics.record_failure("indexer", &e, started.elapsed().as_secs_f64());
                    return Err(e);
                }
            }
        }
        update_task_rpc_metrics(&metrics, "indexer");
        metrics.record_success("indexer", started.elapsed().as_secs_f64());
    }
    Ok(())
}

async fn finality_loop(
    pg: PgPool,
    l1: Arc<livepeer_core::rpc::Provider>,
    metrics: Arc<Metrics>,
    shutdown: Arc<Notify>,
) -> Result<()> {
    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            _ = sleep(Duration::from_secs(FINALITY_INTERVAL_SECS)) => {}
        }
        let started = std::time::Instant::now();
        update_task_rpc_metrics(&metrics, "finality");
        let summary = match with_rpc_task_label(
            "finality",
            finality_runner::run_once(&pg, l1.as_ref()),
        )
        .await
        {
            Ok(summary) => summary,
            Err(e) => {
                metrics.record_failure("finality", &e, started.elapsed().as_secs_f64());
                return Err(e);
            }
        };
        update_task_rpc_metrics(&metrics, "finality");
        info!(?summary, "daemon: finality iteration complete");
        metrics.record_success("finality", started.elapsed().as_secs_f64());
    }
    Ok(())
}

async fn reorg_loop(
    pg: PgPool,
    secondary: Arc<livepeer_core::rpc::Provider>,
    metrics: Arc<Metrics>,
    shutdown: Arc<Notify>,
) -> Result<()> {
    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            _ = sleep(Duration::from_secs(REORG_INTERVAL_SECS)) => {}
        }
        let started = std::time::Instant::now();
        update_task_rpc_metrics(&metrics, "reorg");
        let summary =
            match with_rpc_task_label("reorg", reorg_runner::run_once(&pg, secondary.as_ref()))
                .await
            {
                Ok(summary) => summary,
                Err(e) => {
                    metrics.record_failure("reorg", &e, started.elapsed().as_secs_f64());
                    return Err(e);
                }
            };
        if summary.divergences > 0 {
            metrics
                .reorgs_detected_total
                .with_label_values(&["info"])
                .inc_by(summary.divergences);
        }
        update_task_rpc_metrics(&metrics, "reorg");
        info!(?summary, "daemon: reorg iteration complete");
        metrics.record_success("reorg", started.elapsed().as_secs_f64());
    }
    Ok(())
}

async fn valuator_loop(
    pg: PgPool,
    cfg: Config,
    archive: Arc<livepeer_core::rpc::Provider>,
    metrics: Arc<Metrics>,
    shutdown: Arc<Notify>,
    follow: FollowConfig,
) -> Result<()> {
    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            _ = sleep(Duration::from_secs(VALUATOR_INTERVAL_SECS)) => {}
        }
        let started = std::time::Instant::now();
        update_task_rpc_metrics(&metrics, "valuator");
        let summary = match with_rpc_task_label(
            "valuator",
            valuator_runner::run_all(
                &pg,
                archive.as_ref(),
                &cfg,
                &follow.valuation_version,
                follow.include_tentative,
            ),
        )
        .await
        {
            Ok(summary) => summary,
            Err(e) => {
                metrics.record_failure("valuator", &e, started.elapsed().as_secs_f64());
                return Err(e);
            }
        };
        let priced = summary.seed.priced_this_run
            + summary.eth.priced
            + summary.lpt.priced_twap
            + summary.lpt.priced_degraded
            + summary.multi_asset.lpt_rows_priced
            + summary.multi_asset.eth_rows_priced;
        if priced > 0 {
            metrics
                .events_valued_total
                .with_label_values(&["priced"])
                .inc_by(priced);
        }
        if summary.eth.failed_missing_oracle > 0 {
            metrics
                .events_valued_total
                .with_label_values(&["failed_missing_oracle"])
                .inc_by(summary.eth.failed_missing_oracle);
        }
        if summary.eth.failed_sequencer_outage > 0 {
            metrics
                .events_valued_total
                .with_label_values(&["failed_sequencer_outage"])
                .inc_by(summary.eth.failed_sequencer_outage);
        }
        if summary.lpt.failed_missing_oracle > 0 {
            metrics
                .events_valued_total
                .with_label_values(&["failed_missing_oracle"])
                .inc_by(summary.lpt.failed_missing_oracle);
        }
        if summary.lpt.failed_missing_pool > 0 {
            metrics
                .events_valued_total
                .with_label_values(&["failed_missing_pool"])
                .inc_by(summary.lpt.failed_missing_pool);
        }
        if summary.lpt.failed_sequencer_outage > 0 {
            metrics
                .events_valued_total
                .with_label_values(&["failed_sequencer_outage"])
                .inc_by(summary.lpt.failed_sequencer_outage);
        }
        if summary.multi_asset.failures > 0 {
            metrics
                .events_valued_total
                .with_label_values(&["failed_other"])
                .inc_by(summary.multi_asset.failures);
        }
        update_task_rpc_metrics(&metrics, "valuator");
        info!(?summary, "daemon: valuator iteration complete");
        metrics.record_success("valuator", started.elapsed().as_secs_f64());
    }
    Ok(())
}

async fn staker_loop(
    pg: PgPool,
    cfg: Config,
    archive: Arc<livepeer_core::rpc::Provider>,
    metrics: Arc<Metrics>,
    shutdown: Arc<Notify>,
    include_tentative: bool,
) -> Result<()> {
    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            _ = sleep(Duration::from_secs(STAKER_INTERVAL_SECS)) => {}
        }
        let started = std::time::Instant::now();
        update_task_rpc_metrics(&metrics, "staker");
        let backfill = match staker_runner::run_backfill(&pg, include_tentative).await {
            Ok(summary) => summary,
            Err(e) => {
                metrics.record_failure("staker", &e, started.elapsed().as_secs_f64());
                return Err(e);
            }
        };
        info!(?backfill, "daemon: staker backfill iteration complete");
        let gateway = with_rpc_task_label(
            "staker",
            staker_runner::run_gateway_backfill(&pg, archive.as_ref(), &cfg, include_tentative),
        )
        .await;
        let gateway = match gateway {
            Ok(summary) => summary,
            Err(e) => {
                metrics.record_failure("staker", &e, started.elapsed().as_secs_f64());
                return Err(e);
            }
        };
        info!(?gateway, "daemon: gateway backfill iteration complete");
        let refresh = with_rpc_task_label(
            "staker",
            staker_runner::run_refresh_pending(&pg, archive.as_ref(), &cfg, include_tentative),
        )
        .await;
        let refresh = match refresh {
            Ok(summary) => summary,
            Err(e) => {
                metrics.record_failure("staker", &e, started.elapsed().as_secs_f64());
                return Err(e);
            }
        };
        info!(?refresh, "daemon: staker refresh iteration complete");
        let current = with_rpc_task_label(
            "staker",
            staker_runner::run_refresh_current_stake(&pg, archive.as_ref(), &cfg, include_tentative),
        )
        .await;
        let current = match current {
            Ok(summary) => summary,
            Err(e) => {
                metrics.record_failure("staker", &e, started.elapsed().as_secs_f64());
                return Err(e);
            }
        };
        update_task_rpc_metrics(&metrics, "staker");
        info!(
            ?current,
            "daemon: staker current-stake refresh iteration complete"
        );
        metrics.record_success("staker", started.elapsed().as_secs_f64());
    }
    Ok(())
}

fn update_task_rpc_metrics(metrics: &Metrics, task: &'static str) {
    if let Some(snapshot) = Provider::task_concurrency_snapshot(task) {
        metrics
            .task_rpc_limit
            .with_label_values(&[task])
            .set(snapshot.limit as i64);
        metrics
            .task_rpc_in_flight
            .with_label_values(&[task])
            .set(snapshot.in_flight as i64);
    }
}

async fn current_indexer_lag(pg: &PgPool, head: u64) -> Result<u64> {
    let min_cp: Option<i64> = sqlx::query_scalar(
        r#"SELECT MIN(last_processed_block)
             FROM indexer_checkpoints
            WHERE name IN (
              'indexer_BondingManager',
              'indexer_TicketBroker',
              'indexer_LivepeerToken',
              'indexer_RoundsManager',
              'indexer_Governor'
            )"#,
    )
    .fetch_one(pg)
    .await?;
    let Some(cp) = min_cp else {
        return Ok(u64::MAX / 2);
    };
    Ok(head.saturating_sub(cp as u64))
}
