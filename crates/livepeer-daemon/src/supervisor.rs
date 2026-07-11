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
use std::future::Future;
use std::sync::Arc;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::{sleep, Duration};
use tracing::{error, info, warn};

/// All supervised task names (labels for metrics + `/health`).
pub const SUPERVISED_TASKS: [&str; 6] =
    ["indexer", "finality", "reorg", "valuator", "staker", "matview"];

/// Restart behavior for a supervised loop.
#[derive(Clone, Copy, Debug)]
pub struct RestartPolicy {
    pub base_backoff: Duration,
    pub max_backoff: Duration,
    /// Consecutive deaths-without-progress before a loop is marked `task_up=0`.
    pub max_consecutive: u32,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            base_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(60),
            max_consecutive: 10,
        }
    }
}

/// Keep one loop alive: (re)spawn it, catch errors *and* panics, back off, and
/// escalate (mark `task_up=0`) after repeated deaths-without-progress — but
/// never crash the process for a single broken loop (the `/health` probe turns
/// a persistently-down/wedged task into a whole-container restart). Returns
/// only when `shutdown` is observed. Must stay panic-free: a supervisor panic
/// is treated as fatal by `run_follow`.
async fn supervise<F, Fut>(
    task: &'static str,
    metrics: Arc<Metrics>,
    mut shutdown: watch::Receiver<bool>,
    policy: RestartPolicy,
    mut make_fut: F,
) where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    let mut consecutive: u32 = 0;
    loop {
        if *shutdown.borrow() {
            break;
        }

        let hb_before = metrics.heartbeat(task);
        let outcome = tokio::spawn(make_fut()).await;

        // Any exit during/after shutdown is a clean stop, not a restart.
        if *shutdown.borrow() {
            break;
        }

        let reason = match outcome {
            Ok(Ok(())) => {
                // A loop only returns Ok on shutdown; reaching here without
                // shutdown is anomalous — restart it.
                warn!(task, "supervised loop returned unexpectedly; will restart");
                "error"
            }
            Ok(Err(e)) => {
                warn!(task, error = %e, "supervised loop errored; will restart");
                "error"
            }
            Err(join_err) => {
                if join_err.is_cancelled() {
                    // Nothing aborts the inner handle here; a cancel means the
                    // runtime is tearing down. Stop without restarting.
                    break;
                }
                error!(task, "supervised loop panicked; will restart");
                "panic"
            }
        };
        metrics.record_restart(task, reason);

        // Progress-based reset: if the loop completed >=1 iteration its
        // heartbeat advanced, so it made progress — reset the death counter.
        if metrics.heartbeat(task) > hb_before {
            consecutive = 0;
        } else {
            consecutive = consecutive.saturating_add(1);
        }

        if consecutive > policy.max_consecutive {
            error!(
                task,
                consecutive, "supervised loop exceeded restart budget; marking task down"
            );
            metrics.set_task_up(task, false);
        } else {
            metrics.set_task_up(task, true);
        }

        // Exponential backoff (base * 2^(n-1), capped), interruptible by shutdown.
        let shift = consecutive.saturating_sub(1).min(6);
        let backoff = policy
            .base_backoff
            .saturating_mul(1u32 << shift)
            .min(policy.max_backoff);
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = sleep(backoff) => {}
        }
    }
    info!(task, "supervisor stopped (shutdown)");
}

const INDEXER_INTERVAL_SECS: u64 = 12;
const REORG_INTERVAL_SECS: u64 = 60;
const FINALITY_INTERVAL_SECS: u64 = 60;
const VALUATOR_INTERVAL_SECS: u64 = 60;
const STAKER_INTERVAL_SECS: u64 = 300;
const INDEXER_HEAD_DEPTH_BLOCKS: u64 = 10;
const INDEXER_PER_TICK_BLOCKS: u64 = 1_000;

#[derive(Clone, Debug)]
pub struct FollowConfig {
    pub max_start_lag_blocks: u64,
    pub valuation_version: String,
    pub include_tentative: bool,
    /// Materialized-view refresh cadence in seconds (tunable to trade profile
    /// freshness for DB load).
    pub matview_refresh_secs: u64,
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

