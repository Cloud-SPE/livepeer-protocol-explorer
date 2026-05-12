use crate::{error::ApiError, state::AppState};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::str::FromStr;
use utoipa::{IntoParams, ToSchema};

const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 1_000;

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
#[schema(description = "Query parameters for listing orchestrator profiles.")]
pub struct OrchestratorsQuery {
    /// Opaque cursor for `(total_stake DESC, address ASC)` pagination.
    pub cursor: Option<String>,
    /// Maximum number of rows to return.
    pub limit: Option<u32>,
    /// If true, include only active orchestrators.
    pub active_only: Option<bool>,
}

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
#[schema(description = "Query parameters for listing gateway profiles.")]
pub struct GatewaysQuery {
    /// Opaque cursor for `(latest_deposit DESC, address ASC)` pagination.
    pub cursor: Option<String>,
    /// Maximum number of rows to return.
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Point-in-time orchestrator profile row.")]
pub struct OrchestratorProfileRow {
    pub address: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub total_stake: String,
    pub fee_cut_percent: String,
    pub fee_share_percent: String,
    pub reward_cut_percent: String,
    pub is_active: bool,
    pub service_uri: Option<String>,
    pub last_lifecycle_event_at: Option<DateTime<Utc>>,
    pub as_of_block: String,
    pub as_of_round: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Point-in-time gateway profile row.")]
pub struct GatewayProfileRow {
    pub address: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub kind: String,
    pub latest_deposit: String,
    pub latest_reserve: String,
    /// TD-025: how much of the reserve has been drained this round, useful
    /// for "is this gateway about to bounce a ticket?" alerts.
    pub reserve_claimed_in_current_round: String,
    /// TD-025: round at which the gateway scheduled their withdrawal
    /// (relevant when `unlock_in_progress = true`).
    pub withdraw_round: String,
    pub unlock_in_progress: bool,
    pub as_of_block: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Pagination/meta block for profile list endpoints.")]
pub struct ProfileListMeta {
    pub chain_id: String,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Paginated orchestrator profile list.")]
pub struct OrchestratorListResponse {
    pub data: Vec<OrchestratorProfileRow>,
    pub meta: ProfileListMeta,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Paginated gateway profile list.")]
pub struct GatewayListResponse {
    pub data: Vec<GatewayProfileRow>,
    pub meta: ProfileListMeta,
}

#[utoipa::path(
    get,
    path = "/orchestrators",
    tag = "Profiles",
    params(OrchestratorsQuery),
    responses(
        (status = 200, description = "Paginated orchestrator profiles ordered by total stake.", body = OrchestratorListResponse),
        (status = 400, description = "Invalid cursor or query parameters.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn orchestrators_list(
    State(state): State<AppState>,
    Query(q): Query<OrchestratorsQuery>,
) -> Result<Json<OrchestratorListResponse>, ApiError> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as i64;
    let cursor = q.cursor.as_deref().map(StakeCursor::decode).transpose()?;
    let active_only = q.active_only.unwrap_or(false);

    let sql = r#"SELECT p.address,
                        COALESCE(o.display_name, e.ens_name) AS display_name,
                        COALESCE(o.avatar_url, e.ens_avatar_url) AS avatar_url,
                        p.total_stake,
                        p.latest_fee_cut_percent,
                        p.latest_fee_share_percent,
                        p.latest_reward_cut_percent,
                        p.is_active,
                        p.service_uri,
                        p.last_lifecycle_event_at,
                        p.as_of_block,
                        p.as_of_round
                   FROM orchestrator_profile p
              LEFT JOIN orchestrator_ens e
                     ON e.chain_id = p.chain_id
                    AND e.address = p.address
              LEFT JOIN name_avatar_overrides o
                     ON o.chain_id = p.chain_id
                    AND o.address = p.address
                  WHERE p.chain_id = $1
                    AND ($2::bool = FALSE OR p.is_active = TRUE)
                    AND ($3::numeric IS NULL OR p.total_stake < $3 OR (p.total_stake = $3 AND p.address > $4))
               ORDER BY p.total_stake DESC, p.address ASC
                  LIMIT $5"#;
    let rows = sqlx::query(sql)
        .bind(state.chain_id)
        .bind(active_only)
        .bind(cursor.as_ref().map(|c| c.value.clone()))
        .bind(cursor.as_ref().map(|c| c.address.clone()))
        .bind(limit)
        .fetch_all(&state.pg)
        .await?;
    let data: Vec<OrchestratorProfileRow> = rows.iter().map(to_orchestrator_row).collect();
    let next_cursor = data.last().map(|r| {
        StakeCursor {
            value: BigDecimal::from_str(&r.total_stake).unwrap_or_default(),
            address: r.address.clone(),
        }
        .encode()
    });
    Ok(Json(OrchestratorListResponse {
        data,
        meta: ProfileListMeta {
            chain_id: state.chain_id.to_string(),
            next_cursor,
        },
    }))
}

#[utoipa::path(
    get,
    path = "/orchestrators/{address}",
    tag = "Profiles",
    params(
        ("address" = String, Path, description = "Orchestrator address.")
    ),
    responses(
        (status = 200, description = "Single orchestrator profile row.", body = OrchestratorProfileRow),
        (status = 404, description = "No orchestrator profile exists for the address.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn orchestrators_get(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Result<Json<OrchestratorProfileRow>, ApiError> {
    let address = normalize_addr(&address)?;
    let row = sqlx::query(
        r#"SELECT p.address,
                  COALESCE(o.display_name, e.ens_name) AS display_name,
                  COALESCE(o.avatar_url, e.ens_avatar_url) AS avatar_url,
                  p.total_stake,
                  p.latest_fee_cut_percent,
                  p.latest_fee_share_percent,
                  p.latest_reward_cut_percent,
                  p.is_active,
                  p.service_uri,
                  p.last_lifecycle_event_at,
                  p.as_of_block,
                  p.as_of_round
             FROM orchestrator_profile p
        LEFT JOIN orchestrator_ens e
               ON e.chain_id = p.chain_id
              AND e.address = p.address
        LEFT JOIN name_avatar_overrides o
               ON o.chain_id = p.chain_id
              AND o.address = p.address
            WHERE p.chain_id = $1
              AND p.address = $2"#,
    )
    .bind(state.chain_id)
    .bind(address)
    .fetch_optional(&state.pg)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::not_found("orchestrator profile not found"));
    };
    Ok(Json(to_orchestrator_row(&row)))
}

#[utoipa::path(
    get,
    path = "/gateways",
    tag = "Profiles",
    params(GatewaysQuery),
    responses(
        (status = 200, description = "Paginated gateway profiles ordered by latest deposit.", body = GatewayListResponse),
        (status = 400, description = "Invalid cursor or query parameters.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn gateways_list(
    State(state): State<AppState>,
    Query(q): Query<GatewaysQuery>,
) -> Result<Json<GatewayListResponse>, ApiError> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as i64;
    let cursor = q.cursor.as_deref().map(StakeCursor::decode).transpose()?;
    let sql = r#"SELECT p.address,
                        COALESCE(o.display_name, e.ens_name) AS display_name,
                        COALESCE(o.avatar_url, e.ens_avatar_url) AS avatar_url,
                        COALESCE(c.kind, 'transcoding') AS kind,
                        p.latest_deposit,
                        p.latest_reserve,
                        p.reserve_claimed_in_current_round,
                        p.withdraw_round,
                        p.unlock_in_progress,
                        p.as_of_block
                   FROM broadcaster_profile p
              LEFT JOIN broadcaster_ens e
                     ON e.chain_id = p.chain_id
                    AND e.address = p.address
              LEFT JOIN name_avatar_overrides o
                     ON o.chain_id = p.chain_id
                    AND o.address = p.address
              LEFT JOIN broadcaster_classifications c
                     ON c.chain_id = p.chain_id
                    AND c.address = p.address
                  WHERE p.chain_id = $1
                    AND ($2::numeric IS NULL OR p.latest_deposit < $2 OR (p.latest_deposit = $2 AND p.address > $3))
               ORDER BY p.latest_deposit DESC, p.address ASC
                  LIMIT $4"#;
    let rows = sqlx::query(sql)
        .bind(state.chain_id)
        .bind(cursor.as_ref().map(|c| c.value.clone()))
        .bind(cursor.as_ref().map(|c| c.address.clone()))
        .bind(limit)
        .fetch_all(&state.pg)
        .await?;
    let data: Vec<GatewayProfileRow> = rows.iter().map(to_gateway_row).collect();
    let next_cursor = data.last().map(|r| {
        StakeCursor {
            value: BigDecimal::from_str(&r.latest_deposit).unwrap_or_default(),
            address: r.address.clone(),
        }
        .encode()
    });
    Ok(Json(GatewayListResponse {
        data,
        meta: ProfileListMeta {
            chain_id: state.chain_id.to_string(),
            next_cursor,
        },
    }))
}

#[utoipa::path(
    get,
    path = "/gateways/{address}/profile",
    tag = "Profiles",
    params(
        ("address" = String, Path, description = "Gateway address.")
    ),
    responses(
        (status = 200, description = "Single gateway profile row.", body = GatewayProfileRow),
        (status = 404, description = "No gateway profile exists for the address.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn gateways_get(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Result<Json<GatewayProfileRow>, ApiError> {
    let address = normalize_addr(&address)?;
    let row = sqlx::query(
        r#"SELECT p.address,
                  COALESCE(o.display_name, e.ens_name) AS display_name,
                  COALESCE(o.avatar_url, e.ens_avatar_url) AS avatar_url,
                  COALESCE(c.kind, 'transcoding') AS kind,
                  p.latest_deposit,
                  p.latest_reserve,
                  p.reserve_claimed_in_current_round,
                  p.withdraw_round,
                  p.unlock_in_progress,
                  p.as_of_block
             FROM broadcaster_profile p
        LEFT JOIN broadcaster_ens e
               ON e.chain_id = p.chain_id
              AND e.address = p.address
        LEFT JOIN name_avatar_overrides o
               ON o.chain_id = p.chain_id
              AND o.address = p.address
        LEFT JOIN broadcaster_classifications c
               ON c.chain_id = p.chain_id
              AND c.address = p.address
            WHERE p.chain_id = $1
              AND p.address = $2"#,
    )
    .bind(state.chain_id)
    .bind(address)
    .fetch_optional(&state.pg)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::not_found("gateway profile not found"));
    };
    Ok(Json(to_gateway_row(&row)))
}

fn to_orchestrator_row(r: &sqlx::postgres::PgRow) -> OrchestratorProfileRow {
    OrchestratorProfileRow {
        address: r.get("address"),
        display_name: r.try_get("display_name").ok(),
        avatar_url: r.try_get("avatar_url").ok(),
        total_stake: r
            .get::<BigDecimal, _>("total_stake")
            .normalized()
            .to_string(),
        fee_cut_percent: r
            .get::<BigDecimal, _>("latest_fee_cut_percent")
            .normalized()
            .to_string(),
        fee_share_percent: r
            .get::<BigDecimal, _>("latest_fee_share_percent")
            .normalized()
            .to_string(),
        reward_cut_percent: r
            .get::<BigDecimal, _>("latest_reward_cut_percent")
            .normalized()
            .to_string(),
        is_active: r.get("is_active"),
        service_uri: r.try_get("service_uri").ok(),
        last_lifecycle_event_at: r.try_get("last_lifecycle_event_at").ok(),
        as_of_block: r.get::<i64, _>("as_of_block").to_string(),
        as_of_round: r
            .try_get::<Option<i64>, _>("as_of_round")
            .ok()
            .flatten()
            .map(|v| v.to_string()),
    }
}

fn to_gateway_row(r: &sqlx::postgres::PgRow) -> GatewayProfileRow {
    GatewayProfileRow {
        address: r.get("address"),
        display_name: r.try_get("display_name").ok(),
        avatar_url: r.try_get("avatar_url").ok(),
        kind: r.get("kind"),
        latest_deposit: r
            .get::<BigDecimal, _>("latest_deposit")
            .normalized()
            .to_string(),
        latest_reserve: r
            .get::<BigDecimal, _>("latest_reserve")
            .normalized()
            .to_string(),
        reserve_claimed_in_current_round: r
            .get::<BigDecimal, _>("reserve_claimed_in_current_round")
            .normalized()
            .to_string(),
        withdraw_round: r.get::<i64, _>("withdraw_round").to_string(),
        unlock_in_progress: r.get("unlock_in_progress"),
        as_of_block: r.get::<i64, _>("as_of_block").to_string(),
    }
}

#[derive(Debug, Clone)]
struct StakeCursor {
    value: BigDecimal,
    address: String,
}

impl StakeCursor {
    fn encode(&self) -> String {
        format!("N{}|{}", self.value.normalized(), self.address)
    }

