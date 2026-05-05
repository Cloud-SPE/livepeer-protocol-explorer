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

pub async fn run_backfill(
    pg: &PgPool,
    archive: &Provider,
    cfg: &Config,
    contract: ContractKind,
    from_block: u64,
    to_block: u64,
    no_resume: bool,
    checkpoint_suffix: &str,
) -> Result<BackfillRunSummary> {
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
        backfill::drive_backfill(
            pg,
            archive,
            contract,
            checkpoint_suffix,
            &proxy,
            &abi_hash,
            actual_from,
            to_block,
        )
        .await?
    };

    Ok(BackfillRunSummary {
        contract_name: contract.name(),
        requested_from: from_block,
        actual_from,
        to_block,
        inner,
    })
}