    // Latched shutdown: `watch` (not `Notify`) so a signal delivered during a
    // supervisor's backoff/respawn gap is not lost. Both SIGINT and SIGTERM
    // (docker stop / compose) set it.
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    tokio::spawn({
        let tx = shutdown_tx.clone();
        async move {
            let mut term = match signal(SignalKind::terminate()) {
                Ok(s) => s,
                Err(e) => {
                    error!(error = %e, "failed to install SIGTERM handler; SIGINT only");
                    let _ = tokio::signal::ctrl_c().await;
                    let _ = tx.send(true);
                    return;
                }
            };
            tokio::select! {
                _ = tokio::signal::ctrl_c() => info!("received SIGINT; shutting down"),
                _ = term.recv() => info!("received SIGTERM; shutting down"),
            }
            let _ = tx.send(true);
        }
    });

    // Initialize per-task liveness so `/health` has a fresh baseline before the
    // first iteration completes, and every task starts "up".
    for task in SUPERVISED_TASKS {
        metrics.beat(task);
        metrics.set_task_up(task, true);
    }

    let pg = pg.clone();
    let policy = RestartPolicy::default();
    let mut set: JoinSet<()> = JoinSet::new();

    set.spawn(supervise("indexer", metrics.clone(), shutdown_rx.clone(), policy, {
        let pg = pg.clone();
        let cfg = cfg.clone();
        let archive = rpc.archive.clone();
        let metrics = metrics.clone();
        let sd = shutdown_rx.clone();
        move || indexer_loop(pg.clone(), cfg.clone(), archive.clone(), metrics.clone(), sd.clone())
    }));
    set.spawn(supervise("finality", metrics.clone(), shutdown_rx.clone(), policy, {
        let pg = pg.clone();
        let l1 = rpc.l1.clone();
        let metrics = metrics.clone();
        let sd = shutdown_rx.clone();
        move || finality_loop(pg.clone(), l1.clone(), metrics.clone(), sd.clone())
    }));
    set.spawn(supervise("reorg", metrics.clone(), shutdown_rx.clone(), policy, {
        let pg = pg.clone();
        let secondary = rpc.secondary.clone();
        let metrics = metrics.clone();
        let sd = shutdown_rx.clone();
        move || reorg_loop(pg.clone(), secondary.clone(), metrics.clone(), sd.clone())
    }));
    set.spawn(supervise("valuator", metrics.clone(), shutdown_rx.clone(), policy, {
        let pg = pg.clone();
        let cfg = cfg.clone();
        let archive = rpc.archive.clone();
        let metrics = metrics.clone();
        let follow = follow.clone();
        let sd = shutdown_rx.clone();
        move || {
            valuator_loop(
                pg.clone(),
                cfg.clone(),
                archive.clone(),
                metrics.clone(),
                sd.clone(),
                follow.clone(),
            )
        }
    }));
    set.spawn(supervise("staker", metrics.clone(), shutdown_rx.clone(), policy, {
        let pg = pg.clone();
        let cfg = cfg.clone();
        let archive = rpc.archive.clone();
        let metrics = metrics.clone();
        let include_tentative = follow.include_tentative;
        let sd = shutdown_rx.clone();
        move || {
            staker_loop(
                pg.clone(),
                cfg.clone(),
                archive.clone(),
                metrics.clone(),
                sd.clone(),
                include_tentative,
            )
        }
    }));
    set.spawn(supervise("matview", metrics.clone(), shutdown_rx.clone(), policy, {
        let pg = pg.clone();
        let metrics = metrics.clone();
        let sd = shutdown_rx.clone();
        let interval = follow.matview_refresh_secs;
        move || matview_refresh_loop(pg.clone(), metrics.clone(), sd.clone(), interval)
    }));

