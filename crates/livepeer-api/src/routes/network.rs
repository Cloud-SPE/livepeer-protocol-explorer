//! Network-level endpoints (TD-027).
//!
//! - `GET /network/stats` — single dashboard payload (TVL, active orchs,
//!   24h payouts/rewards, distinct gateways, recent gas burn).
//! - `GET /rounds/{round_id}` — single-round summary using the per-round
//!   snapshots in `orch_stake_by_round` plus the daily rollups bucketed
//!   by the round's date.

use crate::{error::ApiError, state::AppState};
use axum::{
    extract::{Path, State},
    Json,
};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::Row;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Network-level snapshot for a frontend dashboard.")]
pub struct NetworkStatsResponse {
    pub chain_id: String,
    /// Latest known round id (max(round) in `orch_stake_by_round`).
    pub latest_round: Option<String>,
    /// NewRound block that started the latest known round.
    pub latest_round_started_block: Option<String>,
    /// Timestamp of the NewRound block that started the latest known round.
    pub latest_round_started_at: Option<DateTime<Utc>>,
    /// Number of orchestrators flagged active in the latest round.
    pub active_orchestrators: u32,
    /// Total LPT bonded across all orchs (sum of latest total_stake per orch).
    pub total_lpt_staked: String,
    /// Number of distinct gateways with a snapshot in
    /// `gateway_balances_by_block`.
    pub gateways_known: u32,
    /// Sum of orch commission earned in the trailing 24 h, in USD.
    pub payouts_usd_24h: String,
    /// Sum of orch reward share in the trailing 24 h, in USD.
    pub rewards_usd_24h: String,
    /// Total tx fees burned (native ETH) across all observed Livepeer
    /// transactions in the trailing 24 h.
    pub gas_burned_eth_24h: String,
    /// Snapshot freshness (when this matview last refreshed).
    pub orchestrator_profile_refreshed_at: Option<DateTime<Utc>>,
    pub broadcaster_profile_refreshed_at: Option<DateTime<Utc>>,
    /// Number of delegators flagged active in `delegator_registry`.
    pub active_delegators: u32,
    /// Total live delegations: distinct `(delegator, delegate)` pairs whose
    /// latest `stake_balances_by_block` snapshot has `bonded_principal > 0`.
    pub total_delegations: u32,
}

