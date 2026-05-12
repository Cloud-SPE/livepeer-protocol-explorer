use crate::{cursor::Cursor, error::ApiError, state::AppState};
use alloy::primitives::U256;
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use futures::stream::{self, StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::Row;
use std::{collections::HashMap, str::FromStr};
use utoipa::{IntoParams, ToSchema};

/// Concurrency for the RPC fallback path that fetches receipts for tx_hashes
/// not yet present in `tx_receipts`. Matches the empirically-safe ceiling
/// the staker uses for `eth_call` fanout (Chainstack burst tolerance).
const TX_FEE_RPC_FALLBACK_CONCURRENCY: usize = 12;

const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 1_000;
const PERCENT_DENOMINATOR: i64 = 1_000_000;
const ARBISCAN_TX_BASE_URL: &str = "https://arbiscan.io/tx/";

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
#[schema(description = "Query parameters for payout and reward CSV report downloads.")]
pub struct ReportQuery {
    pub orchestrator: Option<String>,
    pub gateway: Option<String>,
    pub start: String,
    pub end: String,
    pub valuation_version: Option<String>,
    pub chain_id: Option<i64>,
}

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
#[schema(description = "Query parameters for orchestrator and gateway ticket history reads.")]
pub struct TicketHistoryQuery {
    pub start: Option<String>,
    pub end: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
    pub valuation_version: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "One winning-ticket payout row for ticket-history endpoints.")]
pub struct TicketHistoryRow {
    pub event_id: String,
    pub tx_hash: String,
    pub block_number: String,
    pub block_timestamp: DateTime<Utc>,
    pub gateway_address: String,
    pub orchestrator_address: String,
    pub face_value: String,
    pub face_value_usd: String,
    pub fee_share_percent: String,
    pub fee_cut_percent: String,
    pub valuation_version: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Paginated ticket history response.")]
pub struct TicketHistoryResponse {
    pub data: Vec<TicketHistoryRow>,
    pub next_cursor: Option<String>,
}

#[utoipa::path(
    get,
    path = "/reports/payouts.csv",
    tag = "Reports",
    params(ReportQuery),
    responses(
        (status = 200, description = "CSV report of WinningTicketRedeemed rows filtered by orchestrator.", body = String, content_type = "text/csv"),
        (status = 400, description = "Invalid query parameters.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn payouts_csv(
    State(state): State<AppState>,
    Query(q): Query<ReportQuery>,
) -> Result<Response, ApiError> {
    let orchestrator = normalize_required_addr(q.orchestrator.as_deref(), "orchestrator")?;
    let start = parse_date(&q.start, "start")?;
    let end = parse_date(&q.end, "end")?;
    validate_range(start, end)?;
    validate_chain_id(state.chain_id, q.chain_id)?;
    let valuation_version = q
        .valuation_version
        .clone()
        .unwrap_or_else(|| state.default_version.clone());

    let rows = fetch_payout_like_rows(
        &state,
        start,
        end,
        &valuation_version,
        Some(&orchestrator),
        None,
    )
    .await?;
    let tx_fees = load_tx_fees(
        &state,
        rows.iter()
            .map(|row| row.tx_hash.clone())
            .collect::<Vec<_>>(),
    )
    .await?;
    let mut csv = String::from(
        "timestamp,transaction_id,face_value,face_value_usd,orch_commission,orch_commission_usd,eth_price,fee_cut,transaction_fee,transaction_fee_usd,total_value_usd,total_value,block_number,chain_id,valuation_version,from_address,fee_share_percent,fee_cut_percent\n",
    );
    for row in rows {
        let tx_fee_native = tx_fees
            .get(&row.tx_hash)
            .cloned()
            .ok_or_else(|| ApiError::internal(format!("missing tx fee for {}", row.tx_hash)))?;
        let tx_fee_usd = tx_fee_native.clone() * row.native_usd_price.clone();
        let total_value_native = row.amount_native.clone() - tx_fee_native.clone();
        let total_value_usd = row.amount_usd.clone() - tx_fee_usd.clone();
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            row.block_timestamp.to_rfc3339(),
            csv_escape(&arbiscan_tx_url(&row.tx_hash)),
            row.amount_native.normalized(),
            row.amount_usd.normalized(),
            row.orch_amount_native.normalized(),
            row.orch_amount_usd.normalized(),
            row.native_usd_price.normalized(),
            row.keep_fraction.normalized(),
            tx_fee_native.normalized(),
            tx_fee_usd.normalized(),
            total_value_usd.normalized(),
            total_value_native.normalized(),
            row.block_number,
            row.chain_id,
            csv_escape(&row.valuation_version),
            csv_escape(&row.from_address),
            row.fee_share_percent.normalized(),
            row.fee_cut_percent.normalized(),
        ));
    }

