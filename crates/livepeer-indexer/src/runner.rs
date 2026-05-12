use crate::backfill::{self, ContractKind, DriveSummary};
use anyhow::{Context, Result};
use livepeer_core::{config::Config, rpc::Provider};
use sqlx::PgPool;

#[derive(Debug)]
pub struct BackfillRunSummary {
    pub contract_name: &'static str,
    pub requested_from: u64,
    pub actual_from: u64,
    pub to_block: u64,
    pub inner: DriveSummary,
}

/// All inputs to a single `run_backfill` invocation. Grouped so the entry-point
/// has a small signature even though backfill needs DB + archive + config + range.
pub struct RunBackfillArgs<'a> {
    pub pg: &'a PgPool,
    pub archive: &'a Provider,
    pub cfg: &'a Config,
    pub contract: ContractKind,
    pub from_block: u64,
    pub to_block: u64,
    pub no_resume: bool,
    pub checkpoint_suffix: &'a str,
}

pub async fn run_backfill(args: RunBackfillArgs<'_>) -> Result<BackfillRunSummary> {
    let RunBackfillArgs {
        pg,
        archive,
        cfg,
        contract,
        from_block,
        to_block,
        no_resume,
        checkpoint_suffix,
    } = args;
    let proxy = match contract {
        ContractKind::BondingManager => &cfg.static_.contracts.bonding_manager,
        ContractKind::TicketBroker => &cfg.static_.contracts.ticket_broker,
        ContractKind::LivepeerToken => &cfg.static_.contracts.livepeer_token,
        ContractKind::RoundsManager => &cfg.static_.contracts.rounds_manager,
        ContractKind::Governor => &cfg.static_.contracts.governor,
    }
    .to_lowercase();

    let abi_hash: String =
        sqlx::query_scalar("SELECT abi_hash FROM contract_abi_registry WHERE contract_name = $1")
            .bind(contract.name())
            .fetch_one(pg)
            .await
            .with_context(|| format!("loading {} abi_hash from registry", contract.name()))?;

    let actual_from = if no_resume {
        from_block
    } else {
        backfill::resume_from(pg, contract, checkpoint_suffix, from_block).await?
    };

    let inner = if actual_from > to_block {
        DriveSummary::default()
    } else {
        let job = backfill::BackfillJob {
            pg,
            archive,
            contract,
            suffix: checkpoint_suffix,
            proxy_address: &proxy,
            abi_hash: &abi_hash,
        };
        backfill::drive_backfill(&job, actual_from, to_block).await?
    };

    Ok(BackfillRunSummary {
        contract_name: contract.name(),
        requested_from: from_block,
        actual_from,
        to_block,
        inner,
    })
}
