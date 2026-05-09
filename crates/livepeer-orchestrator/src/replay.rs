use crate::{reset, resolve_to_block, run_migrations, Runtime};
use anyhow::{bail, Context, Result};
use livepeer_indexer::{backfill::ContractKind, runner as indexer_runner};
use livepeer_rollups::runner as rollup_runner;
use livepeer_seed_migrator::runner as seed_runner;
use livepeer_staker::runner as staker_runner;
use livepeer_valuator::runner as valuator_runner;
use sqlx::Row;
use tracing::info;

#[derive(Debug)]
pub struct ReplayOpts {
    pub from_block: Option<u64>,
    pub to_block: Option<u64>,
    pub version: Option<String>,
    pub include_tentative: bool,
    pub keep_raw_events: bool,
    pub allow_live_rpc: bool,
    pub skip_seed_import: bool,
    pub skip_cross_check: bool,
}

pub async fn run(rt: &Runtime, opts: ReplayOpts) -> Result<()> {
    if opts.to_block.is_none() {
        bail!("replay requires explicit --to-block so it does not depend on live head state");
    }
    run_migrations(&rt.pg).await?;
    seed_runner::seed_abi_registry(&rt.pg, std::path::Path::new("abi"), &rt.cfg).await?;
    reset::truncate_for_replay(&rt.pg, opts.keep_raw_events).await?;

    if !opts.skip_seed_import {
        if let Some(source_sqlite) = &rt.source_sqlite {
            let summary = seed_runner::run_import(&rt.pg, source_sqlite).await?;
            info!(?summary, "replay: seed import complete");
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

    if !opts.keep_raw_events {
        for contract in [
            ContractKind::BondingManager,
            ContractKind::TicketBroker,
            ContractKind::LivepeerToken,
            ContractKind::RoundsManager,
            ContractKind::Governor,
        ] {
            let contract_address = match contract {
                ContractKind::BondingManager => &rt.cfg.static_.contracts.bonding_manager,
                ContractKind::TicketBroker => &rt.cfg.static_.contracts.ticket_broker,
                ContractKind::LivepeerToken => &rt.cfg.static_.contracts.livepeer_token,
                ContractKind::RoundsManager => &rt.cfg.static_.contracts.rounds_manager,
                ContractKind::Governor => &rt.cfg.static_.contracts.governor,
            };
            if !has_cached_logs_for_contract(&rt.pg, contract_address).await? {
                info!(
                    contract = contract.name(),
                    address = %contract_address,
                    "replay: skipping contract because fixture cache contains no eth_getLogs rows for address"
                );
                continue;
            }
            let summary = indexer_runner::run_backfill(
                &rt.pg,
                &rt.archive,
                &rt.cfg,
                contract,
                from_block,
                to_block,
                true,
                "",
            )
            .await?;
            info!(?summary, "replay: indexer contract complete");
        }
    }

    let finality = if opts.allow_live_rpc {
        let l1 = rt
            .l1
            .as_ref()
            .context("replay with --allow-live-rpc requires configured L1 provider for finality")?;
        livepeer_finality_watcher::runner::run_once(&rt.pg, l1).await?
    } else {
        livepeer_finality_watcher::runner::run_once_replay(&rt.pg).await?
    };
    info!(?finality, "replay: finality pass complete");

    let valuation = valuator_runner::run_all(
        &rt.pg,
        &rt.archive,
        &rt.cfg,
        &version,
        opts.include_tentative,
    )
    .await?;
    info!(?valuation, "replay: valuator complete");

    let stake = staker_runner::run_backfill(&rt.pg, opts.include_tentative).await?;
    info!(?stake, "replay: staker flow complete");
    let pending =
        staker_runner::run_refresh_pending(&rt.pg, &rt.archive, &rt.cfg, opts.include_tentative)
            .await?;
    info!(?pending, "replay: staker pending complete");
    let profile =
        staker_runner::run_profile_backfill(&rt.pg, &rt.archive, &rt.cfg, opts.include_tentative)
            .await?;
    info!(?profile, "replay: staker profile complete");
    let payout_rollup =
        rollup_runner::run_orch_payouts_daily(&rt.pg, opts.include_tentative, 10_000).await?;
    info!(?payout_rollup, "replay: orch payouts rollup complete");
    let rewards_rollup =
        rollup_runner::run_orch_rewards_daily(&rt.pg, opts.include_tentative, 10_000).await?;
    info!(?rewards_rollup, "replay: orch rewards rollup complete");
    let tickets_rollup =
        rollup_runner::run_tickets_daily(&rt.pg, opts.include_tentative, 10_000).await?;
    info!(?tickets_rollup, "replay: tickets daily rollup complete");

    if !opts.skip_cross_check {
        if let Some(source_sqlite) = &rt.source_sqlite {
            let report = seed_runner::run_cross_check(&rt.pg, source_sqlite).await?;
            info!(?report, "replay: cross-check complete");
        }
    }

    // TD-025/TD-026: orchestrator_profile and broadcaster_profile are
    // matviews. Replay rebuilt their source tables; refresh now so
    // post-replay state is observable. In live mode the daemon's
    // matview-refresh loop handles this every 30 s; replay has no daemon.
    reset::refresh_derived_matviews(&rt.pg).await?;
    info!("replay: matviews refreshed");

    Ok(())
}

async fn has_cached_logs_for_contract(pg: &sqlx::PgPool, address: &str) -> Result<bool> {
    let row = sqlx::query(
        r#"SELECT EXISTS(
               SELECT 1
                 FROM rpc_call_cache
                WHERE method = 'eth_getLogs'
                  AND lower(params->0->>'address') = $1
           ) AS exists"#,
    )
    .bind(address.to_lowercase())
    .fetch_one(pg)
    .await?;
    Ok(row.get::<bool, _>("exists"))
}