    csv_response(
        csv,
        &valuation_version,
        backfill_complete_for_contract(&state, "TicketBroker", end).await?,
    )
}

#[utoipa::path(
    get,
    path = "/reports/gateway-payouts.csv",
    tag = "Reports",
    params(ReportQuery),
    responses(
        (status = 200, description = "CSV report of WinningTicketRedeemed rows filtered by gateway.", body = String, content_type = "text/csv"),
        (status = 400, description = "Invalid query parameters.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn gateway_payouts_csv(
    State(state): State<AppState>,
    Query(q): Query<ReportQuery>,
) -> Result<Response, ApiError> {
    let gateway = normalize_required_addr(q.gateway.as_deref(), "gateway")?;
    let start = parse_date(&q.start, "start")?;
    let end = parse_date(&q.end, "end")?;
    validate_range(start, end)?;
    validate_chain_id(state.chain_id, q.chain_id)?;
    let valuation_version = q
        .valuation_version
        .clone()
        .unwrap_or_else(|| state.default_version.clone());

    let rows = fetch_payout_like_rows(&state, start, end, &valuation_version, None, Some(&gateway))
        .await?;
    let tx_fees = load_tx_fees(
        &state,
        rows.iter()
            .map(|row| row.tx_hash.clone())
            .collect::<Vec<_>>(),
    )
    .await?;
    let mut csv = String::from(
        "timestamp,transaction_id,face_value,face_value_usd,orch_commission,orch_commission_usd,eth_price,fee_cut,transaction_fee,transaction_fee_usd,total_value_usd,total_value,block_number,chain_id,valuation_version,from_address,fee_share_percent,fee_cut_percent\n",
    );
    for row in rows {
        let tx_fee_native = tx_fees
            .get(&row.tx_hash)
            .cloned()
            .ok_or_else(|| ApiError::internal(format!("missing tx fee for {}", row.tx_hash)))?;
        let tx_fee_usd = tx_fee_native.clone() * row.native_usd_price.clone();
        let total_value_native = row.amount_native.clone() - tx_fee_native.clone();
        let total_value_usd = row.amount_usd.clone() - tx_fee_usd.clone();
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            row.block_timestamp.to_rfc3339(),
            csv_escape(&arbiscan_tx_url(&row.tx_hash)),
            row.amount_native.normalized(),
            row.amount_usd.normalized(),
            row.orch_amount_native.normalized(),
            row.orch_amount_usd.normalized(),
            row.native_usd_price.normalized(),
            row.keep_fraction.normalized(),
            tx_fee_native.normalized(),
            tx_fee_usd.normalized(),
            total_value_usd.normalized(),
            total_value_native.normalized(),
            row.block_number,
            row.chain_id,
            csv_escape(&row.valuation_version),
            csv_escape(&row.from_address),
            row.fee_share_percent.normalized(),
            row.fee_cut_percent.normalized(),
        ));
    }

    csv_response(
        csv,
        &valuation_version,
        backfill_complete_for_contract(&state, "TicketBroker", end).await?,
    )
}