#[utoipa::path(
    get,
    path = "/network/stats",
    tag = "Network",
    responses(
        (status = 200, description = "Single-payload network dashboard.", body = NetworkStatsResponse),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn stats(
    State(state): State<AppState>,
) -> Result<Json<NetworkStatsResponse>, ApiError> {
    let row = sqlx::query(
        r#"
        WITH t AS (SELECT $1::bigint AS cid, NOW() - INTERVAL '24 hours' AS cutoff_24h)
        SELECT
          (SELECT MAX(round) FROM orch_stake_by_round WHERE chain_id = t.cid) AS latest_round,
          (SELECT block_number
             FROM orch_stake_by_round
            WHERE chain_id = t.cid
              AND round = (SELECT MAX(round) FROM orch_stake_by_round WHERE chain_id = t.cid)
            ORDER BY address ASC
            LIMIT 1) AS latest_round_started_block,
          (SELECT block_timestamp
             FROM orch_stake_by_round
            WHERE chain_id = t.cid
              AND round = (SELECT MAX(round) FROM orch_stake_by_round WHERE chain_id = t.cid)
            ORDER BY address ASC
            LIMIT 1) AS latest_round_started_at,
          (SELECT COUNT(*) FROM orchestrator_profile WHERE chain_id = t.cid AND is_active) AS active_orchs,
          (SELECT COALESCE(SUM(total_stake), 0) FROM orchestrator_profile WHERE chain_id = t.cid) AS total_stake,
          (SELECT COUNT(*) FROM broadcaster_profile WHERE chain_id = t.cid) AS gateways_known,
          (SELECT COALESCE(SUM(sum_commission_usd), 0)
             FROM orch_payouts_daily
            WHERE chain_id = t.cid AND day_utc >= (NOW() - INTERVAL '1 day')::date) AS payouts_usd_24h,
          (SELECT COALESCE(SUM(sum_orch_tokens_usd), 0)
             FROM orch_rewards_daily
            WHERE chain_id = t.cid AND day_utc >= (NOW() - INTERVAL '1 day')::date) AS rewards_usd_24h,
          (SELECT COALESCE(SUM(tx_fee_eth), 0)
             FROM tx_receipts
            WHERE chain_id = t.cid AND block_timestamp >= t.cutoff_24h) AS gas_burned_eth_24h,
          (SELECT MAX(updated_at) FROM orchestrator_profile WHERE chain_id = t.cid) AS orch_refreshed,
          (SELECT MAX(updated_at) FROM broadcaster_profile WHERE chain_id = t.cid) AS gw_refreshed,
          (SELECT COUNT(*) FROM delegator_registry WHERE chain_id = t.cid AND is_active) AS active_delegators,
          (SELECT COUNT(*)
             FROM (SELECT DISTINCT ON (delegator_address, delegate_address) bonded_principal
                     FROM stake_balances_by_block
                    WHERE chain_id = t.cid
                    ORDER BY delegator_address, delegate_address, block_number DESC) latest
            WHERE latest.bonded_principal > 0) AS total_delegations
        FROM t
        "#,
    )
    .bind(state.chain_id)
    .fetch_one(&state.pg)
    .await?;

    let total_stake: BigDecimal = row.get("total_stake");
    let payouts_usd: BigDecimal = row.get("payouts_usd_24h");
    let rewards_usd: BigDecimal = row.get("rewards_usd_24h");
    let gas_eth: BigDecimal = row.get("gas_burned_eth_24h");

    Ok(Json(NetworkStatsResponse {
        chain_id: state.chain_id.to_string(),
        latest_round: row
            .try_get::<Option<i64>, _>("latest_round")
            .ok()
            .flatten()
            .map(|v| v.to_string()),
        latest_round_started_block: row
            .try_get::<Option<i64>, _>("latest_round_started_block")
            .ok()
            .flatten()
            .map(|v| v.to_string()),
        latest_round_started_at: row.try_get("latest_round_started_at").ok(),
        active_orchestrators: row.get::<i64, _>("active_orchs") as u32,
        total_lpt_staked: total_stake.normalized().to_string(),
        gateways_known: row.get::<i64, _>("gateways_known") as u32,
        payouts_usd_24h: payouts_usd.normalized().to_string(),
        rewards_usd_24h: rewards_usd.normalized().to_string(),
        gas_burned_eth_24h: gas_eth.normalized().to_string(),
        orchestrator_profile_refreshed_at: row.try_get("orch_refreshed").ok(),
        broadcaster_profile_refreshed_at: row.try_get("gw_refreshed").ok(),
        active_delegators: row.get::<i64, _>("active_delegators") as u32,
        total_delegations: row.get::<i64, _>("total_delegations") as u32,
    }))
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "One orchestrator's contribution within a round summary.")]
pub struct RoundOrchSummary {
    pub address: String,
    pub total_stake: String,
    pub fee_cut_percent: String,
    pub reward_cut_percent: String,
    pub fee_share_percent: String,
    pub is_active: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Single-round summary built from per-round snapshots + daily rollups bucketed by round date.")]
pub struct RoundSummaryResponse {
    pub round: String,
    pub round_started_block: String,
    pub round_started_at: DateTime<Utc>,
    pub active_orchestrators: u32,
    pub total_lpt_staked: String,
    /// Orchs sorted by total_stake DESC, capped at 10 for the summary view.
    pub top_orchs: Vec<RoundOrchSummary>,
    /// Aggregated payouts on the day this round started (USD).
    pub payouts_usd_on_day: String,
    /// Aggregated rewards on the day this round started (USD).
    pub rewards_usd_on_day: String,
    /// Number of NewRound events that fired with this round id (typically 1).
    pub new_round_events: u32,
}

#[utoipa::path(
    get,
    path = "/rounds/{round_id}",
    tag = "Network",
    params(
        ("round_id" = i64, Path, description = "Round id (e.g. 3500).")
    ),
    responses(
        (status = 200, description = "Round summary.", body = RoundSummaryResponse),
        (status = 404, description = "Round id not present in orch_stake_by_round.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn round_get(
    State(state): State<AppState>,
    Path(round_id): Path<i64>,
) -> Result<Json<RoundSummaryResponse>, ApiError> {
    // Pick the orch_stake_by_round rows for this round to drive the snapshot.
    // block_number + block_timestamp are constant across all rows for a
    // given (chain, round) — they reflect the NewRound block.
    let rows = sqlx::query(
        r#"SELECT address,
                  block_number,
                  block_timestamp,
                  total_stake,
                  latest_fee_cut_percent,
                  latest_reward_cut_percent,
                  latest_fee_share_percent,
                  is_active
             FROM orch_stake_by_round
            WHERE chain_id = $1 AND round = $2
         ORDER BY total_stake DESC"#,
    )
    .bind(state.chain_id)
    .bind(round_id)
    .fetch_all(&state.pg)
    .await?;

    if rows.is_empty() {
        return Err(ApiError::not_found("round not found in orch_stake_by_round"));
    }

    let block_number: i64 = rows[0].get("block_number");
    let block_timestamp: DateTime<Utc> = rows[0].get("block_timestamp");
    let active_count = rows.iter().filter(|r| r.get::<bool, _>("is_active")).count() as u32;
    let total_stake: BigDecimal = rows
        .iter()
        .map(|r| r.get::<BigDecimal, _>("total_stake"))
        .fold(BigDecimal::from(0), |acc, x| acc + x);

    let top_orchs: Vec<RoundOrchSummary> = rows
        .iter()
        .take(10)
        .map(|r| RoundOrchSummary {
            address: r.get("address"),
            total_stake: r
                .get::<BigDecimal, _>("total_stake")
                .normalized()
                .to_string(),
            fee_cut_percent: r
                .get::<BigDecimal, _>("latest_fee_cut_percent")
                .normalized()
                .to_string(),
            reward_cut_percent: r
                .get::<BigDecimal, _>("latest_reward_cut_percent")
                .normalized()
                .to_string(),
            fee_share_percent: r
                .get::<BigDecimal, _>("latest_fee_share_percent")
                .normalized()
                .to_string(),
            is_active: r.get("is_active"),
        })
        .collect();

    // Daily rollups bucket on day_utc — use the round's date.
    let agg = sqlx::query(
        r#"
        SELECT
          COALESCE((SELECT SUM(sum_commission_usd) FROM orch_payouts_daily
                     WHERE chain_id = $1 AND day_utc = ($2::timestamptz)::date), 0) AS payouts_usd,
          COALESCE((SELECT SUM(sum_orch_tokens_usd) FROM orch_rewards_daily
                     WHERE chain_id = $1 AND day_utc = ($2::timestamptz)::date), 0) AS rewards_usd
        "#,
    )
    .bind(state.chain_id)
    .bind(block_timestamp)
    .fetch_one(&state.pg)
    .await?;

    let payouts_usd: BigDecimal = agg.get("payouts_usd");
    let rewards_usd: BigDecimal = agg.get("rewards_usd");

    let new_round_count: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM raw_protocol_events
            WHERE chain_id = $1
              AND is_canonical = TRUE
              AND event_name = 'NewRound'
              AND (raw_event -> 'decoded' ->> 'round')::bigint = $2"#,
    )
    .bind(state.chain_id)
    .bind(round_id)
    .fetch_one(&state.pg)
    .await
    .unwrap_or(0);

    Ok(Json(RoundSummaryResponse {
        round: round_id.to_string(),
        round_started_block: block_number.to_string(),
        round_started_at: block_timestamp,
        active_orchestrators: active_count,
        total_lpt_staked: total_stake.normalized().to_string(),
        top_orchs,
        payouts_usd_on_day: payouts_usd.normalized().to_string(),
        rewards_usd_on_day: rewards_usd.normalized().to_string(),
        new_round_events: new_round_count as u32,
    }))
}
