use crate::rpc_manager::RpcManager;
use anyhow::{bail, Result};
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
const INDEXER_HEAD_DEPTH_BLOCKS: u64 = 10;
const INDEXER_PER_TICK_BLOCKS: u64 = 1_000;

#[derive(Clone, Debug)]
pub struct FollowConfig {
    pub max_start_lag_blocks: u64,
    pub valuation_version: String,
    pub include_tentative: bool,
}

pub async fn run_follow(pg: &PgPool, cfg: &Config, rpc: RpcManager, follow: FollowConfig) -> Result<()> {
    let head = rpc.archive.eth_block_number().await?;
    let lag = current_indexer_lag(pg, head).await?;
    if lag > follow.max_start_lag_blocks {
        bail!(
            "follow-mode startup refused: current lag {lag} exceeds max_start_lag_blocks {}",
            follow.max_start_lag_blocks
        );
    }
    info!(head, lag, max_start_lag_blocks = follow.max_start_lag_blocks, "follow-mode startup gate passed");

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
        shutdown.clone(),
    ));
    let finality_task = tokio::spawn(finality_loop(
        pg.clone(),
        rpc.l1.clone(),
        shutdown.clone(),
    ));
    let reorg_task = tokio::spawn(reorg_loop(
        pg.clone(),
        rpc.secondary.clone(),
        shutdown.clone(),
    ));
    let valuator_task = tokio::spawn(valuator_loop(
        pg.clone(),
        cfg.clone(),
        rpc.archive.clone(),
        shutdown.clone(),
        follow.clone(),
    ));
    let staker_task = tokio::spawn(staker_loop(
        pg.clone(),
        cfg.clone(),
        rpc.archive.clone(),
        shutdown.clone(),
        follow.include_tentative,
    ));

    let (a, b, c, d, e) = tokio::join!(
        indexer_task,
        finality_task,
        reorg_task,
        valuator_task,
        staker_task
    );
    for r in [a, b, c, d, e] {
        match r {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

async fn indexer_loop(
    pg: PgPool,
    cfg: Config,
    archive: Arc<livepeer_core::rpc::Provider>,
    shutdown: Arc<Notify>,
) -> Result<()> {
    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            _ = sleep(Duration::from_secs(INDEXER_INTERVAL_SECS)) => {}
        }
        let head = archive.eth_block_number().await?;
        let to_block = head.saturating_sub(INDEXER_HEAD_DEPTH_BLOCKS);
        for contract in [
            ContractKind::BondingManager,
            ContractKind::TicketBroker,
            ContractKind::LivepeerToken,
            ContractKind::RoundsManager,
            ContractKind::Governor,
        ] {
            let from_block = livepeer_indexer::backfill::resume_from(&pg, contract, "", cfg.static_.chain.livepeer_arbitrum_genesis_block).await?;
            if from_block > to_block {
                continue;
            }
            let bounded_to = from_block.saturating_add(INDEXER_PER_TICK_BLOCKS - 1).min(to_block);
            let summary = indexer_runner::run_backfill(
                &pg,
                archive.as_ref(),
                &cfg,
                contract,
                from_block,
                bounded_to,
                true,
                "",
            )
            .await?;
            info!(?summary, "daemon: indexer iteration complete");
        }
    }
    Ok(())
}

async fn finality_loop(
    pg: PgPool,
    l1: Arc<livepeer_core::rpc::Provider>,
    shutdown: Arc<Notify>,
) -> Result<()> {
    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            _ = sleep(Duration::from_secs(FINALITY_INTERVAL_SECS)) => {}
        }
        let summary = finality_runner::run_once(&pg, l1.as_ref()).await?;
        info!(?summary, "daemon: finality iteration complete");
    }
    Ok(())
}

async fn reorg_loop(
    pg: PgPool,
    secondary: Arc<livepeer_core::rpc::Provider>,
    shutdown: Arc<Notify>,
) -> Result<()> {
    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            _ = sleep(Duration::from_secs(REORG_INTERVAL_SECS)) => {}
        }
        let summary = reorg_runner::run_once(&pg, secondary.as_ref()).await?;
        info!(?summary, "daemon: reorg iteration complete");
    }
    Ok(())
}

async fn valuator_loop(
    pg: PgPool,
    cfg: Config,
    archive: Arc<livepeer_core::rpc::Provider>,
    shutdown: Arc<Notify>,
    follow: FollowConfig,
) -> Result<()> {
    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            _ = sleep(Duration::from_secs(VALUATOR_INTERVAL_SECS)) => {}
        }
        let summary = valuator_runner::run_all(
            &pg,
            archive.as_ref(),
            &cfg,
            &follow.valuation_version,
            follow.include_tentative,
        )
        .await?;
        info!(?summary, "daemon: valuator iteration complete");
    }
    Ok(())
}

async fn staker_loop(
    pg: PgPool,
    cfg: Config,
    archive: Arc<livepeer_core::rpc::Provider>,
    shutdown: Arc<Notify>,
    include_tentative: bool,
) -> Result<()> {
    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            _ = sleep(Duration::from_secs(STAKER_INTERVAL_SECS)) => {}
        }
        let backfill = staker_runner::run_backfill(&pg, include_tentative).await?;
        info!(?backfill, "daemon: staker backfill iteration complete");
        let refresh =
            staker_runner::run_refresh_pending(&pg, archive.as_ref(), &cfg, include_tentative)
                .await?;
        info!(?refresh, "daemon: staker refresh iteration complete");
    }
    Ok(())
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