#[utoipa::path(
    get,
    path = "/reports/rewards.csv",
    tag = "Reports",
    params(ReportQuery),
    responses(
        (status = 200, description = "CSV report of Reward rows filtered by orchestrator.", body = String, content_type = "text/csv"),
        (status = 400, description = "Invalid query parameters.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn rewards_csv(
    State(state): State<AppState>,
    Query(q): Query<ReportQuery>,
) -> Result<Response, ApiError> {
    let orchestrator = normalize_required_addr(q.orchestrator.as_deref(), "orchestrator")?;
    let start = parse_date(&q.start, "start")?;
    let end = parse_date(&q.end, "end")?;
    validate_range(start, end)?;
    validate_chain_id(state.chain_id, q.chain_id)?;
    let valuation_version = q
        .valuation_version
        .clone()
        .unwrap_or_else(|| state.default_version.clone());

    let rows = fetch_reward_rows(&state, start, end, &valuation_version, &orchestrator).await?;
    let tx_fees = load_tx_fees(
        &state,
        rows.iter()
            .map(|row| row.tx_hash.clone())
            .collect::<Vec<_>>(),
    )
    .await?;
    let mut csv = String::from(
        "timestamp,transaction_id,lpt_price,eth_price,orch_tokens,total_tokens,reward_cut,transaction_fee,transaction_fee_usd,total_value_usd,block_number,chain_id,valuation_version,reward_cut_percent\n",
    );
    for row in rows {
        let tx_fee_native = tx_fees
            .get(&row.tx_hash)
            .cloned()
            .ok_or_else(|| ApiError::internal(format!("missing tx fee for {}", row.tx_hash)))?;
        let tx_fee_usd = tx_fee_native.clone() * row.eth_price.clone();
        let orch_value_usd = row.orch_tokens.clone() * row.native_usd_price.clone();
        let total_value_usd = orch_value_usd - tx_fee_usd.clone();
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            row.block_timestamp.to_rfc3339(),
            csv_escape(&arbiscan_tx_url(&row.tx_hash)),
            row.native_usd_price.normalized(),
            row.eth_price.normalized(),
            row.orch_tokens.normalized(),
            row.amount_native.normalized(),
            row.keep_fraction.normalized(),
            tx_fee_native.normalized(),
            tx_fee_usd.normalized(),
            total_value_usd.normalized(),
            row.block_number,
            row.chain_id,
            csv_escape(&row.valuation_version),
            row.reward_cut_percent.normalized(),
        ));
    }

    csv_response(
        csv,
        &valuation_version,
        backfill_complete_for_contract(&state, "BondingManager", end).await?,
    )
}

#[utoipa::path(
    get,
    path = "/orchestrators/{address}/tickets/latest",
    tag = "Tickets",
    params(("address" = String, Path, description = "Orchestrator address."), TicketHistoryQuery),
    responses((status = 200, description = "Latest ticket redemptions for an orchestrator.", body = TicketHistoryResponse))
)]
pub async fn orchestrator_tickets_latest(
    State(state): State<AppState>,
    Path(address): Path<String>,
    Query(q): Query<TicketHistoryQuery>,
) -> Result<Json<TicketHistoryResponse>, ApiError> {
    let orchestrator = normalize_addr(&address)?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as i64;
    let cursor = q.cursor.as_deref().map(Cursor::decode).transpose()?;
    let valuation_version = q
        .valuation_version
        .clone()
        .unwrap_or_else(|| state.default_version.clone());

    let (data, next_cursor) = fetch_ticket_history(
        &state,
        TicketHistoryFetch {
            orchestrator: Some(&orchestrator),
            gateway: None,
            start: None,
            end: None,
            valuation_version: &valuation_version,
            cursor,
            limit,
        },
    )
    .await?;
    Ok(Json(TicketHistoryResponse { data, next_cursor }))
}

#[utoipa::path(
    get,
    path = "/gateways/{address}/tickets",
    tag = "Tickets",
    params(("address" = String, Path, description = "Gateway address."), TicketHistoryQuery),
    responses((status = 200, description = "Ticket redemptions sourced from a gateway.", body = TicketHistoryResponse))
)]
pub async fn gateway_tickets(
    State(state): State<AppState>,
    Path(address): Path<String>,
    Query(q): Query<TicketHistoryQuery>,
) -> Result<Json<TicketHistoryResponse>, ApiError> {
    let gateway = normalize_addr(&address)?;
    let start = q
        .start
        .as_deref()
        .map(|s| parse_date(s, "start"))
        .transpose()?;
    let end = q.end.as_deref().map(|s| parse_date(s, "end")).transpose()?;
    if let (Some(start), Some(end)) = (start, end) {
        validate_range(start, end)?;
    }
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as i64;
    let cursor = q.cursor.as_deref().map(Cursor::decode).transpose()?;
    let valuation_version = q
        .valuation_version
        .clone()
        .unwrap_or_else(|| state.default_version.clone());

    let (data, next_cursor) = fetch_ticket_history(
        &state,
        TicketHistoryFetch {
            orchestrator: None,
            gateway: Some(&gateway),
            start,
            end,
            valuation_version: &valuation_version,
            cursor,
            limit,
        },
    )
    .await?;
    Ok(Json(TicketHistoryResponse { data, next_cursor }))
}

