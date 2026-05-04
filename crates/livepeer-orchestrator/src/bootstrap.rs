use crate::{run_migrations, resolve_to_block, Runtime};
use anyhow::{Context, Result};
use livepeer_indexer::{backfill::ContractKind, runner as indexer_runner};
use livepeer_seed_migrator::runner as seed_runner;
use livepeer_staker::runner as staker_runner;
use livepeer_valuator::runner as valuator_runner;
use tracing::info;

#[derive(Debug)]
pub struct BootstrapOpts {
    pub from_block: Option<u64>,
    pub to_block: Option<u64>,
    pub version: Option<String>,
    pub include_tentative: bool,
    pub no_resume: bool,
    pub skip_seed_import: bool,
    pub skip_cross_check: bool,
}

pub async fn run(rt: &Runtime, opts: BootstrapOpts) -> Result<()> {
    run_migrations(&rt.pg).await?;
    seed_runner::seed_abi_registry(&rt.pg, std::path::Path::new("abi"), &rt.cfg).await?;

    if !opts.skip_seed_import {
        if let Some(source_sqlite) = &rt.source_sqlite {
            let summary = seed_runner::run_import(&rt.pg, source_sqlite).await?;
            info!(?summary, "bootstrap: seed import complete");
        }
    }

    let from_block = opts
        .from_block
        .unwrap_or(rt.cfg.static_.chain.livepeer_arbitrum_genesis_block);
    let to_block = resolve_to_block(&rt.archive, opts.to_block).await?;
    let version = opts
        .version
        .clone()
        .unwrap_or_else(|| rt.cfg.static_.pricing.default_valuation_version.clone());

    for contract in [
        ContractKind::BondingManager,
        ContractKind::TicketBroker,
        ContractKind::LivepeerToken,
        ContractKind::RoundsManager,
        ContractKind::Governor,
    ] {
        let summary = indexer_runner::run_backfill(
            &rt.pg,
            &rt.archive,
            &rt.cfg,
            contract,
            from_block,
            to_block,
            opts.no_resume,
            "",
        )
        .await?;
        info!(?summary, "bootstrap: indexer contract complete");
    }

    let l1 = rt
        .l1
        .as_ref()
        .context("bootstrap requires configured L1 provider for finality")?;
    let finality = livepeer_finality_watcher::runner::run_once(&rt.pg, l1).await?;
    info!(?finality, "bootstrap: finality pass complete");

    let valuation = valuator_runner::run_all(
        &rt.pg,
        &rt.archive,
        &rt.cfg,
        &version,
        opts.include_tentative,
    )
    .await?;
    info!(?valuation, "bootstrap: valuator complete");

    let stake = staker_runner::run_backfill(&rt.pg, opts.include_tentative).await?;
    info!(?stake, "bootstrap: staker flow complete");
    let pending =
        staker_runner::run_refresh_pending(&rt.pg, &rt.archive, &rt.cfg, opts.include_tentative)
            .await?;
    info!(?pending, "bootstrap: staker pending complete");

    if !opts.skip_cross_check {
        if let Some(source_sqlite) = &rt.source_sqlite {
            let report = seed_runner::run_cross_check(&rt.pg, source_sqlite).await?;
            info!(?report, "bootstrap: cross-check complete");
        }
    }
    Ok(())
}
