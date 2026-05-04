//! Stake endpoints. SPEC §14.3.4.
//!
//! Returns the stake snapshot at-or-before the requested block, with
//! `staleness_blocks` indicating how stale the answer is. Per SPEC §14.3.4 (Scope 2)
//! staleness is bounded by the delegator's event activity — between events we don't
//! re-fan-out pendingStake reads (that's a v2 concern).

use crate::{error::ApiError, state::AppState};
use axum::{extract::{Path, Query, State}, Json};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;

#[derive(Debug, Serialize)]
pub struct StakeRow {
    pub chain_id: String,
    pub delegator_address: String,
    pub delegate_address: String,
    pub block_number: String,
    pub block_timestamp: DateTime<Utc>,
    pub block_hash: String,
    pub bonded_principal: String,
    pub pending_stake: Option<String>,
    pub pending_fees: Option<String>,
    pub pending_round: Option<String>,
    pub source: String,
    /// How many blocks have elapsed since this snapshot was taken vs the requested block.
    pub staleness_blocks: String,
}

pub async fn at_block(
    State(state): State<AppState>,
    Path((delegator, block)): Path<(String, i64)>,
) -> Result<Json<StakeRow>, ApiError> {
    let delegator_lower = delegator.to_lowercase();
    let row = sqlx::query(
        r#"SELECT chain_id, delegator_address, delegate_address,
                  block_number, block_timestamp, block_hash,
                  bonded_principal, pending_stake, pending_fees, pending_round, source
             FROM stake_balances_by_block
            WHERE chain_id = $1 AND delegator_address = $2 AND block_number <= $3
            ORDER BY block_number DESC
            LIMIT 1"#,
    )
    .bind(state.chain_id)
    .bind(&delegator_lower)
    .bind(block)
    .fetch_optional(&state.pg)
    .await?;
    let Some(r) = row else {
        return Err(ApiError::not_found(format!(
            "no stake snapshot for {delegator_lower} at-or-before block {block}"
        )));
    };
    let snap_block: i64 = r.get("block_number");
    Ok(Json(to_stake_row(&r, block - snap_block)))
}

#[derive(Debug, Default, Deserialize)]
pub struct StakeRangeQuery {
    pub from_block: i64,
    pub to_block: i64,
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct StakeRangeResponse {
    pub data: Vec<StakeRow>,
}

#[derive(Debug, Serialize)]
pub struct DelegatorStakeRow {
    pub delegator_address: String,
    pub delegate_address: String,
    pub block_number: String,
    pub block_timestamp: DateTime<Utc>,
    pub block_hash: String,
    pub bonded_principal: String,
    pub pending_stake: Option<String>,
    pub pending_fees: Option<String>,
    pub pending_round: Option<String>,
    pub source: String,
    pub staleness_blocks: String,
}

#[derive(Debug, Serialize)]
pub struct DelegatorListResponse {
    pub transcoder_address: String,
    pub requested_block: String,
    pub delegator_count: String,
    pub total_bonded_principal: String,
    pub data: Vec<DelegatorStakeRow>,
}

pub async fn range(
    State(state): State<AppState>,
    Path(delegator): Path<String>,
    Query(q): Query<StakeRangeQuery>,
) -> Result<Json<StakeRangeResponse>, ApiError> {
    if q.to_block < q.from_block {
        return Err(ApiError::bad_request("to_block < from_block"));
    }
    let delegator_lower = delegator.to_lowercase();
    let limit = q.limit.unwrap_or(1000).min(10_000) as i64;
    let rows = sqlx::query(
        r#"SELECT chain_id, delegator_address, delegate_address,
                  block_number, block_timestamp, block_hash,
                  bonded_principal, pending_stake, pending_fees, pending_round, source
             FROM stake_balances_by_block
            WHERE chain_id = $1 AND delegator_address = $2
              AND block_number BETWEEN $3 AND $4
            ORDER BY block_number ASC
            LIMIT $5"#,
    )
    .bind(state.chain_id)
    .bind(&delegator_lower)
    .bind(q.from_block)
    .bind(q.to_block)
    .bind(limit)
    .fetch_all(&state.pg)
    .await?;

    Ok(Json(StakeRangeResponse {
        data: rows.iter().map(|r| to_stake_row(r, 0)).collect(),
    }))
}

pub async fn delegators_at_block(
    State(state): State<AppState>,
    Path((transcoder, block)): Path<(String, i64)>,
) -> Result<Json<DelegatorListResponse>, ApiError> {
    let transcoder_lower = transcoder.to_lowercase();
    let rows = sqlx::query(
        r#"WITH latest AS (
               SELECT DISTINCT ON (delegator_address)
                      delegator_address, delegate_address,
                      block_number, block_timestamp, block_hash,
                      bonded_principal, pending_stake, pending_fees, pending_round, source
                 FROM stake_balances_by_block
                WHERE chain_id = $1
                  AND block_number <= $2
                ORDER BY delegator_address, block_number DESC
           )
           SELECT delegator_address, delegate_address,
                  block_number, block_timestamp, block_hash,
                  bonded_principal, pending_stake, pending_fees, pending_round, source
             FROM latest
            WHERE delegate_address = $3
              AND bonded_principal > 0
            ORDER BY bonded_principal DESC, delegator_address ASC"#,
    )
    .bind(state.chain_id)
    .bind(block)
    .bind(&transcoder_lower)
    .fetch_all(&state.pg)
    .await?;

    let mut total = BigDecimal::from(0);
    let data: Vec<DelegatorStakeRow> = rows
        .iter()
        .map(|r| {
            let snap_block: i64 = r.get("block_number");
            let bonded: BigDecimal = r.get("bonded_principal");
            total += bonded.clone();
            DelegatorStakeRow {
                delegator_address: r.get("delegator_address"),
                delegate_address: r.get("delegate_address"),
                block_number: snap_block.to_string(),
                block_timestamp: r.get("block_timestamp"),
                block_hash: r.get("block_hash"),
                bonded_principal: bonded.to_string(),
                pending_stake: r.try_get::<BigDecimal, _>("pending_stake").ok().map(|v| v.to_string()),
                pending_fees: r.try_get::<BigDecimal, _>("pending_fees").ok().map(|v| v.to_string()),
                pending_round: r.try_get::<i64, _>("pending_round").ok().map(|v| v.to_string()),
                source: r.get("source"),
                staleness_blocks: (block - snap_block).to_string(),
            }
        })
        .collect();

    Ok(Json(DelegatorListResponse {
        transcoder_address: transcoder_lower,
        requested_block: block.to_string(),
        delegator_count: data.len().to_string(),
        total_bonded_principal: total.to_string(),
        data,
    }))
}

fn to_stake_row(r: &sqlx::postgres::PgRow, staleness_blocks: i64) -> StakeRow {
    StakeRow {
        chain_id: r.get::<i64, _>("chain_id").to_string(),
        delegator_address: r.get("delegator_address"),
        delegate_address: r.get("delegate_address"),
        block_number: r.get::<i64, _>("block_number").to_string(),
        block_timestamp: r.get("block_timestamp"),
        block_hash: r.get("block_hash"),
        bonded_principal: r.get::<BigDecimal, _>("bonded_principal").to_string(),
        pending_stake: r.try_get::<BigDecimal, _>("pending_stake").ok().map(|v| v.to_string()),
        pending_fees: r.try_get::<BigDecimal, _>("pending_fees").ok().map(|v| v.to_string()),
        pending_round: r.try_get::<i64, _>("pending_round").ok().map(|v| v.to_string()),
        source: r.get("source"),
        staleness_blocks: staleness_blocks.to_string(),
    }
}