struct PayoutLikeRow {
    block_timestamp: DateTime<Utc>,
    tx_hash: String,
    block_number: i64,
    chain_id: i64,
    valuation_version: String,
    from_address: String,
    amount_native: BigDecimal,
    amount_usd: BigDecimal,
    native_usd_price: BigDecimal,
    orch_amount_native: BigDecimal,
    orch_amount_usd: BigDecimal,
    fee_share_percent: BigDecimal,
    fee_cut_percent: BigDecimal,
    keep_fraction: BigDecimal,
}

struct RewardCsvRow {
    block_timestamp: DateTime<Utc>,
    tx_hash: String,
    block_number: i64,
    chain_id: i64,
    valuation_version: String,
    amount_native: BigDecimal,
    native_usd_price: BigDecimal,
    eth_price: BigDecimal,
    orch_tokens: BigDecimal,
    reward_cut_percent: BigDecimal,
    keep_fraction: BigDecimal,
}

async fn fetch_payout_like_rows(
    state: &AppState,
    start: NaiveDate,
    end: NaiveDate,
    valuation_version: &str,
    orchestrator: Option<&str>,
    gateway: Option<&str>,
) -> Result<Vec<PayoutLikeRow>, ApiError> {
    let rows = sqlx::query(
        r#"SELECT e.block_timestamp,
                  e.tx_hash,
                  e.block_number,
                  e.chain_id,
                  e.from_address,
                  v.valuation_version,
                  v.amount_native,
                  v.amount_usd,
                  v.native_usd_price,
                  COALESCE(tu.raw_event -> 'decoded' ->> 'feeShare', '0') AS fee_share_raw
             FROM raw_protocol_events e
             JOIN event_valuations v
               ON v.event_id = e.id
              AND v.asset = 'ETH'
              AND v.valuation_version = $4
        LEFT JOIN LATERAL (
               SELECT raw_event
                 FROM raw_protocol_events tu
                WHERE tu.chain_id = e.chain_id
                  AND tu.is_canonical = TRUE
                  AND tu.contract_name = 'BondingManager'
                  AND tu.event_name = 'TranscoderUpdate'
                  AND tu.to_address = e.to_address
                  AND (
                        tu.block_number < e.block_number
                        OR (tu.block_number = e.block_number AND tu.log_index <= e.log_index)
                  )
                ORDER BY tu.block_number DESC, tu.log_index DESC
                LIMIT 1
           ) tu ON TRUE
            WHERE e.chain_id = $1
              AND e.event_name = 'WinningTicketRedeemed'
              AND e.is_canonical = TRUE
              AND e.finality = 'finalized'
              AND e.block_timestamp >= $2::date
              AND e.block_timestamp < ($3::date + INTERVAL '1 day')
              AND ($5::text IS NULL OR e.to_address = $5)
              AND ($6::text IS NULL OR e.from_address = $6)
         ORDER BY e.block_number ASC, e.log_index ASC"#,
    )
    .bind(state.chain_id)
    .bind(start)
    .bind(end)
    .bind(valuation_version)
    .bind(orchestrator)
    .bind(gateway)
    .fetch_all(&state.pg)
    .await?;

    rows.into_iter()
        .map(|row| {
            let fee_share_raw = BigDecimal::from_str(&row.get::<String, _>("fee_share_raw"))
                .map_err(ApiError::internal)?;
            let amount_native: BigDecimal = row.get("amount_native");
            let amount_usd: BigDecimal = row.get("amount_usd");
            let keep_fraction = (BigDecimal::from(PERCENT_DENOMINATOR) - fee_share_raw.clone())
                / BigDecimal::from(PERCENT_DENOMINATOR);
            let orch_amount_native = amount_native.clone() * keep_fraction.clone();
            let orch_amount_usd = amount_usd.clone() * keep_fraction.clone();
            Ok(PayoutLikeRow {
                block_timestamp: row.get("block_timestamp"),
                tx_hash: row.get("tx_hash"),
                block_number: row.get("block_number"),
                chain_id: row.get("chain_id"),
                from_address: row.get("from_address"),
                valuation_version: row.get("valuation_version"),
                native_usd_price: row.get("native_usd_price"),
                amount_native,
                amount_usd,
                orch_amount_native,
                orch_amount_usd,
                fee_share_percent: raw_percent_to_percent(&fee_share_raw),
                fee_cut_percent: raw_percent_to_percent(
                    &(BigDecimal::from(PERCENT_DENOMINATOR) - fee_share_raw.clone()),
                ),
                keep_fraction,
            })
        })
        .collect()
}

