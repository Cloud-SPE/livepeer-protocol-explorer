use crate::{
    multi_asset::{self, MultiAssetSummary},
    onchain::{self, LptRunSummary, OnChainRunSummary},
    seed::{self, SeedRunSummary},
};
use anyhow::Result;
use livepeer_core::{config::Config, rpc::Provider};
use sqlx::PgPool;

#[derive(Debug)]
pub struct BackfillAllSummary {
    pub seed: SeedRunSummary,
    pub eth: OnChainRunSummary,
    pub lpt: LptRunSummary,
    pub multi_asset: MultiAssetSummary,
}

pub async fn run_seed(
    pg: &PgPool,
    valuation_version: &str,
    include_tentative: bool,
) -> Result<SeedRunSummary> {
    seed::run_seed_pass(pg, valuation_version, include_tentative).await
}

pub async fn run_eth(
    pg: &PgPool,
    archive: &Provider,
    cfg: &Config,
    valuation_version: &str,
    include_tentative: bool,
) -> Result<OnChainRunSummary> {
    onchain::run_onchain_pass_eth(pg, archive, cfg, valuation_version, include_tentative).await
}

pub async fn run_lpt(
    pg: &PgPool,
    archive: &Provider,
    cfg: &Config,
    valuation_version: &str,
    include_tentative: bool,
) -> Result<LptRunSummary> {
    onchain::run_onchain_pass_lpt(pg, archive, cfg, valuation_version, include_tentative).await
}

pub async fn run_multi_asset(
    pg: &PgPool,
    archive: &Provider,
    cfg: &Config,
    valuation_version: &str,
    include_tentative: bool,
) -> Result<MultiAssetSummary> {
    multi_asset::run_multi_asset_pass(pg, archive, cfg, valuation_version, include_tentative)
        .await
}

pub async fn run_all(
    pg: &PgPool,
    archive: &Provider,
    cfg: &Config,
    valuation_version: &str,
    include_tentative: bool,
) -> Result<BackfillAllSummary> {
    let seed = run_seed(pg, valuation_version, include_tentative).await?;
    let eth = run_eth(pg, archive, cfg, valuation_version, include_tentative).await?;
    let lpt = run_lpt(pg, archive, cfg, valuation_version, include_tentative).await?;
    let multi_asset =
        run_multi_asset(pg, archive, cfg, valuation_version, include_tentative).await?;
    Ok(BackfillAllSummary {
        seed,
        eth,
        lpt,
        multi_asset,
    })
}
