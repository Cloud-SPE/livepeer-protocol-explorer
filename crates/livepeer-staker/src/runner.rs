use crate::{
    flow::{self, FlowSummary},
    gateway::{self, GatewayBackfillSummary},
    pending::{self, PendingSummary},
};
use anyhow::Result;
use livepeer_core::{config::Config, rpc::Provider};
use sqlx::PgPool;

pub async fn run_backfill(pg: &PgPool, include_tentative: bool) -> Result<FlowSummary> {
    flow::run_flow_backfill(pg, include_tentative).await
}

pub async fn run_gateway_backfill(
    pg: &PgPool,
    archive: &Provider,
    cfg: &Config,
    include_tentative: bool,
) -> Result<GatewayBackfillSummary> {
    gateway::run_gateway_backfill(pg, archive, cfg, include_tentative).await
}

pub async fn run_refresh_pending(
    pg: &PgPool,
    archive: &Provider,
    cfg: &Config,
    include_tentative: bool,
) -> Result<PendingSummary> {
    let bonding_manager = cfg.static_.contracts.bonding_manager.to_lowercase();
    pending::refresh_pending(pg, archive, &bonding_manager, include_tentative).await
}