async fn fetch_reward_rows(
    state: &AppState,
    start: NaiveDate,
    end: NaiveDate,
    valuation_version: &str,
    orchestrator: &str,
) -> Result<Vec<RewardCsvRow>, ApiError> {
    let rows = sqlx::query(
        r#"SELECT e.block_timestamp,
                  e.tx_hash,
                  e.block_number,
                  e.chain_id,
                  v.valuation_version,
                  v.amount_native,
                  v.amount_usd,
                  v.native_usd_price,
                  v.pricing_chain,
                  COALESCE(tu.raw_event -> 'decoded' ->> 'rewardCut', '0') AS reward_cut_raw
             FROM raw_protocol_events e
             JOIN event_valuations v
               ON v.event_id = e.id
              AND v.asset = 'LPT'
              AND v.valuation_version = $4
        LEFT JOIN LATERAL (
               SELECT raw_event
                 FROM raw_protocol_events tu
                WHERE tu.chain_id = e.chain_id
                  AND tu.is_canonical = TRUE
                  AND tu.contract_name = 'BondingManager'
                  AND tu.event_name = 'TranscoderUpdate'
                  AND tu.to_address = e.to_address
                  AND (
                        tu.block_number < e.block_number
                        OR (tu.block_number = e.block_number AND tu.log_index <= e.log_index)
                  )
                ORDER BY tu.block_number DESC, tu.log_index DESC
                LIMIT 1
           ) tu ON TRUE
            WHERE e.chain_id = $1
              AND e.event_name = 'Reward'
              AND e.is_canonical = TRUE
              AND e.finality = 'finalized'
              AND e.to_address = $5
              AND e.block_timestamp >= $2::date
              AND e.block_timestamp < ($3::date + INTERVAL '1 day')
         ORDER BY e.block_number ASC, e.log_index ASC"#,
    )
    .bind(state.chain_id)
    .bind(start)
    .bind(end)
    .bind(valuation_version)
    .bind(orchestrator)
    .fetch_all(&state.pg)
    .await?;

    rows.into_iter()
        .map(|row| {
            let reward_cut_raw = BigDecimal::from_str(&row.get::<String, _>("reward_cut_raw"))
                .map_err(ApiError::internal)?;
            let amount_native: BigDecimal = row.get("amount_native");
            let keep_fraction = reward_cut_raw.clone() / BigDecimal::from(PERCENT_DENOMINATOR);
            let orch_tokens = amount_native.clone() * keep_fraction.clone();
            let pricing_chain: Value = row.get("pricing_chain");
            Ok(RewardCsvRow {
                block_timestamp: row.get("block_timestamp"),
                tx_hash: row.get("tx_hash"),
                block_number: row.get("block_number"),
                chain_id: row.get("chain_id"),
                valuation_version: row.get("valuation_version"),
                native_usd_price: row.get("native_usd_price"),
                eth_price: eth_price_from_chain(&pricing_chain),
                amount_native,
                orch_tokens,
                reward_cut_percent: raw_percent_to_percent(&reward_cut_raw),
                keep_fraction,
            })
        })
        .collect()
}