    fn decode(s: &str) -> Result<Self, ApiError> {
        let stripped = s
            .strip_prefix('N')
            .ok_or_else(|| ApiError::bad_request("invalid cursor"))?;
        let (value, address) = stripped
            .split_once('|')
            .ok_or_else(|| ApiError::bad_request("invalid cursor"))?;
        Ok(Self {
            value: BigDecimal::from_str(value)
                .map_err(|_| ApiError::bad_request("invalid cursor numeric"))?,
            address: normalize_addr(address)?,
        })
    }
}

fn normalize_addr(s: &str) -> Result<String, ApiError> {
    let lower = s.to_lowercase();
    if lower.starts_with("0x") && lower.len() == 42 {
        Ok(lower)
    } else {
        Err(ApiError::bad_request(format!("invalid address: {s}")))
    }
}

// ─────────────────────────────────────────────────────────────────────────
// /orchestrators/{addr}/stake-history (TD-026 powered)
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
#[schema(description = "Range filters for orchestrator stake-history.")]
pub struct StakeHistoryQuery {
    /// Inclusive lower-bound round id. Defaults to (latest_round - 100).
    pub from_round: Option<i64>,
    /// Inclusive upper-bound round id. Defaults to latest_round.
    pub to_round: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "One per-round stake snapshot for an orchestrator.")]
pub struct StakeHistoryPoint {
    pub round: String,
    pub block_number: String,
    pub block_timestamp: DateTime<Utc>,
    pub total_stake: String,
    pub fee_cut_percent: String,
    pub reward_cut_percent: String,
    pub fee_share_percent: String,
    pub is_active: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StakeHistoryResponse {
    pub address: String,
    pub data: Vec<StakeHistoryPoint>,
    pub meta: ProfileListMeta,
}

#[utoipa::path(
    get,
    path = "/orchestrators/{address}/stake-history",
    tag = "Orchestrator history",
    params(
        ("address" = String, Path, description = "Orchestrator address."),
        StakeHistoryQuery
    ),
    responses(
        (status = 200, description = "Per-round stake snapshots from orch_stake_by_round.", body = StakeHistoryResponse),
        (status = 400, description = "Invalid query.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn orchestrators_stake_history(
    State(state): State<AppState>,
    Path(address): Path<String>,
    Query(q): Query<StakeHistoryQuery>,
) -> Result<Json<StakeHistoryResponse>, ApiError> {
    let address = normalize_addr(&address)?;
    let rows = sqlx::query(
        r#"
        WITH latest AS (
          SELECT MAX(round) AS r FROM orch_stake_by_round
           WHERE chain_id = $1 AND address = $2
        )
        SELECT round, block_number, block_timestamp, total_stake,
               latest_fee_cut_percent, latest_reward_cut_percent,
               latest_fee_share_percent, is_active
          FROM orch_stake_by_round
         WHERE chain_id = $1 AND address = $2
           AND round >= COALESCE($3, GREATEST((SELECT r FROM latest) - 100, 0))
           AND round <= COALESCE($4, (SELECT r FROM latest))
         ORDER BY round ASC
        "#,
    )
    .bind(state.chain_id)
    .bind(&address)
    .bind(q.from_round)
    .bind(q.to_round)
    .fetch_all(&state.pg)
    .await?;

    let data: Vec<StakeHistoryPoint> = rows
        .iter()
        .map(|r| StakeHistoryPoint {
            round: r.get::<i64, _>("round").to_string(),
            block_number: r.get::<i64, _>("block_number").to_string(),
            block_timestamp: r.get("block_timestamp"),
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

    Ok(Json(StakeHistoryResponse {
        address,
        data,
        meta: ProfileListMeta {
            chain_id: state.chain_id.to_string(),
            next_cursor: None,
        },
    }))
}

// ─────────────────────────────────────────────────────────────────────────
// /orchestrators/{addr}/cuts-history
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "One TranscoderUpdate event for an orchestrator.")]
pub struct CutsHistoryPoint {
    pub block_number: String,
    pub block_timestamp: DateTime<Utc>,
    pub fee_cut_percent: String,
    pub reward_cut_percent: String,
    pub fee_share_percent: String,
    pub event_id: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CutsHistoryResponse {
    pub address: String,
    pub data: Vec<CutsHistoryPoint>,
    pub meta: ProfileListMeta,
}

#[utoipa::path(
    get,
    path = "/orchestrators/{address}/cuts-history",
    tag = "Orchestrator history",
    params(
        ("address" = String, Path, description = "Orchestrator address.")
    ),
    responses(
        (status = 200, description = "Chronological list of cut changes (TranscoderUpdate events).", body = CutsHistoryResponse),
        (status = 400, description = "Invalid query.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn orchestrators_cuts_history(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Result<Json<CutsHistoryResponse>, ApiError> {
    let address = normalize_addr(&address)?;
    // raw_protocol_events stores the raw decoded fields under raw_event.decoded.
    // rewardCut / feeShare are in raw scaled units (1e6 = 100%); we use the
    // same conversion the staker does to expose human-readable percentages.
    let rows = sqlx::query(
        r#"
        SELECT id,
               block_number,
               block_timestamp,
               (raw_event -> 'decoded' ->> 'rewardCut')::numeric AS reward_cut_raw,
               (raw_event -> 'decoded' ->> 'feeShare')::numeric  AS fee_share_raw
          FROM raw_protocol_events
         WHERE chain_id = $1
           AND is_canonical = TRUE
           AND event_name = 'TranscoderUpdate'
           AND to_address = $2
         ORDER BY block_number ASC, log_index ASC
        "#,
    )
    .bind(state.chain_id)
    .bind(&address)
    .fetch_all(&state.pg)
    .await?;

    let percent_denom = BigDecimal::from(10_000);
    let full_percent = BigDecimal::from(1_000_000);

    let data: Vec<CutsHistoryPoint> = rows
        .iter()
        .map(|r| {
            let reward_raw: BigDecimal = r.try_get("reward_cut_raw").unwrap_or_default();
            let fee_share_raw: BigDecimal = r.try_get("fee_share_raw").unwrap_or_default();
            let reward_cut = (&reward_raw / &percent_denom).normalized().to_string();
            let fee_share = (&fee_share_raw / &percent_denom).normalized().to_string();
            // fee_cut = inverse of fee_share (orch keeps what the gateway
            // doesn't keep, expressed as percentage of the ticket).
            let fee_cut = ((&full_percent - &fee_share_raw) / &percent_denom)
                .normalized()
                .to_string();
            CutsHistoryPoint {
                block_number: r.get::<i64, _>("block_number").to_string(),
                block_timestamp: r.get("block_timestamp"),
                fee_cut_percent: fee_cut,
                reward_cut_percent: reward_cut,
                fee_share_percent: fee_share,
                event_id: r.get::<i64, _>("id").to_string(),
            }
        })
        .collect();

    Ok(Json(CutsHistoryResponse {
        address,
        data,
        meta: ProfileListMeta {
            chain_id: state.chain_id.to_string(),
            next_cursor: None,
        },
    }))
}

// ─────────────────────────────────────────────────────────────────────────
// /orchestrators/{addr}/net-economics
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
#[schema(description = "Time window for net-economics calculation.")]
pub struct NetEconomicsQuery {
    /// Number of trailing days to aggregate. Defaults to 30.
    pub period_days: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Aggregate revenue minus on-chain gas costs over a window.")]
pub struct NetEconomicsResponse {
    pub address: String,
    pub period_days: u32,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    /// Sum of orch_payouts_daily.sum_orch_value_usd across the period.
    pub gross_payouts_usd: String,
    /// Sum of orch_rewards_daily.orch_lpt_rewards_usd across the period.
    pub gross_rewards_usd: String,
    /// Sum of tx fees (in native ETH) across all reward + redeem events
    /// the orch was party to in the window. Currently captured for events
    /// `to_address = orch` (Reward + WinningTicketRedeemed). Note that the
    /// economic impact of gas falls on whoever paid the tx (often the
    /// orch itself for Reward; the gateway for redeems).
    pub gas_cost_native_eth: String,
    /// Net = gross_payouts_usd + gross_rewards_usd. Gas cost is exposed
    /// separately in native ETH; the API leaves USD conversion to the
    /// caller (no token_prices_by_block lookup needed here).
    pub gross_total_usd: String,
}

#[utoipa::path(
    get,
    path = "/orchestrators/{address}/net-economics",
    tag = "Orchestrator history",
    params(
        ("address" = String, Path, description = "Orchestrator address."),
        NetEconomicsQuery
    ),
    responses(
        (status = 200, description = "Aggregate gross payouts/rewards plus gas spent over the window.", body = NetEconomicsResponse),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn orchestrators_net_economics(
    State(state): State<AppState>,
    Path(address): Path<String>,
    Query(q): Query<NetEconomicsQuery>,
) -> Result<Json<NetEconomicsResponse>, ApiError> {
    let address = normalize_addr(&address)?;
    let period_days = q.period_days.unwrap_or(30).clamp(1, 365);
    let period_end = Utc::now();
    let period_start = period_end - chrono::Duration::days(period_days as i64);

    let row = sqlx::query(
        r#"
        WITH gross AS (
          SELECT
            COALESCE((SELECT SUM(sum_commission_usd) FROM orch_payouts_daily
                       WHERE chain_id = $1 AND orchestrator_address = $2
                         AND day_utc >= $3::date AND day_utc < $4::date), 0) AS payouts_usd,
            COALESCE((SELECT SUM(sum_orch_tokens_usd) FROM orch_rewards_daily
                       WHERE chain_id = $1 AND orchestrator_address = $2
                         AND day_utc >= $3::date AND day_utc < $4::date), 0) AS rewards_usd
        ),
        gas AS (
          SELECT COALESCE(SUM(t.tx_fee_eth), 0) AS gas_eth
            FROM tx_receipts t
            JOIN raw_protocol_events e ON e.tx_hash = t.tx_hash AND e.chain_id = t.chain_id
           WHERE t.chain_id = $1
             AND e.to_address = $2
             AND e.event_name IN ('Reward', 'WinningTicketRedeemed')
             AND e.is_canonical = TRUE
             AND e.block_timestamp >= $3
             AND e.block_timestamp <  $4
        )
        SELECT gross.payouts_usd, gross.rewards_usd, gas.gas_eth
          FROM gross, gas
        "#,
    )
    .bind(state.chain_id)
    .bind(&address)
    .bind(period_start)
    .bind(period_end)
    .fetch_one(&state.pg)
    .await?;

    let payouts_usd: BigDecimal = row.get("payouts_usd");
    let rewards_usd: BigDecimal = row.get("rewards_usd");
    let gas_eth: BigDecimal = row.get("gas_eth");
    let total = (&payouts_usd + &rewards_usd).normalized().to_string();

    Ok(Json(NetEconomicsResponse {
        address,
        period_days,
        period_start,
        period_end,
        gross_payouts_usd: payouts_usd.normalized().to_string(),
        gross_rewards_usd: rewards_usd.normalized().to_string(),
        gas_cost_native_eth: gas_eth.normalized().to_string(),
        gross_total_usd: total,
    }))
}

#[cfg(test)]
mod tests {
    use crate::{build_router, metrics::Metrics, state::AppState};
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use livepeer_core::{db, rpc::Provider};
    use serde_json::Value;
    use sqlx::PgPool;
    use std::{
        sync::Arc,
        time::{SystemTime, UNIX_EPOCH},
    };
    use tower::util::ServiceExt;

    #[tokio::test]
    async fn orchestrators_route_uses_override_and_supports_cursor_and_active_filter() {
        let ctx = TestContext::new().await;
        let orch_a = "0x1111111111111111111111111111111111111111";
        let orch_b = "0x2222222222222222222222222222222222222222";
        let orch_c = "0x3333333333333333333333333333333333333333";

        // TD-026: orchestrator_profile is now a matview over orch_stake_by_round.
        // Seed the source table and refresh.
        sqlx::query(
            r#"INSERT INTO orch_stake_by_round (
                   chain_id, address, round, block_number, block_timestamp, block_hash,
                   total_stake, service_uri, latest_fee_cut_percent,
                   latest_reward_cut_percent, latest_fee_share_percent,
                   is_active, last_lifecycle_event_at, triggering_event_id
               ) VALUES
                   ($1, $2, 7, 123, '2024-01-01T00:00:00+00'::timestamptz, '0xfa', 300, 'https://orch-a.test', 80, 10, 20, TRUE, now(), NULL),
                   ($1, $3, 7, 124, '2024-01-01T00:00:01+00'::timestamptz, '0xfb', 200, 'https://orch-b.test', 70, 20, 30, FALSE, now(), NULL),
                   ($1, $4, 7, 125, '2024-01-01T00:00:02+00'::timestamptz, '0xfc', 100, 'https://orch-c.test', 60, 30, 40, TRUE, now(), NULL)"#,
        )
        .bind(ctx.chain_id)
        .bind(orch_a)
        .bind(orch_b)
        .bind(orch_c)
        .execute(&ctx.pg)
        .await
        .unwrap();
        sqlx::query("REFRESH MATERIALIZED VIEW orchestrator_profile")
            .execute(&ctx.pg)
            .await
            .unwrap();

        sqlx::query(
            r#"INSERT INTO orchestrator_ens (
                   chain_id, address, ens_name, ens_avatar_url, ens_last_resolved_at
               ) VALUES
                   ($1, $2, 'alpha.eth', 'https://ens.alpha/avatar.png', now()),
                   ($1, $3, 'beta.eth', 'https://ens.beta/avatar.png', now())"#,
        )
        .bind(ctx.chain_id)
        .bind(orch_a)
        .bind(orch_b)
        .execute(&ctx.pg)
        .await
        .unwrap();

        sqlx::query(
            r#"INSERT INTO name_avatar_overrides (
                   chain_id, address, display_name, avatar_url, notes, updated_at
               ) VALUES
                   ($1, $2, 'override-alpha', 'https://override.alpha/avatar.png', 'fixture', now())"#,
        )
        .bind(ctx.chain_id)
        .bind(orch_a)
        .execute(&ctx.pg)
        .await
        .unwrap();

        let response = ctx
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/orchestrators?limit=2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        let data = body["data"].as_array().unwrap();
        assert_eq!(data.len(), 2);
        assert_eq!(data[0]["address"], orch_a);
        assert_eq!(data[0]["display_name"], "override-alpha");
        assert_eq!(data[0]["avatar_url"], "https://override.alpha/avatar.png");
        assert_eq!(data[1]["address"], orch_b);
        assert_eq!(data[1]["display_name"], "beta.eth");
        let cursor = body["meta"]["next_cursor"].as_str().unwrap().to_string();

        let response = ctx
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/orchestrators?limit=2&cursor={cursor}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        let data = body["data"].as_array().unwrap();
        assert_eq!(data.len(), 1);
        assert_eq!(data[0]["address"], orch_c);

        let response = ctx
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/orchestrators?active_only=true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        let data = body["data"].as_array().unwrap();
        assert_eq!(data.len(), 2);
        assert!(data.iter().all(|row| row["is_active"] == Value::Bool(true)));

        let response = ctx
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/orchestrators/{orch_a}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["display_name"], "override-alpha");
        assert_eq!(body["service_uri"], "https://orch-a.test");
    }

    #[tokio::test]
    async fn gateways_route_uses_classification_default_and_override_precedence() {
        let ctx = TestContext::new().await;
        let gateway_a = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let gateway_b = "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

        // TD-025: broadcaster_profile is now a materialized view over
        // gateway_balances_by_block. Seed the source table and refresh.
        sqlx::query(
            r#"INSERT INTO gateway_balances_by_block (
                   chain_id, gateway_address, block_number, block_timestamp, block_hash,
                   deposit, reserve_funds_remaining, reserve_claimed_in_current_round,
                   withdraw_round, unlock_in_progress, source, raw_call,
                   triggering_event_id, created_at
               ) VALUES
                   ($1, $2, 500, '2024-01-01T00:00:00+00'::timestamptz, '0xfixture_a', 99, 12, 0, 0, FALSE, 'test', NULL, NULL, NOW()),
                   ($1, $3, 501, '2024-01-01T00:00:01+00'::timestamptz, '0xfixture_b', 50, 6, 0, 0, TRUE, 'test', NULL, NULL, NOW())"#,
        )
        .bind(ctx.chain_id)
        .bind(gateway_a)
        .bind(gateway_b)
        .execute(&ctx.pg)
        .await
        .unwrap();
        sqlx::query("REFRESH MATERIALIZED VIEW broadcaster_profile")
            .execute(&ctx.pg)
            .await
            .unwrap();

        sqlx::query(
            r#"INSERT INTO broadcaster_ens (
                   chain_id, address, ens_name, ens_avatar_url, ens_last_resolved_at
               ) VALUES
                   ($1, $2, 'gateway-a.eth', 'https://ens.gateway-a/avatar.png', now()),
                   ($1, $3, 'gateway-b.eth', 'https://ens.gateway-b/avatar.png', now())"#,
        )
        .bind(ctx.chain_id)
        .bind(gateway_a)
        .bind(gateway_b)
        .execute(&ctx.pg)
        .await
        .unwrap();

        sqlx::query(
            r#"INSERT INTO broadcaster_classifications (
                   chain_id, address, kind, source, notes, updated_at
               ) VALUES
                   ($1, $2, 'ai', 'test', 'fixture', now())"#,
        )
        .bind(ctx.chain_id)
        .bind(gateway_a)
        .execute(&ctx.pg)
        .await
        .unwrap();