    // Supervisors only return on shutdown. A supervisor *panic* means a loop is
    // now unsupervised — treat it as fatal so the process exits and Docker
    // restarts the container.
    while let Some(res) = set.join_next().await {
        match res {
            Ok(()) => {}
            Err(join_err) if join_err.is_panic() => {
                let _ = shutdown_tx.send(true);
                return Err(anyhow::anyhow!("supervisor task panicked: {join_err}"));
            }
            Err(_) => {}
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
    mut shutdown: watch::Receiver<bool>,
    interval_secs: u64,
) -> Result<()> {
    /// (matview name, source table) — source name is informational only,
    /// used for log/metric labels and to keep this list grep-able.
    const VIEWS: &[&str] = &["broadcaster_profile", "orchestrator_profile"];

    loop {
        if *shutdown.borrow() {
            break;
        }
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = sleep(Duration::from_secs(interval_secs)) => {}
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
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    loop {
        if *shutdown.borrow() {
            break;
        }
        tokio::select! {
            _ = shutdown.changed() => break,
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
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    loop {
        if *shutdown.borrow() {
            break;
        }
        tokio::select! {
            _ = shutdown.changed() => break,
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
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    loop {
        if *shutdown.borrow() {
            break;
        }
        tokio::select! {
            _ = shutdown.changed() => break,
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
    mut shutdown: watch::Receiver<bool>,
    follow: FollowConfig,
) -> Result<()> {
    loop {
        if *shutdown.borrow() {
            break;
        }
        tokio::select! {
            _ = shutdown.changed() => break,
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
    mut shutdown: watch::Receiver<bool>,
    include_tentative: bool,
) -> Result<()> {
    loop {
        if *shutdown.borrow() {
            break;
        }
        tokio::select! {
            _ = shutdown.changed() => break,
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
            staker_runner::run_refresh_current_stake(
                &pg,
                archive.as_ref(),
                &cfg,
                include_tentative,
            ),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::Metrics;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};

    type BoxFut = Pin<Box<dyn Future<Output = Result<()>> + Send>>;

    fn fast_policy(max_consecutive: u32) -> RestartPolicy {
        RestartPolicy {
            base_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(5),
            max_consecutive,
        }
    }

    /// An inner "loop" that beats then waits for shutdown, mirroring the real
    /// loops (it returns Ok only when shutdown is observed).
    fn running_loop(
        metrics: Arc<Metrics>,
        task: &'static str,
        mut sd: watch::Receiver<bool>,
    ) -> BoxFut {
        Box::pin(async move {
            loop {
                if *sd.borrow() {
                    return Ok(());
                }
                metrics.beat(task);
                tokio::select! {
                    _ = sd.changed() => return Ok(()),
                    _ = sleep(Duration::from_millis(1)) => {}
                }
            }
        })
    }

    #[tokio::test]
    async fn restarts_on_error_and_does_not_escalate_after_progress() {
        let metrics = Arc::new(Metrics::new());
        let (tx, rx) = watch::channel(false);
        let attempts = Arc::new(AtomicUsize::new(0));

        let sup = tokio::spawn(supervise("indexer", metrics.clone(), rx.clone(), fast_policy(10), {
            let metrics = metrics.clone();
            let attempts = attempts.clone();
            let rx = rx.clone();
            move || -> BoxFut {
                let n = attempts.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Box::pin(async { Err(anyhow::anyhow!("boom")) })
                } else {
                    running_loop(metrics.clone(), "indexer", rx.clone())
                }
            }
        }));

        sleep(Duration::from_millis(200)).await;
        assert!(
            metrics
                .task_restarts_total
                .with_label_values(&["indexer", "error"])
                .get()
                >= 2
        );
        assert!(metrics.heartbeat("indexer") > 0, "running loop should beat");
        assert_eq!(metrics.task_up_value("indexer"), 1, "should not have escalated");

        tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(2), sup)
            .await
            .expect("supervise should stop on shutdown")
            .unwrap();
    }

    #[tokio::test]
    async fn panics_are_caught_and_escalate() {
        let metrics = Arc::new(Metrics::new());
        let (tx, rx) = watch::channel(false);

        let sup = tokio::spawn(supervise(
            "reorg",
            metrics.clone(),
            rx.clone(),
            fast_policy(3),
            move || -> BoxFut { Box::pin(async { panic!("kaboom") }) },
        ));

        sleep(Duration::from_millis(200)).await;
        assert!(
            metrics
                .task_restarts_total
                .with_label_values(&["reorg", "panic"])
                .get()
                >= 3
        );
        assert_eq!(
            metrics.task_up_value("reorg"),
            0,
            "should escalate after exceeding the restart budget"
        );

        tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(2), sup)
            .await
            .expect("supervise should stop on shutdown")
            .unwrap();
    }

    #[tokio::test]
    async fn stops_on_shutdown_without_restart() {
        let metrics = Arc::new(Metrics::new());
        let (tx, rx) = watch::channel(false);
        let sup = tokio::spawn(supervise("staker", metrics.clone(), rx.clone(), fast_policy(10), {
            let metrics = metrics.clone();
            let rx = rx.clone();
            move || running_loop(metrics.clone(), "staker", rx.clone())
        }));

        sleep(Duration::from_millis(50)).await;
        tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(2), sup)
            .await
            .expect("supervise should stop")
            .unwrap();
        assert_eq!(
            metrics
                .task_restarts_total
                .with_label_values(&["staker", "error"])
                .get(),
            0,
            "a cleanly-shutdown loop must not be counted as a restart"
        );
    }
}
