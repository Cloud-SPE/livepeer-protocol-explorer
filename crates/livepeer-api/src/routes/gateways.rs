use crate::{abi::TicketBroker, error::ApiError, state::AppState};
use alloy::primitives::{Address, U256};
use alloy::sol_types::SolCall;
use axum::{
    extract::{Path, Query, State},
    Json,
};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Duration, Utc};
use livepeer_core::rpc::{cross_check, BlockTag};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::Row;
use std::str::FromStr;
use utoipa::{IntoParams, ToSchema};

const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 1_000;
const ETH_DECIMALS: u32 = 18;

#[derive(Debug, Serialize, ToSchema)]
#[schema(
    description = "Exact TicketBroker sender balance state for a gateway at a specific block."
)]
pub struct GatewayBalanceRow {
    pub gateway_address: String,
    pub block_number: String,
    pub deposit: String,
    pub reserve_funds_remaining: String,
    pub reserve_claimed_in_current_round: String,
    pub withdraw_round: String,
    pub unlock_in_progress: bool,
    pub source: String,
}

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
#[schema(description = "Query parameters for gateway balance history snapshots.")]
pub struct GatewayBalanceHistoryQuery {
    /// Inclusive lower block bound.
    pub from_block: Option<i64>,
    /// Inclusive upper block bound.
    pub to_block: Option<i64>,
    /// Maximum number of balance snapshots to return.
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Historical TicketBroker sender balance snapshots for a gateway.")]
pub struct GatewayBalanceHistoryResponse {
    pub gateway_address: String,
    pub data: Vec<GatewayBalanceRow>,
}

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
#[schema(description = "Query parameters for gateway claimant reserve history.")]
pub struct GatewayClaimantsQuery {
    /// Inclusive lower block bound.
    pub from_block: Option<i64>,
    /// Inclusive upper block bound.
    pub to_block: Option<i64>,
    /// Maximum number of claimant rows to return.
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Point-in-time claimant reserve state for a gateway.")]
pub struct GatewayClaimantRow {
    pub gateway_address: String,
    pub claimant_address: String,
    pub block_number: String,
    pub claimable_reserve: String,
    pub claimed_reserve: String,
    pub source: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Historical claimant reserve rows for a gateway.")]
pub struct GatewayClaimantsResponse {
    pub gateway_address: String,
    pub data: Vec<GatewayClaimantRow>,
}

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
#[schema(description = "Query parameters for gateway flow history.")]
pub struct GatewayFlowsQuery {
    /// Optional lower block bound.
    pub from_block: Option<i64>,
    /// Optional upper block bound.
    pub to_block: Option<i64>,
    /// Maximum number of flow rows to return.
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(
    description = "One funding, payout, or withdrawal flow row for a gateway derived from TicketBroker events."
)]
pub struct GatewayFlowRow {
    pub event_id: String,
    pub event_name: String,
    pub flow_kind: String,
    pub block_number: String,
    pub block_timestamp: DateTime<Utc>,
    pub tx_hash: String,
    pub log_index: u32,
    pub asset: Option<String>,
    pub amount_native: Option<String>,
    pub amount_usd: Option<String>,
    pub from_address: Option<String>,
    pub to_address: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Historical funding and payout flow rows for a gateway.")]
pub struct GatewayFlowsResponse {
    pub gateway_address: String,
    pub data: Vec<GatewayFlowRow>,
}

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
#[schema(description = "Query parameters for gateway payout flow history.")]
pub struct GatewayPayoutsQuery {
    /// Optional lower block bound.
    pub from_block: Option<i64>,
    /// Optional upper block bound.
    pub to_block: Option<i64>,
    /// Maximum number of payout rows to return.
    pub limit: Option<u32>,
    /// If true, include transfer-side reserve movement rows alongside redemption and reserve-claim rows.
    pub include_transfers: Option<bool>,
}

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
#[schema(description = "Summary query for gateway funding and payout activity.")]
pub struct GatewaySummaryQuery {
    /// Rolling window length in days. Defaults to 7.
    pub days: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Aggregated funding or payout totals for a single TicketBroker event type.")]
pub struct GatewaySummaryRow {
    pub event_name: String,
    pub flow_kind: String,
    pub count: String,
    pub total_amount_native: String,
    pub total_amount_usd: String,
    pub usd_rows_priced: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Rolling gateway funding and payout summary over a recent time window.")]
pub struct GatewaySummaryResponse {
    pub gateway_address: String,
    pub days: String,
    pub from_timestamp: DateTime<Utc>,
    pub to_timestamp: DateTime<Utc>,
    pub data: Vec<GatewaySummaryRow>,
}

#[utoipa::path(
    get,
    path = "/gateways/{gateway}/balance/latest",
    tag = "Gateways",
    params(
        ("gateway" = String, Path, description = "Gateway/sender address to inspect.")
    ),
    responses(
        (status = 200, description = "Exact current TicketBroker sender state from on-chain RPC.", body = GatewayBalanceRow),
        (status = 400, description = "Invalid gateway address.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn balance_latest(
    State(state): State<AppState>,
    Path(gateway): Path<String>,
) -> Result<Json<GatewayBalanceRow>, ApiError> {
    let gateway = normalize_addr(&gateway)?;
    if let Some(row) = load_gateway_balance_latest_materialized(&state, &gateway).await? {
        return Ok(Json(row));
    }
    let block = state
        .archive
        .eth_block_number()
        .await
        .map_err(ApiError::internal)? as i64;
    let row = load_gateway_balance(&state, &gateway, block).await?;
    Ok(Json(row))
}

#[utoipa::path(
    get,
    path = "/gateways/{gateway}/balance/block/{block}",
    tag = "Gateways",
    params(
        ("gateway" = String, Path, description = "Gateway/sender address to inspect."),
        ("block" = i64, Path, description = "Resolve exact TicketBroker sender state at this block.")
    ),
    responses(
        (status = 200, description = "Exact TicketBroker sender state at the requested block.", body = GatewayBalanceRow),
        (status = 400, description = "Invalid gateway address or block number.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn balance_at_block(
    State(state): State<AppState>,
    Path((gateway, block)): Path<(String, i64)>,
) -> Result<Json<GatewayBalanceRow>, ApiError> {
    if block < 0 {
        return Err(ApiError::bad_request("block must be non-negative"));
    }
    let gateway = normalize_addr(&gateway)?;
    if let Some(row) = load_gateway_balance_materialized(&state, &gateway, block).await? {
        return Ok(Json(row));
    }
    Ok(Json(load_gateway_balance(&state, &gateway, block).await?))
}

#[utoipa::path(
    get,
    path = "/gateways/{gateway}/balance/history",
    tag = "Gateways",
    params(
        ("gateway" = String, Path, description = "Gateway/sender address to inspect."),
        GatewayBalanceHistoryQuery
    ),
    responses(
        (status = 200, description = "Materialized historical balance snapshots for the gateway.", body = GatewayBalanceHistoryResponse),
        (status = 400, description = "Invalid gateway address or range parameters.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn balance_history(
    State(state): State<AppState>,
    Path(gateway): Path<String>,
    Query(q): Query<GatewayBalanceHistoryQuery>,
) -> Result<Json<GatewayBalanceHistoryResponse>, ApiError> {
    let gateway = normalize_addr(&gateway)?;
    if let (Some(from), Some(to)) = (q.from_block, q.to_block) {
        if to < from {
            return Err(ApiError::bad_request("to_block < from_block"));
        }
    }
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as i64;
    let rows = sqlx::query(
        r#"SELECT gateway_address, block_number, deposit, reserve_funds_remaining,
                  reserve_claimed_in_current_round, withdraw_round, unlock_in_progress, source
             FROM gateway_balances_by_block
            WHERE chain_id = $1
              AND gateway_address = $2
              AND ($3::bigint IS NULL OR block_number >= $3)
              AND ($4::bigint IS NULL OR block_number <= $4)
            ORDER BY block_number ASC
            LIMIT $5"#,
    )
    .bind(state.chain_id)
    .bind(&gateway)
    .bind(q.from_block)
    .bind(q.to_block)
    .bind(limit)
    .fetch_all(&state.pg)
    .await?;
    Ok(Json(GatewayBalanceHistoryResponse {
        gateway_address: gateway,
        data: rows.iter().map(materialized_balance_row).collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/gateways/{gateway}/claimants/block/{block}",
    tag = "Gateways",
    params(
        ("gateway" = String, Path, description = "Gateway/sender address to inspect."),
        ("block" = i64, Path, description = "Return each claimant's latest reserve snapshot at or before this block.")
    ),
    responses(
        (status = 200, description = "Claimant reserve state at the requested block.", body = GatewayClaimantsResponse),
        (status = 400, description = "Invalid gateway address or block.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn claimants_at_block(
    State(state): State<AppState>,
    Path((gateway, block)): Path<(String, i64)>,
) -> Result<Json<GatewayClaimantsResponse>, ApiError> {
    if block < 0 {
        return Err(ApiError::bad_request("block must be non-negative"));
    }
    let gateway = normalize_addr(&gateway)?;
    let rows = sqlx::query(
        r#"WITH latest AS (
               SELECT DISTINCT ON (claimant_address)
                      gateway_address, claimant_address, block_number,
                      claimable_reserve, claimed_reserve, source
                 FROM gateway_claimants_by_block
                WHERE chain_id = $1
                  AND gateway_address = $2
                  AND block_number <= $3
                ORDER BY claimant_address, block_number DESC
           )
           SELECT gateway_address, claimant_address, block_number,
                  claimable_reserve, claimed_reserve, source
             FROM latest
            ORDER BY claimable_reserve DESC, claimant_address ASC"#,
    )
    .bind(state.chain_id)
    .bind(&gateway)
    .bind(block)
    .fetch_all(&state.pg)
    .await?;
    Ok(Json(GatewayClaimantsResponse {
        gateway_address: gateway,
        data: rows.iter().map(materialized_claimant_row).collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/gateways/{gateway}/claimants/history",
    tag = "Gateways",
    params(
        ("gateway" = String, Path, description = "Gateway/sender address to inspect."),
        GatewayClaimantsQuery
    ),
    responses(
        (status = 200, description = "Historical claimant reserve snapshots for the gateway.", body = GatewayClaimantsResponse),
        (status = 400, description = "Invalid gateway address or range.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn claimants_history(
    State(state): State<AppState>,
    Path(gateway): Path<String>,
    Query(q): Query<GatewayClaimantsQuery>,
) -> Result<Json<GatewayClaimantsResponse>, ApiError> {
    let gateway = normalize_addr(&gateway)?;
    if let (Some(from), Some(to)) = (q.from_block, q.to_block) {
        if to < from {
            return Err(ApiError::bad_request("to_block < from_block"));
        }
    }
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as i64;
    let rows = sqlx::query(
        r#"SELECT gateway_address, claimant_address, block_number,
                  claimable_reserve, claimed_reserve, source
             FROM gateway_claimants_by_block
            WHERE chain_id = $1
              AND gateway_address = $2
              AND ($3::bigint IS NULL OR block_number >= $3)
              AND ($4::bigint IS NULL OR block_number <= $4)
            ORDER BY block_number ASC, claimant_address ASC
            LIMIT $5"#,
    )
    .bind(state.chain_id)
    .bind(&gateway)
    .bind(q.from_block)
    .bind(q.to_block)
    .bind(limit)
    .fetch_all(&state.pg)
    .await?;
    Ok(Json(GatewayClaimantsResponse {
        gateway_address: gateway,
        data: rows.iter().map(materialized_claimant_row).collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/gateways/{gateway}/flows",
    tag = "Gateways",
    params(
        ("gateway" = String, Path, description = "Gateway/sender address to inspect."),
        GatewayFlowsQuery
    ),
    responses(
        (status = 200, description = "Funding, payout, reserve-claim, and withdrawal flows for the gateway.", body = GatewayFlowsResponse),
        (status = 400, description = "Invalid gateway address.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn flows(
    State(state): State<AppState>,
    Path(gateway): Path<String>,
    Query(q): Query<GatewayFlowsQuery>,
) -> Result<Json<GatewayFlowsResponse>, ApiError> {
    let gateway = normalize_addr(&gateway)?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as i64;
    let rows = sqlx::query(
        r#"SELECT r.id, r.event_name, r.block_number, r.block_timestamp, r.tx_hash, r.log_index,
                  r.asset, r.amount_normalized, r.from_address, r.to_address,
                  v.amount_usd
             FROM raw_protocol_events r
             LEFT JOIN event_valuations v
               ON v.event_id = r.id
              AND v.status = 'priced'
            WHERE r.chain_id = $1
              AND r.is_canonical = TRUE
              AND r.contract_name = 'TicketBroker'
              AND r.from_address = $2
              AND r.event_name IN (
                    'DepositFunded',
                    'ReserveFunded',
                    'WinningTicketTransfer',
                    'WinningTicketRedeemed',
                    'ReserveClaimed',
                    'Withdrawal'
              )
              AND ($3::bigint IS NULL OR r.block_number >= $3)
              AND ($4::bigint IS NULL OR r.block_number <= $4)
            ORDER BY r.block_number DESC, r.log_index DESC
            LIMIT $5"#,
    )
    .bind(state.chain_id)
    .bind(&gateway)
    .bind(q.from_block)
    .bind(q.to_block)
    .bind(limit)
    .fetch_all(&state.pg)
    .await?;

    let data = rows
        .iter()
        .map(|r| GatewayFlowRow {
            event_id: r.get::<i64, _>("id").to_string(),
            event_name: r.get("event_name"),
            flow_kind: flow_kind_for_event_name(r.get::<String, _>("event_name").as_str())
                .to_string(),
            block_number: r.get::<i64, _>("block_number").to_string(),
            block_timestamp: r.get("block_timestamp"),
            tx_hash: r.get("tx_hash"),
            log_index: r.get::<i32, _>("log_index") as u32,
            asset: r.try_get("asset").ok(),
            amount_native: r
                .try_get::<BigDecimal, _>("amount_normalized")
                .ok()
                .map(|v| v.to_string()),
            amount_usd: r
                .try_get::<BigDecimal, _>("amount_usd")
                .ok()
                .map(|v| v.to_string()),
            from_address: r.try_get("from_address").ok(),
            to_address: r.try_get("to_address").ok(),
        })
        .collect();

    Ok(Json(GatewayFlowsResponse {
        gateway_address: gateway,
        data,
    }))
}

#[utoipa::path(
    get,
    path = "/gateways/{gateway}/payouts",
    tag = "Gateways",
    params(
        ("gateway" = String, Path, description = "Gateway/sender address to inspect."),
        GatewayPayoutsQuery
    ),
    responses(
        (status = 200, description = "Materialized payout-side gateway flow rows.", body = GatewayFlowsResponse),
        (status = 400, description = "Invalid gateway address.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn payouts(
    State(state): State<AppState>,
    Path(gateway): Path<String>,
    Query(q): Query<GatewayPayoutsQuery>,
) -> Result<Json<GatewayFlowsResponse>, ApiError> {
    let gateway = normalize_addr(&gateway)?;
    let include_transfers = q.include_transfers.unwrap_or(false);
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as i64;
    let rows = sqlx::query(
        r#"SELECT event_id, gateway_address, claimant_address, counterparty_address,
                  block_number, block_timestamp, tx_hash, log_index, event_name, flow_kind,
                  asset, amount_native, amount_usd
             FROM gateway_flows
            WHERE chain_id = $1
              AND gateway_address = $2
              AND ($3::bigint IS NULL OR block_number >= $3)
              AND ($4::bigint IS NULL OR block_number <= $4)
              AND (
                    flow_kind IN ('ticket_redeemed', 'reserve_claimed', 'withdrawal')
                    OR ($5::boolean = TRUE AND flow_kind = 'reserve_transfer')
              )
            ORDER BY block_number DESC, log_index DESC
            LIMIT $6"#,
    )
    .bind(state.chain_id)
    .bind(&gateway)
    .bind(q.from_block)
    .bind(q.to_block)
    .bind(include_transfers)
    .bind(limit)
    .fetch_all(&state.pg)
    .await?;
    Ok(Json(GatewayFlowsResponse {
        gateway_address: gateway,
        data: rows.iter().map(materialized_flow_row).collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/gateways/{gateway}/summary",
    tag = "Gateways",
    params(
        ("gateway" = String, Path, description = "Gateway/sender address to inspect."),
        GatewaySummaryQuery
    ),
    responses(
        (status = 200, description = "Rolling funding and payout summary for the gateway.", body = GatewaySummaryResponse),
        (status = 400, description = "Invalid gateway address or days value.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn summary(
    State(state): State<AppState>,
    Path(gateway): Path<String>,
    Query(q): Query<GatewaySummaryQuery>,
) -> Result<Json<GatewaySummaryResponse>, ApiError> {
    let gateway = normalize_addr(&gateway)?;
    let days = q.days.unwrap_or(7);
    if !(1..=3650).contains(&days) {
        return Err(ApiError::bad_request("days must be between 1 and 3650"));
    }
    let to_ts = Utc::now();
    let from_ts = to_ts - Duration::days(days);

    let rows = sqlx::query(
        r#"SELECT r.event_name,
                  COUNT(*)::bigint AS count,
                  COALESCE(SUM(r.amount_normalized), 0) AS total_amount_native,
                  COALESCE(SUM(v.amount_usd), 0) AS total_amount_usd,
                  COUNT(v.amount_usd)::bigint AS usd_rows_priced
             FROM raw_protocol_events r
             LEFT JOIN event_valuations v
               ON v.event_id = r.id
              AND v.status = 'priced'
            WHERE r.chain_id = $1
              AND r.is_canonical = TRUE
              AND r.contract_name = 'TicketBroker'
              AND r.from_address = $2
              AND r.event_name IN (
                    'DepositFunded',
                    'ReserveFunded',
                    'WinningTicketTransfer',
                    'WinningTicketRedeemed',
                    'ReserveClaimed',
                    'Withdrawal'
              )
              AND r.block_timestamp >= $3
              AND r.block_timestamp <= $4
            GROUP BY r.event_name
            ORDER BY MIN(r.block_timestamp) DESC, r.event_name ASC"#,
    )
    .bind(state.chain_id)
    .bind(&gateway)
    .bind(from_ts)
    .bind(to_ts)
    .fetch_all(&state.pg)
    .await?;

    let data = rows
        .iter()
        .map(|r| {
            let event_name: String = r.get("event_name");
            GatewaySummaryRow {
                flow_kind: flow_kind_for_event_name(&event_name).to_string(),
                event_name,
                count: r.get::<i64, _>("count").to_string(),
                total_amount_native: r.get::<BigDecimal, _>("total_amount_native").to_string(),
                total_amount_usd: r.get::<BigDecimal, _>("total_amount_usd").to_string(),
                usd_rows_priced: r.get::<i64, _>("usd_rows_priced").to_string(),
            }
        })
        .collect();

    Ok(Json(GatewaySummaryResponse {
        gateway_address: gateway,
        days: days.to_string(),
        from_timestamp: from_ts,
        to_timestamp: to_ts,
        data,
    }))
}

async fn load_gateway_balance(
    state: &AppState,
    gateway: &str,
    block: i64,
) -> Result<GatewayBalanceRow, ApiError> {
    let gateway_addr =
        Address::from_str(gateway).map_err(|_| ApiError::bad_request("invalid gateway address"))?;

    let sender_data = format!(
        "0x{}",
        alloy::hex::encode(
            TicketBroker::getSenderInfoCall {
                _sender: gateway_addr
            }
            .abi_encode()
        )
    );
    let sender_params = json!([
        { "to": state.ticket_broker_address, "data": sender_data },
        BlockTag::Number(block as u64).to_param()
    ]);
    let sender_outcome = cross_check::single_call_cached(
        &state.pg,
        &state.archive,
        "eth_call",
        &sender_params,
        Some(block),
    )
    .await
    .map_err(ApiError::internal)?;
    let sender_raw = decode_hex_result(&sender_outcome.response_bytes)?;
    let sender = TicketBroker::getSenderInfoCall::abi_decode_returns(&sender_raw, true)
        .map_err(ApiError::internal)?;

    let unlock_data = format!(
        "0x{}",
        alloy::hex::encode(
            TicketBroker::isUnlockInProgressCall {
                _sender: gateway_addr
            }
            .abi_encode()
        )
    );
    let unlock_params = json!([
        { "to": state.ticket_broker_address, "data": unlock_data },
        BlockTag::Number(block as u64).to_param()
    ]);
    let unlock_outcome = cross_check::single_call_cached(
        &state.pg,
        &state.archive,
        "eth_call",
        &unlock_params,
        Some(block),
    )
    .await
    .map_err(ApiError::internal)?;
    let unlock_raw = decode_hex_result(&unlock_outcome.response_bytes)?;
    let unlock = TicketBroker::isUnlockInProgressCall::abi_decode_returns(&unlock_raw, true)
        .map_err(ApiError::internal)?;

    Ok(GatewayBalanceRow {
        gateway_address: gateway.to_string(),
        block_number: block.to_string(),
        deposit: u256_to_decimal(&sender.sender.deposit, ETH_DECIMALS).to_string(),
        reserve_funds_remaining: u256_to_decimal(&sender.reserve.fundsRemaining, ETH_DECIMALS)
            .to_string(),
        reserve_claimed_in_current_round: u256_to_decimal(
            &sender.reserve.claimedInCurrentRound,
            ETH_DECIMALS,
        )
        .to_string(),
        withdraw_round: sender.sender.withdrawRound.to_string(),
        unlock_in_progress: unlock._0,
        source: "ticketbroker_getSenderInfo_rpc".to_string(),
    })
}

async fn load_gateway_balance_latest_materialized(
    state: &AppState,
    gateway: &str,
) -> Result<Option<GatewayBalanceRow>, ApiError> {
    let row = sqlx::query(
        r#"SELECT gateway_address, block_number, deposit, reserve_funds_remaining,
                  reserve_claimed_in_current_round, withdraw_round, unlock_in_progress, source
             FROM gateway_balances_by_block
            WHERE chain_id = $1
              AND gateway_address = $2
            ORDER BY block_number DESC
            LIMIT 1"#,
    )
    .bind(state.chain_id)
    .bind(gateway)
    .fetch_optional(&state.pg)
    .await?;
    Ok(row.as_ref().map(materialized_balance_row))
}

async fn load_gateway_balance_materialized(
    state: &AppState,
    gateway: &str,
    block: i64,
) -> Result<Option<GatewayBalanceRow>, ApiError> {
    let row = sqlx::query(
        r#"SELECT gateway_address, block_number, deposit, reserve_funds_remaining,
                  reserve_claimed_in_current_round, withdraw_round, unlock_in_progress, source
             FROM gateway_balances_by_block
            WHERE chain_id = $1
              AND gateway_address = $2
              AND block_number = $3
            LIMIT 1"#,
    )
    .bind(state.chain_id)
    .bind(gateway)
    .bind(block)
    .fetch_optional(&state.pg)
    .await?;
    Ok(row.as_ref().map(materialized_balance_row))
}

fn materialized_balance_row(r: &sqlx::postgres::PgRow) -> GatewayBalanceRow {
    GatewayBalanceRow {
        gateway_address: r.get("gateway_address"),
        block_number: r.get::<i64, _>("block_number").to_string(),
        deposit: r.get::<BigDecimal, _>("deposit").to_string(),
        reserve_funds_remaining: r
            .get::<BigDecimal, _>("reserve_funds_remaining")
            .to_string(),
        reserve_claimed_in_current_round: r
            .get::<BigDecimal, _>("reserve_claimed_in_current_round")
            .to_string(),
        withdraw_round: r.get::<i64, _>("withdraw_round").to_string(),
        unlock_in_progress: r.get("unlock_in_progress"),
        source: r.get("source"),
    }
}

fn materialized_claimant_row(r: &sqlx::postgres::PgRow) -> GatewayClaimantRow {
    GatewayClaimantRow {
        gateway_address: r.get("gateway_address"),
        claimant_address: r.get("claimant_address"),
        block_number: r.get::<i64, _>("block_number").to_string(),
        claimable_reserve: r.get::<BigDecimal, _>("claimable_reserve").to_string(),
        claimed_reserve: r.get::<BigDecimal, _>("claimed_reserve").to_string(),
        source: r.get("source"),
    }
}

fn materialized_flow_row(r: &sqlx::postgres::PgRow) -> GatewayFlowRow {
    GatewayFlowRow {
        event_id: r.get::<i64, _>("event_id").to_string(),
        event_name: r.get("event_name"),
        flow_kind: r.get("flow_kind"),
        block_number: r.get::<i64, _>("block_number").to_string(),
        block_timestamp: r.get("block_timestamp"),
        tx_hash: r.get("tx_hash"),
        log_index: r.get::<i32, _>("log_index") as u32,
        asset: r.try_get("asset").ok(),
        amount_native: r
            .try_get::<BigDecimal, _>("amount_native")
            .ok()
            .map(|v| v.to_string()),
        amount_usd: r
            .try_get::<BigDecimal, _>("amount_usd")
            .ok()
            .map(|v| v.to_string()),
        from_address: Some(r.get("gateway_address")),
        to_address: r
            .try_get::<String, _>("claimant_address")
            .ok()
            .or_else(|| r.try_get::<String, _>("counterparty_address").ok()),
    }
}

fn normalize_addr(addr: &str) -> Result<String, ApiError> {
    let parsed = Address::from_str(addr).map_err(|_| ApiError::bad_request("invalid address"))?;
    Ok(format!("{parsed:#x}").to_lowercase())
}

fn decode_hex_result(bytes: &[u8]) -> Result<Vec<u8>, ApiError> {
    let s = std::str::from_utf8(bytes).unwrap_or_default();
    let hex_str = s.trim_matches('"').trim_start_matches("0x");
    alloy::hex::decode(hex_str).map_err(ApiError::internal)
}

fn u256_to_decimal(u: &U256, decimals: u32) -> BigDecimal {
    let raw = BigDecimal::from_str(&u.to_string()).unwrap_or_default();
    raw / BigDecimal::from(10u128.pow(decimals))
}

fn flow_kind_for_event_name(event_name: &str) -> &'static str {
    match event_name {
        "DepositFunded" => "deposit_in",
        "ReserveFunded" => "reserve_in",
        "WinningTicketTransfer" => "reserve_transfer",
        "WinningTicketRedeemed" => "ticket_redeemed",
        "ReserveClaimed" => "reserve_claimed",
        "Withdrawal" => "withdrawal",
        _ => "other",
    }
}