        sqlx::query(
            r#"INSERT INTO name_avatar_overrides (
                   chain_id, address, display_name, avatar_url, notes, updated_at
               ) VALUES
                   ($1, $2, 'gateway-override', 'https://override.gateway/avatar.png', 'fixture', now())"#,
        )
        .bind(ctx.chain_id)
        .bind(gateway_a)
        .execute(&ctx.pg)
        .await
        .unwrap();

        let response = ctx
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/gateways?limit=2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        let data = body["data"].as_array().unwrap();
        assert_eq!(data.len(), 2);
        assert_eq!(data[0]["address"], gateway_a);
        assert_eq!(data[0]["kind"], "ai");
        assert_eq!(data[0]["display_name"], "gateway-override");
        assert_eq!(data[0]["avatar_url"], "https://override.gateway/avatar.png");
        assert_eq!(data[1]["address"], gateway_b);
        assert_eq!(data[1]["kind"], "transcoding");
        assert_eq!(data[1]["display_name"], "gateway-b.eth");

        let response = ctx
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/gateways/{gateway_a}/profile"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["kind"], "ai");
        assert_eq!(body["unlock_in_progress"], Value::Bool(false));
    }

    struct TestContext {
        app: axum::Router,
        pg: PgPool,
        chain_id: i64,
    }