struct TicketHistoryFetch<'a> {
    orchestrator: Option<&'a str>,
    gateway: Option<&'a str>,
    start: Option<NaiveDate>,
    end: Option<NaiveDate>,
    valuation_version: &'a str,
    cursor: Option<Cursor>,
    limit: i64,
}

async fn fetch_ticket_history(
    state: &AppState,
    q: TicketHistoryFetch<'_>,
) -> Result<(Vec<TicketHistoryRow>, Option<String>), ApiError> {
    let TicketHistoryFetch {
        orchestrator,
        gateway,
        start,
        end,
        valuation_version,
        cursor,
        limit,
    } = q;
    let cursor_clause = match cursor {
        Some(_) => "AND (e.block_number, e.log_index) < ($7, $8)",
        None => "",
    };
    let sql = format!(
        r#"SELECT e.id AS event_id,
                  e.tx_hash,
                  e.block_number,
                  e.log_index,
                  e.block_timestamp,
                  e.from_address AS gateway_address,
                  e.to_address AS orchestrator_address,
                  v.amount_native AS face_value,
                  v.amount_usd AS face_value_usd,
                  COALESCE(tu.raw_event -> 'decoded' ->> 'feeShare', '0') AS fee_share_raw,
                  v.valuation_version
             FROM raw_protocol_events e
             JOIN event_valuations v
               ON v.event_id = e.id
              AND v.asset = 'ETH'
              AND v.valuation_version = $4
        LEFT JOIN LATERAL (
               SELECT raw_event
                 FROM raw_protocol_events tu
                WHERE tu.chain_id = e.chain_id
                  AND tu.is_canonical = TRUE
                  AND tu.contract_name = 'BondingManager'
                  AND tu.event_name = 'TranscoderUpdate'
                  AND tu.to_address = e.to_address
                  AND (
                        tu.block_number < e.block_number
                        OR (tu.block_number = e.block_number AND tu.log_index <= e.log_index)
                  )
                ORDER BY tu.block_number DESC, tu.log_index DESC
                LIMIT 1
           ) tu ON TRUE
            WHERE e.chain_id = $1
              AND e.event_name = 'WinningTicketRedeemed'
              AND e.is_canonical = TRUE
              AND e.finality = 'finalized'
              AND ($2::text IS NULL OR e.to_address = $2)
              AND ($3::text IS NULL OR e.from_address = $3)
              AND ($5::date IS NULL OR e.block_timestamp >= $5::date)
              AND ($6::date IS NULL OR e.block_timestamp < ($6::date + INTERVAL '1 day'))
              {cursor_clause}
         ORDER BY e.block_number DESC, e.log_index DESC
            LIMIT $9"#,
    );
    let mut query = sqlx::query(&sql)
        .bind(state.chain_id)
        .bind(orchestrator)
        .bind(gateway)
        .bind(valuation_version)
        .bind(start)
        .bind(end);
    if let Some(cursor) = cursor {
        query = query.bind(cursor.block_number).bind(cursor.log_index);
    } else {
        query = query.bind(0_i64).bind(0_i32);
    }
    query = query.bind(limit + 1);
    let rows = query.fetch_all(&state.pg).await?;

    let has_more = rows.len() as i64 > limit;
    let page_rows = rows.into_iter().take(limit as usize).collect::<Vec<_>>();
    let mut out = Vec::with_capacity(page_rows.len());
    let mut next_cursor = None;
    for row in page_rows {
        let fee_share_raw = BigDecimal::from_str(&row.get::<String, _>("fee_share_raw"))
            .map_err(ApiError::internal)?;
        let fee_cut_raw = BigDecimal::from(PERCENT_DENOMINATOR) - fee_share_raw.clone();
        let event = TicketHistoryRow {
            event_id: row.get::<i64, _>("event_id").to_string(),
            tx_hash: row.get("tx_hash"),
            block_number: row.get::<i64, _>("block_number").to_string(),
            block_timestamp: row.get("block_timestamp"),
            gateway_address: row.get("gateway_address"),
            orchestrator_address: row.get("orchestrator_address"),
            face_value: row
                .get::<BigDecimal, _>("face_value")
                .normalized()
                .to_string(),
            face_value_usd: row
                .get::<BigDecimal, _>("face_value_usd")
                .normalized()
                .to_string(),
            fee_share_percent: raw_percent_to_percent(&fee_share_raw)
                .normalized()
                .to_string(),
            fee_cut_percent: raw_percent_to_percent(&fee_cut_raw)
                .normalized()
                .to_string(),
            valuation_version: row.get("valuation_version"),
        };
        if has_more {
            next_cursor = Some(
                Cursor {
                    block_number: row.get("block_number"),
                    log_index: row.get("log_index"),
                }
                .encode(),
            );
        }
        out.push(event);
    }
    Ok((out, if has_more { next_cursor } else { None }))
}

