use crate::{abi::BondingManager, error::ApiError, state::AppState};
use alloy::primitives::{FixedBytes, LogData};
use alloy::sol_types::SolEvent;
use axum::{
    extract::{Path, Query, State},
    Json,
};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use std::str::FromStr;
use utoipa::{IntoParams, ToSchema};

const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 1_000;
const PERCENT_DENOMINATOR: i64 = 10_000;

#[derive(Debug, Deserialize, IntoParams, ToSchema)]
#[schema(description = "Shared history query for transcoder parameter and lifecycle endpoints.")]
pub struct HistoryQuery {
    /// Optional lower block bound.
    pub from_block: Option<i64>,
    /// Optional upper block bound.
    pub to_block: Option<i64>,
    /// Maximum number of history rows to return.
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(
    description = "Decoded TranscoderUpdate row describing reward-cut and fee-share policy at a block."
)]
pub struct TranscoderParamsRow {
    pub event_id: String,
    pub transcoder_address: String,
    pub block_number: String,
    pub block_timestamp: DateTime<Utc>,
    pub tx_hash: String,
    pub log_index: u32,
    /// Raw on-chain `rewardCut` scaled by 1_000_000. This is already the operator keep.
    pub reward_cut_raw: String,
    /// Operator-perspective reward cut percentage (raw / 10_000).
    pub reward_cut_percent: String,
    /// Raw on-chain `feeShare` scaled by 1_000_000. This is the delegators' share.
    pub fee_share_raw: String,
    /// Protocol-perspective fee share percentage (delegators' share).
    pub fee_share_percent: String,
    /// Operator-perspective fee cut percentage (`100 - fee_share_percent`).
    pub fee_cut_percent: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Historical collection of transcoder parameter rows.")]
pub struct TranscoderParamsHistoryResponse {
    pub data: Vec<TranscoderParamsRow>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Decoded activation or deactivation event for a transcoder.")]
pub struct TranscoderLifecycleRow {
    pub event_id: String,
    pub transcoder_address: String,
    pub block_number: String,
    pub block_timestamp: DateTime<Utc>,
    pub tx_hash: String,
    pub log_index: u32,
    pub event_name: String,
    pub round: String,
    pub is_active: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Historical collection of transcoder lifecycle rows.")]
pub struct TranscoderLifecycleHistoryResponse {
    pub data: Vec<TranscoderLifecycleRow>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(
    description = "Point-in-time transcoder profile composed from parameter and lifecycle history."
)]
pub struct TranscoderProfileResponse {
    pub transcoder_address: String,
    pub block_number: String,
    pub params: Option<TranscoderParamsRow>,
    pub lifecycle: Option<TranscoderLifecycleRow>,
}

#[utoipa::path(
    get,
    path = "/transcoders/{transcoder}/params/latest",
    tag = "Transcoders",
    params(
        ("transcoder" = String, Path, description = "Transcoder/orchestrator address.")
    ),
    responses(
        (status = 200, description = "Most recent TranscoderUpdate event for the transcoder.", body = TranscoderParamsRow),
        (status = 404, description = "No transcoder parameter history found.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn latest(
    State(state): State<AppState>,
    Path(transcoder): Path<String>,
) -> Result<Json<TranscoderParamsRow>, ApiError> {
    let transcoder = normalize_addr(&transcoder)?;
    let rows = load_updates(&state, &transcoder, None, None, 1).await?;
    let Some(row) = rows.into_iter().next() else {
        return Err(ApiError::not_found(format!(
            "no TranscoderUpdate events found for {transcoder}"
        )));
    };
    Ok(Json(row))
}

#[utoipa::path(
    get,
    path = "/transcoders/{transcoder}/params/block/{block}",
    tag = "Transcoders",
    params(
        ("transcoder" = String, Path, description = "Transcoder/orchestrator address."),
        ("block" = i64, Path, description = "Return the latest parameter change at or before this block.")
    ),
    responses(
        (status = 200, description = "Transcoder parameters effective at the requested block.", body = TranscoderParamsRow),
        (status = 404, description = "No parameter event exists at or before the requested block.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn at_block(
    State(state): State<AppState>,
    Path((transcoder, block)): Path<(String, i64)>,
) -> Result<Json<TranscoderParamsRow>, ApiError> {
    let transcoder = normalize_addr(&transcoder)?;
    let rows = load_updates(&state, &transcoder, None, Some(block), 1).await?;
    let Some(row) = rows.into_iter().next() else {
        return Err(ApiError::not_found(format!(
            "no TranscoderUpdate at or before block {block} for {transcoder}"
        )));
    };
    Ok(Json(row))
}

#[utoipa::path(
    get,
    path = "/transcoders/{transcoder}/params/history",
    tag = "Transcoders",
    params(
        ("transcoder" = String, Path, description = "Transcoder/orchestrator address."),
        HistoryQuery
    ),
    responses(
        (status = 200, description = "Historical TranscoderUpdate rows for a transcoder.", body = TranscoderParamsHistoryResponse),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn history(
    State(state): State<AppState>,
    Path(transcoder): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<TranscoderParamsHistoryResponse>, ApiError> {
    let transcoder = normalize_addr(&transcoder)?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as i64;
    let rows = load_updates(&state, &transcoder, q.from_block, q.to_block, limit).await?;
    Ok(Json(TranscoderParamsHistoryResponse { data: rows }))
}

#[utoipa::path(
    get,
    path = "/transcoders/{transcoder}/lifecycle/latest",
    tag = "Transcoders",
    params(
        ("transcoder" = String, Path, description = "Transcoder/orchestrator address.")
    ),
    responses(
        (status = 200, description = "Most recent activation or deactivation event.", body = TranscoderLifecycleRow),
        (status = 404, description = "No lifecycle events found for the transcoder.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn lifecycle_latest(
    State(state): State<AppState>,
    Path(transcoder): Path<String>,
) -> Result<Json<TranscoderLifecycleRow>, ApiError> {
    let transcoder = normalize_addr(&transcoder)?;
    let rows = load_lifecycle_updates(&state, &transcoder, None, None, 1).await?;
    let Some(row) = rows.into_iter().next() else {
        return Err(ApiError::not_found(format!(
            "no lifecycle events found for {transcoder}"
        )));
    };
    Ok(Json(row))
}

#[utoipa::path(
    get,
    path = "/transcoders/{transcoder}/lifecycle/block/{block}",
    tag = "Transcoders",
    params(
        ("transcoder" = String, Path, description = "Transcoder/orchestrator address."),
        ("block" = i64, Path, description = "Return the latest lifecycle event at or before this block.")
    ),
    responses(
        (status = 200, description = "Lifecycle state effective at the requested block.", body = TranscoderLifecycleRow),
        (status = 404, description = "No lifecycle event exists at or before the requested block.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn lifecycle_at_block(
    State(state): State<AppState>,
    Path((transcoder, block)): Path<(String, i64)>,
) -> Result<Json<TranscoderLifecycleRow>, ApiError> {
    let transcoder = normalize_addr(&transcoder)?;
    let rows = load_lifecycle_updates(&state, &transcoder, None, Some(block), 1).await?;
    let Some(row) = rows.into_iter().next() else {
        return Err(ApiError::not_found(format!(
            "no lifecycle event at or before block {block} for {transcoder}"
        )));
    };
    Ok(Json(row))
}

#[utoipa::path(
    get,
    path = "/transcoders/{transcoder}/lifecycle/history",
    tag = "Transcoders",
    params(
        ("transcoder" = String, Path, description = "Transcoder/orchestrator address."),
        HistoryQuery
    ),
    responses(
        (status = 200, description = "Activation and deactivation history for a transcoder.", body = TranscoderLifecycleHistoryResponse),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn lifecycle_history(
    State(state): State<AppState>,
    Path(transcoder): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<TranscoderLifecycleHistoryResponse>, ApiError> {
    let transcoder = normalize_addr(&transcoder)?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as i64;
    let rows = load_lifecycle_updates(&state, &transcoder, q.from_block, q.to_block, limit).await?;
    Ok(Json(TranscoderLifecycleHistoryResponse { data: rows }))
}

#[utoipa::path(
    get,
    path = "/transcoders/{transcoder}/profile/block/{block}",
    tag = "Transcoders",
    params(
        ("transcoder" = String, Path, description = "Transcoder/orchestrator address."),
        ("block" = i64, Path, description = "Return transcoder params and lifecycle state effective at this block.")
    ),
    responses(
        (status = 200, description = "Point-in-time transcoder profile composed from parameter and lifecycle history.", body = TranscoderProfileResponse),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn profile_at_block(
    State(state): State<AppState>,
    Path((transcoder, block)): Path<(String, i64)>,
) -> Result<Json<TranscoderProfileResponse>, ApiError> {
    let transcoder = normalize_addr(&transcoder)?;
    let params = load_updates(&state, &transcoder, None, Some(block), 1)
        .await?
        .into_iter()
        .next();
    let lifecycle = load_lifecycle_updates(&state, &transcoder, None, Some(block), 1)
        .await?
        .into_iter()
        .next();
    Ok(Json(TranscoderProfileResponse {
        transcoder_address: transcoder,
        block_number: block.to_string(),
        params,
        lifecycle,
    }))
}

async fn load_updates(
    state: &AppState,
    transcoder: &str,
    from_block: Option<i64>,
    to_block: Option<i64>,
    limit: i64,
) -> Result<Vec<TranscoderParamsRow>, ApiError> {
    let topic1 = topic_for_address(transcoder)?;
    let sql = r#"SELECT id, block_number, block_timestamp, tx_hash, log_index, raw_event
                   FROM raw_protocol_events
                  WHERE chain_id = $1
                    AND is_canonical = TRUE
                    AND event_name = 'TranscoderUpdate'
                    AND raw_event->'topics'->>1 = $2
                    AND ($3::bigint IS NULL OR block_number >= $3)
                    AND ($4::bigint IS NULL OR block_number <= $4)
                  ORDER BY block_number DESC, log_index DESC
                  LIMIT $5"#;
    let rows = sqlx::query(sql)
        .bind(state.chain_id)
        .bind(topic1)
        .bind(from_block)
        .bind(to_block)
        .bind(limit)
        .fetch_all(&state.pg)
        .await?;

    let mut out = Vec::new();
    for row in rows {
        let raw_event: Value = row.get("raw_event");
        let decoded = decode_transcoder_update(&raw_event)?;
        out.push(TranscoderParamsRow {
            event_id: row.get::<i64, _>("id").to_string(),
            transcoder_address: decoded.transcoder,
            block_number: row.get::<i64, _>("block_number").to_string(),
            block_timestamp: row.get("block_timestamp"),
            tx_hash: row.get("tx_hash"),
            log_index: row.get::<i32, _>("log_index") as u32,
            reward_cut_raw: decoded.reward_cut_raw.clone(),
            reward_cut_percent: scaled_percent(&decoded.reward_cut_raw)?,
            fee_share_raw: decoded.fee_share_raw.clone(),
            fee_share_percent: scaled_percent(&decoded.fee_share_raw)?,
            fee_cut_percent: inverse_scaled_percent(&decoded.fee_share_raw)?,
        });
        if out.len() >= limit as usize {
            break;
        }
    }
    Ok(out)
}

#[derive(Debug)]
struct DecodedTranscoderUpdate {
    transcoder: String,
    reward_cut_raw: String,
    fee_share_raw: String,
}

#[derive(Debug)]
struct DecodedLifecycle {
    transcoder: String,
    round: String,
    is_active: bool,
}

fn decode_transcoder_update(raw_event: &Value) -> Result<DecodedTranscoderUpdate, ApiError> {
    if let Some(decoded) = raw_event.get("decoded") {
        let transcoder = decoded
            .get("transcoder")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::internal("decoded transcoder missing"))?
            .to_lowercase();
        let reward_cut_raw = decoded
            .get("rewardCut")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::internal("decoded rewardCut missing"))?
            .to_string();
        let fee_share_raw = decoded
            .get("feeShare")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::internal("decoded feeShare missing"))?
            .to_string();
        return Ok(DecodedTranscoderUpdate {
            transcoder,
            reward_cut_raw,
            fee_share_raw,
        });
    }

    let raw: RawLog = serde_json::from_value(raw_event.clone())
        .map_err(|e| ApiError::internal(format!("decoding raw TranscoderUpdate log: {e}")))?;
    let log_data = build_log_data(&raw)?;
    let decoded = BondingManager::TranscoderUpdate::decode_log_data(&log_data, true)
        .map_err(|e| ApiError::internal(format!("ABI-decoding TranscoderUpdate: {e}")))?;
    Ok(DecodedTranscoderUpdate {
        transcoder: format!("{:#x}", decoded.transcoder).to_lowercase(),
        reward_cut_raw: decoded.rewardCut.to_string(),
        fee_share_raw: decoded.feeShare.to_string(),
    })
}

async fn load_lifecycle_updates(
    state: &AppState,
    transcoder: &str,
    from_block: Option<i64>,
    to_block: Option<i64>,
    limit: i64,
) -> Result<Vec<TranscoderLifecycleRow>, ApiError> {
    let topic1 = topic_for_address(transcoder)?;
    let sql = r#"SELECT id, block_number, block_timestamp, tx_hash, log_index, event_name, raw_event
                   FROM raw_protocol_events
                  WHERE chain_id = $1
                    AND is_canonical = TRUE
                    AND event_name IN ('TranscoderActivated', 'TranscoderDeactivated')
                    AND raw_event->'topics'->>1 = $2
                    AND ($3::bigint IS NULL OR block_number >= $3)
                    AND ($4::bigint IS NULL OR block_number <= $4)
                  ORDER BY block_number DESC, log_index DESC
                  LIMIT $5"#;
    let rows = sqlx::query(sql)
        .bind(state.chain_id)
        .bind(topic1)
        .bind(from_block)
        .bind(to_block)
        .bind(limit)
        .fetch_all(&state.pg)
        .await?;

    let mut out = Vec::new();
    for row in rows {
        let raw_event: Value = row.get("raw_event");
        let decoded = decode_lifecycle(&row.get::<String, _>("event_name"), &raw_event)?;
        out.push(TranscoderLifecycleRow {
            event_id: row.get::<i64, _>("id").to_string(),
            transcoder_address: decoded.transcoder,
            block_number: row.get::<i64, _>("block_number").to_string(),
            block_timestamp: row.get("block_timestamp"),
            tx_hash: row.get("tx_hash"),
            log_index: row.get::<i32, _>("log_index") as u32,
            event_name: row.get("event_name"),
            round: decoded.round,
            is_active: decoded.is_active,
        });
        if out.len() >= limit as usize {
            break;
        }
    }
    Ok(out)
}

fn decode_lifecycle(event_name: &str, raw_event: &Value) -> Result<DecodedLifecycle, ApiError> {
    if let Some(decoded) = raw_event.get("decoded") {
        let transcoder = decoded
            .get("transcoder")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::internal("decoded transcoder missing"))?
            .to_lowercase();
        let (round_key, is_active) = match event_name {
            "TranscoderActivated" => ("activationRound", true),
            "TranscoderDeactivated" => ("deactivationRound", false),
            _ => {
                return Err(ApiError::internal(format!(
                    "unsupported lifecycle event {event_name}"
                )))
            }
        };
        let round = decoded
            .get(round_key)
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::internal(format!("decoded {round_key} missing")))?
            .to_string();
        return Ok(DecodedLifecycle {
            transcoder,
            round,
            is_active,
        });
    }

    let raw: RawLog = serde_json::from_value(raw_event.clone())
        .map_err(|e| ApiError::internal(format!("decoding raw lifecycle log: {e}")))?;
    let log_data = build_log_data(&raw)?;
    match event_name {
        "TranscoderActivated" => {
            let decoded = BondingManager::TranscoderActivated::decode_log_data(&log_data, true)
                .map_err(|e| {
                    ApiError::internal(format!("ABI-decoding TranscoderActivated: {e}"))
                })?;
            Ok(DecodedLifecycle {
                transcoder: format!("{:#x}", decoded.transcoder).to_lowercase(),
                round: decoded.activationRound.to_string(),
                is_active: true,
            })
        }
        "TranscoderDeactivated" => {
            let decoded = BondingManager::TranscoderDeactivated::decode_log_data(&log_data, true)
                .map_err(|e| {
                ApiError::internal(format!("ABI-decoding TranscoderDeactivated: {e}"))
            })?;
            Ok(DecodedLifecycle {
                transcoder: format!("{:#x}", decoded.transcoder).to_lowercase(),
                round: decoded.deactivationRound.to_string(),
                is_active: false,
            })
        }
        _ => Err(ApiError::internal(format!(
            "unsupported lifecycle event {event_name}"
        ))),
    }
}

fn build_log_data(raw: &RawLog) -> Result<LogData, ApiError> {
    let topics_b256: Vec<FixedBytes<32>> = raw
        .topics
        .iter()
        .map(|t| FixedBytes::<32>::from_str(t.trim_start_matches("0x")))
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| ApiError::internal(format!("decoding topic bytes: {e}")))?;
    let data_bytes = alloy::hex::decode(raw.data.trim_start_matches("0x"))
        .map_err(|e| ApiError::internal(format!("decoding data hex: {e}")))?;
    LogData::new(topics_b256, data_bytes.into())
        .ok_or_else(|| ApiError::internal("malformed LogData (topics/data shape)"))
}

fn normalize_addr(s: &str) -> Result<String, ApiError> {
    let lower = s.to_lowercase();
    if lower.starts_with("0x") && lower.len() == 42 {
        Ok(lower)
    } else {
        Err(ApiError::bad_request(format!("invalid address: {s}")))
    }
}

fn topic_for_address(addr: &str) -> Result<String, ApiError> {
    let lower = normalize_addr(addr)?;
    Ok(format!(
        "0x000000000000000000000000{}",
        lower.trim_start_matches("0x")
    ))
}

fn scaled_percent(raw: &str) -> Result<String, ApiError> {
    let n = BigDecimal::from_str(raw)
        .map_err(|e| ApiError::internal(format!("parsing percentage value: {e}")))?;
    Ok((n / BigDecimal::from(PERCENT_DENOMINATOR))
        .normalized()
        .to_string())
}

fn inverse_scaled_percent(raw: &str) -> Result<String, ApiError> {
    let share = BigDecimal::from_str(raw)
        .map_err(|e| ApiError::internal(format!("parsing percentage value: {e}")))?;
    Ok(
        ((BigDecimal::from(1_000_000_i64) - share) / BigDecimal::from(PERCENT_DENOMINATOR))
            .normalized()
            .to_string(),
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawLog {
    topics: Vec<String>,
    data: String,
}