    impl TestContext {
        async fn new() -> Self {
            let pg = db::connect(&test_database_url(), 5).await.unwrap();
            let chain_id = unique_chain_id();
            let archive = Provider::new("test", "http://127.0.0.1:9").unwrap();
            let state = AppState {
                pg: pg.clone(),
                default_version: "test".to_string(),
                chain_id,
                ticket_broker_address: "0x0000000000000000000000000000000000000000".to_string(),
                archive,
                metrics: Arc::new(Metrics::new()),
            };
            Self {
                app: build_router(state),
                pg,
                chain_id,
            }
        }
    }

    impl Drop for TestContext {
        fn drop(&mut self) {
            let url = test_database_url();
            let chain_id = self.chain_id;
            std::thread::spawn(move || {
                if let Ok(rt) = tokio::runtime::Runtime::new() {
                    rt.block_on(async move {
                        if let Ok(pg) = db::connect(&url, 1).await {
                            // TD-025/TD-026: broadcaster_profile and
                            // orchestrator_profile are matviews; clean their source
                            // tables instead.
                            let _ = sqlx::query(
                                r#"DELETE FROM name_avatar_overrides WHERE chain_id = $1;
                                   DELETE FROM broadcaster_classifications WHERE chain_id = $1;
                                   DELETE FROM orchestrator_ens WHERE chain_id = $1;
                                   DELETE FROM broadcaster_ens WHERE chain_id = $1;
                                   DELETE FROM orch_stake_by_round WHERE chain_id = $1;
                                   DELETE FROM gateway_balances_by_block WHERE chain_id = $1;"#,
                            )
                            .bind(chain_id)
                            .execute(&pg)
                            .await;
                        }
                    });
                }
            });
        }
    }

