use crate::event_metrics::{self, EventMetricsSummary};
use crate::orch_payouts::{self, OrchPayoutsSummary};
use crate::orch_rewards::{self, OrchRewardsSummary};
use crate::tickets::{self, TicketsSummary};
use anyhow::Result;
use sqlx::PgPool;

pub async fn run_event_metrics_daily(
    pg: &PgPool,
    include_tentative: bool,
    batch_limit: i64,
) -> Result<EventMetricsSummary> {
    event_metrics::run_once(pg, include_tentative, batch_limit).await
}

pub async fn run_orch_payouts_daily(
    pg: &PgPool,
    include_tentative: bool,
    batch_limit: i64,
) -> Result<OrchPayoutsSummary> {
    orch_payouts::run_once(pg, include_tentative, batch_limit).await
}

pub async fn run_orch_rewards_daily(
    pg: &PgPool,
    include_tentative: bool,
    batch_limit: i64,
) -> Result<OrchRewardsSummary> {
    orch_rewards::run_once(pg, include_tentative, batch_limit).await
}

pub async fn run_tickets_daily(
    pg: &PgPool,
    include_tentative: bool,
    batch_limit: i64,
) -> Result<TicketsSummary> {
    tickets::run_once(pg, include_tentative, batch_limit).await
}
