//! Per-delegator endpoints (TD-027 + extensions).
//!
//! `stake_balances_by_block` is per (delegator, delegate, block); the
//! current portfolio is the latest snapshot per (delegator, delegate).

use crate::{cursor::Cursor, error::ApiError, state::AppState};
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

const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 500;

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

    delegations.sort_by(|a, b| {
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

// ─────────────────────────────────────────────────────────────────────────
// /orchestrators/{address}/delegators — A
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "One delegator bonded to a given orchestrator.")]
pub struct OrchDelegatorRow {
    pub delegator_address: String,
    pub bonded_principal: String,
    pub pending_stake: Option<String>,
    pub pending_fees: Option<String>,
    pub pending_round: Option<String>,
    pub as_of_block: String,
    pub as_of_timestamp: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OrchDelegatorsMeta {
    pub chain_id: String,
    pub orchestrator_address: String,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Paginated list of delegators bonded to an orchestrator.")]
pub struct OrchDelegatorsResponse {
    pub data: Vec<OrchDelegatorRow>,
    pub meta: OrchDelegatorsMeta,
}

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
pub struct OrchDelegatorsQuery {
    /// Opaque cursor for stable pagination by `(bonded_principal DESC, delegator_address ASC)`.
    pub cursor: Option<String>,
    /// Maximum number of rows to return (default 50, max 500).
    pub limit: Option<u32>,
}

#[derive(Debug, Clone)]
struct OrchDelegatorsCursor {
    bonded: BigDecimal,
    delegator_address: String,
}

impl OrchDelegatorsCursor {
    fn encode(&self) -> String {
        format!("D{}|{}", self.bonded.normalized(), self.delegator_address)
    }
    fn decode(raw: &str) -> Result<Self, ApiError> {
        let stripped = raw
            .strip_prefix('D')
            .ok_or_else(|| ApiError::bad_request("invalid cursor"))?;
        let (bonded, addr) = stripped
            .split_once('|')
            .ok_or_else(|| ApiError::bad_request("invalid cursor"))?;
        Ok(Self {
            bonded: BigDecimal::from_str(bonded)
                .map_err(|_| ApiError::bad_request("invalid cursor numeric"))?,
            delegator_address: normalize_addr(addr)?,
        })
    }
}

#[utoipa::path(
    get,
    path = "/orchestrators/{address}/delegators",
    tag = "Orchestrator history",
    params(
        ("address" = String, Path, description = "Orchestrator address."),
        OrchDelegatorsQuery
    ),
    responses(
        (status = 200, description = "Paginated list of delegators bonded to the orchestrator, sorted by `bonded_principal DESC`.", body = OrchDelegatorsResponse),
        (status = 400, description = "Invalid address or cursor.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn for_orchestrator(
    State(state): State<AppState>,
    Path(address): Path<String>,
    Query(q): Query<OrchDelegatorsQuery>,
) -> Result<Json<OrchDelegatorsResponse>, ApiError> {
    let orch = normalize_addr(&address)?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as i64;
    let cursor = q
        .cursor
        .as_deref()
        .map(OrchDelegatorsCursor::decode)
        .transpose()?;

    let rows = sqlx::query(
        r#"
        WITH latest AS (
            SELECT DISTINCT ON (delegator_address)
                   delegator_address,
                   bonded_principal,
                   pending_stake,
                   pending_fees,
                   pending_round,
                   block_number,
                   block_timestamp
              FROM stake_balances_by_block
             WHERE chain_id = $1 AND delegate_address = $2
             ORDER BY delegator_address, block_number DESC
        )
        SELECT *
          FROM latest
         WHERE bonded_principal > 0
           AND ($3::numeric IS NULL
                OR bonded_principal < $3
                OR (bonded_principal = $3 AND delegator_address > $4))
         ORDER BY bonded_principal DESC, delegator_address ASC
         LIMIT $5
        "#,
    )
    .bind(state.chain_id)
    .bind(&orch)
    .bind(cursor.as_ref().map(|c| c.bonded.clone()))
    .bind(cursor.as_ref().map(|c| c.delegator_address.clone()))
    .bind(limit)
    .fetch_all(&state.pg)
    .await?;

    let data: Vec<OrchDelegatorRow> = rows
        .iter()
        .map(|r| {
            let principal: BigDecimal = r.get("bonded_principal");
            OrchDelegatorRow {
                delegator_address: r.get::<String, _>("delegator_address"),
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
            }
        })
        .collect();

    let next_cursor = data.last().map(|row| {
        OrchDelegatorsCursor {
            bonded: BigDecimal::from_str(&row.bonded_principal).unwrap_or_default(),
            delegator_address: row.delegator_address.clone(),
        }
        .encode()
    });

    Ok(Json(OrchDelegatorsResponse {
        data,
        meta: OrchDelegatorsMeta {
            chain_id: state.chain_id.to_string(),
            orchestrator_address: orch,
            next_cursor,
        },
    }))
}

// ─────────────────────────────────────────────────────────────────────────
// /delegators — B2 (leaderboard / index)
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "One delegator and their aggregate bonded total.")]
pub struct DelegatorIndexRow {
    pub delegator_address: String,
    /// Sum of `bonded_principal` across all delegations the delegator currently holds.
    pub total_bonded: String,
    /// Count of delegations with non-zero bonded principal.
    pub delegation_count: u32,
    pub is_active: bool,
    pub first_bond_block: Option<String>,
    pub last_seen_block: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DelegatorIndexMeta {
    pub chain_id: String,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Paginated index of delegators sorted by total bonded LPT.")]
pub struct DelegatorIndexResponse {
    pub data: Vec<DelegatorIndexRow>,
    pub meta: DelegatorIndexMeta,
}

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
pub struct DelegatorIndexQuery {
    /// Opaque cursor for stable pagination by `(total_bonded DESC, delegator_address ASC)`.
    pub cursor: Option<String>,
    /// Maximum number of rows to return (default 50, max 500).
    pub limit: Option<u32>,
}

// Cursor reuses OrchDelegatorsCursor format (D<bonded>|<addr>).
type DelegatorIndexCursor = OrchDelegatorsCursor;

#[utoipa::path(
    get,
    path = "/delegators",
    tag = "Delegators",
    params(DelegatorIndexQuery),
    responses(
        (status = 200, description = "Paginated index of delegators sorted by total bonded LPT.", body = DelegatorIndexResponse),
        (status = 400, description = "Invalid cursor.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn list(
    State(state): State<AppState>,
    Query(q): Query<DelegatorIndexQuery>,
) -> Result<Json<DelegatorIndexResponse>, ApiError> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as i64;
    let cursor = q
        .cursor
        .as_deref()
        .map(DelegatorIndexCursor::decode)
        .transpose()?;

    let rows = sqlx::query(
        r#"
        WITH latest AS (
            SELECT DISTINCT ON (delegator_address, delegate_address)
                   delegator_address,
                   delegate_address,
                   bonded_principal
              FROM stake_balances_by_block
             WHERE chain_id = $1
             ORDER BY delegator_address, delegate_address, block_number DESC
        ), agg AS (
            SELECT delegator_address,
                   SUM(bonded_principal) AS total_bonded,
                   COUNT(*) FILTER (WHERE bonded_principal > 0) AS delegation_count
              FROM latest
             GROUP BY delegator_address
            HAVING SUM(bonded_principal) > 0
        )
        SELECT a.delegator_address,
               a.total_bonded,
               a.delegation_count,
               COALESCE(r.is_active, FALSE)             AS is_active,
               r.first_bond_block,
               r.last_seen_block
          FROM agg a
     LEFT JOIN delegator_registry r
            ON r.chain_id = $1 AND r.delegator_address = a.delegator_address
         WHERE ($2::numeric IS NULL
                OR a.total_bonded < $2
                OR (a.total_bonded = $2 AND a.delegator_address > $3))
         ORDER BY a.total_bonded DESC, a.delegator_address ASC
         LIMIT $4
        "#,
    )
    .bind(state.chain_id)
    .bind(cursor.as_ref().map(|c| c.bonded.clone()))
    .bind(cursor.as_ref().map(|c| c.delegator_address.clone()))
    .bind(limit)
    .fetch_all(&state.pg)
    .await?;

    let data: Vec<DelegatorIndexRow> = rows
        .iter()
        .map(|r| {
            let total: BigDecimal = r.get("total_bonded");
            DelegatorIndexRow {
                delegator_address: r.get::<String, _>("delegator_address"),
                total_bonded: total.normalized().to_string(),
                delegation_count: r.get::<i64, _>("delegation_count") as u32,
                is_active: r.get("is_active"),
                first_bond_block: r
                    .try_get::<Option<i64>, _>("first_bond_block")
                    .ok()
                    .flatten()
                    .map(|v| v.to_string()),
                last_seen_block: r
                    .try_get::<Option<i64>, _>("last_seen_block")
                    .ok()
                    .flatten()
                    .map(|v| v.to_string()),
            }
        })
        .collect();

    let next_cursor = data.last().map(|row| {
        DelegatorIndexCursor {
            bonded: BigDecimal::from_str(&row.total_bonded).unwrap_or_default(),
            delegator_address: row.delegator_address.clone(),
        }
        .encode()
    });

    Ok(Json(DelegatorIndexResponse {
        data,
        meta: DelegatorIndexMeta {
            chain_id: state.chain_id.to_string(),
            next_cursor,
        },
    }))
}

// ─────────────────────────────────────────────────────────────────────────
// /delegators/{address}/events — D
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
#[schema(
    description = "One Bond/Unbond/Rebond/EarningsClaimed/etc. event involving the delegator."
)]
pub struct DelegatorEventRow {
    pub event_id: String,
    pub event_name: String,
    pub block_number: String,
    pub block_timestamp: DateTime<Utc>,
    pub tx_hash: String,
    pub log_index: i32,
    /// The delegator's address from the event's perspective. Matches the
    /// `address` path param when the delegator initiated the action; differs
    /// when the delegator is the receiving party of a TransferBond.
    pub from_address: Option<String>,
    pub to_address: Option<String>,
    /// Side the delegator was on for this event: `from`, `to`, or `both`.
    pub side: String,
    /// Asset the event amount is denominated in (LPT for stake events,
    /// ETH for fee events). Null for events without a value.
    pub asset: Option<String>,
    /// Normalized amount (decimal string in LPT or ETH).
    pub amount_normalized: Option<String>,
    /// Decoded event-specific payload (e.g. round, additionalAmount).
    pub decoded: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DelegatorEventsMeta {
    pub chain_id: String,
    pub delegator_address: String,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Paginated activity timeline for a delegator.")]
pub struct DelegatorEventsResponse {
    pub data: Vec<DelegatorEventRow>,
    pub meta: DelegatorEventsMeta,
}

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
pub struct DelegatorEventsQuery {
    /// Opaque `(block_number, log_index)` cursor.
    pub cursor: Option<String>,
    /// Maximum number of rows to return (default 50, max 500).
    pub limit: Option<u32>,
}

#[utoipa::path(
    get,
    path = "/delegators/{address}/events",
    tag = "Delegators",
    params(
        ("address" = String, Path, description = "Delegator address."),
        DelegatorEventsQuery
    ),
    responses(
        (status = 200, description = "Paginated activity timeline (Bond/Unbond/Rebond/EarningsClaimed/TransferBond/WithdrawStake/WithdrawFees) for a delegator.", body = DelegatorEventsResponse),
        (status = 400, description = "Invalid address or cursor.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn events_for(
    State(state): State<AppState>,
    Path(address): Path<String>,
    Query(q): Query<DelegatorEventsQuery>,
) -> Result<Json<DelegatorEventsResponse>, ApiError> {
    let address = normalize_addr(&address)?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as i64;
    let cursor = q.cursor.as_deref().map(Cursor::decode).transpose()?;

    let rows = sqlx::query(
        r#"
        SELECT id,
               event_name,
               block_number,
               block_timestamp,
               tx_hash,
               log_index,
               from_address,
               to_address,
               asset,
               amount_normalized,
               raw_event -> 'decoded' AS decoded
          FROM raw_protocol_events
         WHERE chain_id = $1
           AND is_canonical = TRUE
           AND contract_name = 'BondingManager'
           AND event_name IN (
                 'Bond', 'Unbond', 'Rebond', 'TransferBond',
                 'EarningsClaimed', 'WithdrawStake', 'WithdrawFees'
           )
           AND (from_address = $2 OR to_address = $2)
           AND ($3::bigint IS NULL
                OR (block_number, log_index) < ($3, $4))
         ORDER BY block_number DESC, log_index DESC
         LIMIT $5
        "#,
    )
    .bind(state.chain_id)
    .bind(&address)
    .bind(cursor.as_ref().map(|c| c.block_number))
    .bind(cursor.as_ref().map(|c| c.log_index).unwrap_or(0))
    .bind(limit)
    .fetch_all(&state.pg)
    .await?;

    let data: Vec<DelegatorEventRow> = rows
        .iter()
        .map(|r| {
            let from_address: Option<String> = r.try_get("from_address").ok();
            let to_address: Option<String> = r.try_get("to_address").ok();
            let from_match = from_address.as_deref() == Some(address.as_str());
            let to_match = to_address.as_deref() == Some(address.as_str());
            let side = match (from_match, to_match) {
                (true, true) => "both",
                (true, false) => "from",
                (false, true) => "to",
                (false, false) => "unknown",
            }
            .to_string();
            DelegatorEventRow {
                event_id: r.get::<i64, _>("id").to_string(),
                event_name: r.get("event_name"),
                block_number: r.get::<i64, _>("block_number").to_string(),
                block_timestamp: r.get("block_timestamp"),
                tx_hash: r.get("tx_hash"),
                log_index: r.get("log_index"),
                from_address,
                to_address,
                side,
                asset: r.try_get("asset").ok(),
                amount_normalized: r
                    .try_get::<Option<BigDecimal>, _>("amount_normalized")
                    .ok()
                    .flatten()
                    .map(|v| v.normalized().to_string()),
                decoded: r.try_get("decoded").ok(),
            }
        })
        .collect();

    let next_cursor = data.last().map(|row| {
        Cursor {
            block_number: row.block_number.parse().unwrap_or(0),
            log_index: row.log_index,
        }
        .encode()
    });

    Ok(Json(DelegatorEventsResponse {
        data,
        meta: DelegatorEventsMeta {
            chain_id: state.chain_id.to_string(),
            delegator_address: address,
            next_cursor,
        },
    }))
}