    async fn json_body(response: axum::response::Response) -> Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn unique_chain_id() -> i64 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i64;
        900_000 + (nanos % 100_000)
    }

    fn test_database_url() -> String {
        if let Ok(url) = std::env::var("DATABASE_URL") {
            return url.replace("@postgres:", "@127.0.0.1:");
        }
        let env_path = format!("{}/../../.env", env!("CARGO_MANIFEST_DIR"));
        let env_file = std::fs::read_to_string(&env_path)
            .unwrap_or_else(|_| panic!("{env_path} must exist for API route tests"));
        let mut user = None;
        let mut password = None;
        let mut db_name = None;
        let mut port = None;
        for line in env_file.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') || trimmed.is_empty() {
                continue;
            }
            let Some((key, value)) = trimmed.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches('"');
            match key.trim() {
                "POSTGRES_USER" => user = Some(value.to_string()),
                "POSTGRES_PASSWORD" => password = Some(value.to_string()),
                "POSTGRES_DB" => db_name = Some(value.to_string()),
                "POSTGRES_PORT" => port = Some(value.to_string()),
                _ => {}
            }
        }
        format!(
            "postgres://{}:{}@127.0.0.1:{}/{}",
            user.expect("POSTGRES_USER missing"),
            password.expect("POSTGRES_PASSWORD missing"),
            port.unwrap_or_else(|| "5432".to_string()),
            db_name.expect("POSTGRES_DB missing"),
        )
    }
}
