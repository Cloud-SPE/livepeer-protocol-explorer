//! Per-delegator endpoints (TD-027).
//!
//! `stake_balances_by_block` is per (delegator, delegate, block); the
//! current portfolio is the latest snapshot per (delegator, delegate).

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
#[schema(description = "One delegation held by a delegator (current state).")]
pub struct DelegationRow {
    /// Address of the orchestrator the delegator is bonded to.
    pub delegate_address: String,
    /// Bonded principal (LPT) at the most recent observed block.
    pub bonded_principal: String,
    /// Pending stake from BondingManager.pendingStake (LPT, may be null
    /// if the staker pending-refresh worker hasn't observed this row yet).
    pub pending_stake: Option<String>,
    /// Pending fees from BondingManager.pendingFees (ETH).
    pub pending_fees: Option<String>,
    pub pending_round: Option<String>,
    pub as_of_block: String,
    pub as_of_timestamp: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Delegator portfolio with one entry per delegation.")]
pub struct DelegatorResponse {
    pub delegator_address: String,
    /// True if the delegator has at least one bonded delegation that has
    /// not been fully unbonded.
    pub is_active: bool,
    pub first_bond_block: String,
    pub last_seen_block: String,
    /// Delegations the delegator currently holds. Ordered by
    /// `bonded_principal DESC`.
    pub delegations: Vec<DelegationRow>,
    pub chain_id: String,
}

fn normalize_addr(s: &str) -> Result<String, ApiError> {
    let lower = s.to_lowercase();
    if lower.starts_with("0x") && lower.len() == 42 {
        Ok(lower)
    } else {
        Err(ApiError::bad_request(format!("invalid address: {s}")))
    }
}

#[utoipa::path(
    get,
    path = "/delegators/{address}",
    tag = "Delegators",
    params(
        ("address" = String, Path, description = "Delegator address.")
    ),
    responses(
        (status = 200, description = "Per-delegator portfolio of current delegations.", body = DelegatorResponse),
        (status = 404, description = "Delegator address not present in delegator_registry.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn get(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Result<Json<DelegatorResponse>, ApiError> {
    let address = normalize_addr(&address)?;

    let registry = sqlx::query(
        r#"SELECT first_bond_block, last_seen_block, is_active
             FROM delegator_registry
            WHERE chain_id = $1 AND delegator_address = $2"#,
    )
    .bind(state.chain_id)
    .bind(&address)
    .fetch_optional(&state.pg)
    .await?;

    let Some(reg) = registry else {
        return Err(ApiError::not_found("delegator not found"));
    };

    // Latest snapshot per (delegator, delegate). Skip self-bonded rows
    // where bonded_principal is zero — those are residual ghosts from
    // unbond-everything events.
    let rows = sqlx::query(
        r#"
        SELECT DISTINCT ON (delegate_address)
               delegate_address,
               bonded_principal,
               pending_stake,
               pending_fees,
               pending_round,
               block_number,
               block_timestamp
          FROM stake_balances_by_block
         WHERE chain_id = $1 AND delegator_address = $2
         ORDER BY delegate_address, block_number DESC
        "#,
    )
    .bind(state.chain_id)
    .bind(&address)
    .fetch_all(&state.pg)
    .await?;

    let mut delegations: Vec<DelegationRow> = rows
        .iter()
        .filter_map(|r| {
            let principal: BigDecimal = r.get("bonded_principal");
            // Hide zero-bond rows from the portfolio view; they exist as
            // historical state but mean "fully unbonded" for current purposes.
            if principal == BigDecimal::from(0) {
                return None;
            }
            Some(DelegationRow {
                delegate_address: r.get::<String, _>("delegate_address"),
                bonded_principal: principal.normalized().to_string(),
                pending_stake: r
                    .try_get::<Option<BigDecimal>, _>("pending_stake")
                    .ok()
                    .flatten()
                    .map(|v| v.normalized().to_string()),
                pending_fees: r
                    .try_get::<Option<BigDecimal>, _>("pending_fees")
                    .ok()
                    .flatten()
                    .map(|v| v.normalized().to_string()),
                pending_round: r
                    .try_get::<Option<i64>, _>("pending_round")
                    .ok()
                    .flatten()
                    .map(|v| v.to_string()),
                as_of_block: r.get::<i64, _>("block_number").to_string(),
                as_of_timestamp: r.get("block_timestamp"),
            })
        })
        .collect();

    // Sort by descending bonded_principal. Re-parse as BigDecimal so ordering
    // is exact rather than f64-lossy.
    delegations.sort_by(|a, b| {
        use std::str::FromStr;
        let ba = BigDecimal::from_str(&a.bonded_principal).unwrap_or_default();
        let bb = BigDecimal::from_str(&b.bonded_principal).unwrap_or_default();
        bb.cmp(&ba)
    });

    Ok(Json(DelegatorResponse {
        delegator_address: address,
        is_active: reg.get("is_active"),
        first_bond_block: reg.get::<i64, _>("first_bond_block").to_string(),
        last_seen_block: reg.get::<i64, _>("last_seen_block").to_string(),
        delegations,
        chain_id: state.chain_id.to_string(),
    }))
}