async fn backfill_complete_for_contract(
    state: &AppState,
    contract_name: &str,
    end: NaiveDate,
) -> Result<bool, ApiError> {
    let latest_ts: Option<DateTime<Utc>> = sqlx::query_scalar(
        r#"SELECT MAX(block_timestamp)
             FROM raw_protocol_events
            WHERE chain_id = $1
              AND contract_name = $2
              AND is_canonical = TRUE"#,
    )
    .bind(state.chain_id)
    .bind(contract_name)
    .fetch_one(&state.pg)
    .await?;
    Ok(latest_ts
        .map(|ts| {
            ts >= (end + Duration::days(1))
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
        })
        .unwrap_or(false))
}

fn csv_response(
    body: String,
    valuation_version: &str,
    backfill_complete: bool,
) -> Result<Response, ApiError> {
    let mut response = (StatusCode::OK, body).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    response.headers_mut().insert(
        "X-Valuation-Version",
        HeaderValue::from_str(valuation_version).map_err(ApiError::internal)?,
    );
    response.headers_mut().insert(
        "X-Backfill-Complete",
        HeaderValue::from_static(if backfill_complete { "true" } else { "false" }),
    );
    Ok(response)
}

fn csv_escape(value: &str) -> String {
    if value.contains([',', '"', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

async fn load_tx_fees(
    state: &AppState,
    tx_hashes: Vec<String>,
) -> Result<HashMap<String, BigDecimal>, ApiError> {
    // Dedupe: a single CSV export may reference the same tx_hash through
    // multiple events. Querying once per unique hash is enough.
    let mut unique: Vec<String> = tx_hashes;
    unique.sort();
    unique.dedup();
    if unique.is_empty() {
        return Ok(HashMap::new());
    }

    // 1. Fast path: bulk lookup against the materialized tx_receipts table.
    //    TD-020 backfill populates this for every finalized canonical tx,
    //    so post-cutover the hit rate trends to 100%.
    let rows = sqlx::query(
        "SELECT tx_hash, tx_fee_eth FROM tx_receipts \
           WHERE chain_id = $1 AND tx_hash = ANY($2)",
    )
    .bind(state.chain_id)
    .bind(&unique)
    .fetch_all(&state.pg)
    .await
    .map_err(ApiError::internal)?;

    let mut fees: HashMap<String, BigDecimal> = rows
        .into_iter()
        .map(|r| {
            let h: String = r.get("tx_hash");
            let v: BigDecimal = r.get("tx_fee_eth");
            (h, v)
        })
        .collect();

    // 2. Fallback path: any tx not yet backfilled (or new since the last
    //    follow iteration). Fetch via cached RPC with bounded fan-out.
    //    Once Phase E confirms 100% backfill coverage and the follow loop
    //    is stable for >=1 week, this whole block can be deleted (see plan
    //    "Open question 3" — keep-then-delete in TD-020.5).
    let missing: Vec<String> = unique
        .iter()
        .filter(|h| !fees.contains_key(*h))
        .cloned()
        .collect();
    if !missing.is_empty() {
        let extras: Vec<(String, BigDecimal)> = stream::iter(missing.into_iter().map(|h| {
            let state = state.clone();
            async move {
                let outcome = livepeer_core::rpc::cross_check::single_call_cached(
                    &state.pg,
                    &state.archive,
                    "eth_getTransactionReceipt",
                    &json!([h]),
                    None,
                )
                .await
                .map_err(ApiError::internal)?;
                let receipt: Value =
                    serde_json::from_slice(&outcome.response_bytes).map_err(ApiError::internal)?;
                let fee = parse_tx_fee_from_receipt(&receipt)
                    .ok_or_else(|| ApiError::internal(format!("malformed receipt for {h}")))?;
                Ok::<(String, BigDecimal), ApiError>((h, fee))
            }
        }))
        .buffer_unordered(TX_FEE_RPC_FALLBACK_CONCURRENCY)
        .try_collect()
        .await?;
        fees.extend(extras);
    }

    Ok(fees)
}

fn parse_tx_fee_from_receipt(receipt: &Value) -> Option<BigDecimal> {
    let gas_used = receipt.get("gasUsed")?.as_str()?;
    let gas_price = receipt
        .get("effectiveGasPrice")
        .and_then(Value::as_str)
        .or_else(|| receipt.get("gasPrice").and_then(Value::as_str))?;
    let gas_used = parse_u256_hex(gas_used)?;
    let gas_price = parse_u256_hex(gas_price)?;
    Some(wei_to_eth_decimal(gas_used.saturating_mul(gas_price)))
}

fn parse_u256_hex(raw: &str) -> Option<U256> {
    U256::from_str_radix(raw.trim_start_matches("0x"), 16).ok()
}

fn wei_to_eth_decimal(wei: U256) -> BigDecimal {
    BigDecimal::from_str(&wei.to_string()).unwrap_or_else(|_| BigDecimal::from(0))
        / BigDecimal::from(10_u64.pow(18))
}

fn arbiscan_tx_url(tx_hash: &str) -> String {
    format!("{ARBISCAN_TX_BASE_URL}{tx_hash}")
}

fn raw_percent_to_percent(raw: &BigDecimal) -> BigDecimal {
    raw.clone() / BigDecimal::from(10_000)
}

fn eth_price_from_chain(chain: &Value) -> BigDecimal {
    let rows = chain
        .as_array()
        .or_else(|| chain.get("steps").and_then(Value::as_array));
    rows.and_then(|rows| {
        rows.iter().find_map(|row| {
            let asset = row.get("asset")?.as_str()?;
            let quote = row.get("quote")?.as_str()?;
            if (asset == "WETH" || asset == "ETH") && quote == "USD" {
                row.get("price")
                    .and_then(|v| v.as_str())
                    .and_then(|s| BigDecimal::from_str(s).ok())
            } else {
                None
            }
        })
    })
    .unwrap_or_else(|| BigDecimal::from(0))
}

fn validate_chain_id(default_chain_id: i64, requested: Option<i64>) -> Result<(), ApiError> {
    if let Some(chain_id) = requested {
        if chain_id != default_chain_id {
            return Err(ApiError::bad_request(format!(
                "unsupported chain_id {chain_id}; use {default_chain_id}"
            )));
        }
    }
    Ok(())
}

fn normalize_required_addr(raw: Option<&str>, field: &str) -> Result<String, ApiError> {
    normalize_addr(raw.ok_or_else(|| ApiError::bad_request(format!("{field} is required")))?)
}

fn normalize_addr(raw: &str) -> Result<String, ApiError> {
    let lowered = raw.to_lowercase();
    if lowered.len() != 42 || !lowered.starts_with("0x") {
        return Err(ApiError::bad_request("invalid address"));
    }
    Ok(lowered)
}

fn parse_date(raw: &str, field: &str) -> Result<NaiveDate, ApiError> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .map_err(|_| ApiError::bad_request(format!("invalid {field} {raw:?}; use YYYY-MM-DD")))
}

fn validate_range(start: NaiveDate, end: NaiveDate) -> Result<(), ApiError> {
    if end < start {
        return Err(ApiError::bad_request("end must be >= start"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::eth_price_from_chain;
    use bigdecimal::BigDecimal;
    use serde_json::json;
    use std::str::FromStr;

    #[test]
    fn eth_price_from_chain_reads_steps_object_shape() {
        let chain = json!({
            "steps": [
                {
                    "asset": "LPT",
                    "quote": "WETH",
                    "price": "0.0009"
                },
                {
                    "asset": "WETH",
                    "quote": "USD",
                    "price": "2375.58877817"
                }
            ]
        });

        assert_eq!(
            eth_price_from_chain(&chain),
            BigDecimal::from_str("2375.58877817").unwrap()
        );
    }
}
