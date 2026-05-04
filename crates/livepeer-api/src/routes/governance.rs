//! Governance convenience endpoint. SPEC §14.3.7.
//! Joins `ProposalCreated` + `ProposalExecuted` + per-proposal `VoteCast` tallies.

use crate::{error::ApiError, state::AppState};
use axum::{extract::{Path, Query, State}, Json};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::str::FromStr;
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Deserialize, IntoParams, ToSchema)]
#[schema(description = "Query parameters for the convenience governance endpoints.")]
pub struct ProposalsQuery {
    /// Filter by lifecycle status: `executed`, `not_executed`, `active`, or `all`.
    pub status: Option<String>,
    /// Maximum number of proposals to return.
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Convenience view of a Governor proposal assembled from raw protocol events.")]
pub struct ProposalRow {
    pub proposal_id: String,
    pub proposer: Option<String>,
    pub vote_start: Option<String>,
    pub vote_end: Option<String>,
    pub description: Option<String>,
    pub created_block: String,
    pub created_at: DateTime<Utc>,
    pub created_tx_hash: String,
    pub executed: bool,
    pub executed_block: Option<String>,
    pub executed_at: Option<DateTime<Utc>>,
    pub vote_tally: VoteTally,
}

#[derive(Debug, Default, Serialize, ToSchema)]
#[schema(description = "Vote weights derived from VoteCast events for a single proposal.")]
pub struct VoteTally {
    /// Solidity `support` enum — 0=Against, 1=For, 2=Abstain.
    pub against_weight: String,
    pub for_weight: String,
    pub abstain_weight: String,
    pub vote_count: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Collection response for governance proposals.")]
pub struct ProposalListResponse {
    pub data: Vec<ProposalRow>,
}

#[utoipa::path(
    get,
    path = "/governance/proposals",
    tag = "Governance",
    params(ProposalsQuery),
    responses(
        (status = 200, description = "Convenience governance view joining proposal creation, execution, and vote tallies.", body = ProposalListResponse),
        (status = 400, description = "Invalid status filter.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn list(
    State(state): State<AppState>,
    Query(q): Query<ProposalsQuery>,
) -> Result<Json<ProposalListResponse>, ApiError> {
    let limit = q.limit.unwrap_or(100).min(1000) as i64;
    // Pull all ProposalCreated rows and join executions.
    let rows = sqlx::query(
        r#"SELECT pc.id,
                  pc.block_number, pc.block_timestamp, pc.tx_hash,
                  pc.raw_event -> 'decoded' ->> 'proposalId'  AS proposal_id,
                  pc.raw_event -> 'decoded' ->> 'proposer'    AS proposer,
                  pc.raw_event -> 'decoded' ->> 'voteStart'   AS vote_start,
                  pc.raw_event -> 'decoded' ->> 'voteEnd'     AS vote_end,
                  pc.raw_event -> 'decoded' ->> 'description' AS description,
                  pe.block_number AS executed_block,
                  pe.block_timestamp AS executed_at
             FROM raw_protocol_events pc
             LEFT JOIN raw_protocol_events pe
               ON pe.chain_id = pc.chain_id
              AND pe.event_name = 'ProposalExecuted'
              AND pe.is_canonical = TRUE
              AND pe.raw_event -> 'decoded' ->> 'proposalId'
                = pc.raw_event -> 'decoded' ->> 'proposalId'
            WHERE pc.chain_id = $1
              AND pc.event_name = 'ProposalCreated'
              AND pc.is_canonical = TRUE
            ORDER BY pc.block_number DESC, pc.log_index DESC
            LIMIT $2"#,
    )
    .bind(state.chain_id)
    .bind(limit)
    .fetch_all(&state.pg)
    .await?;

    let mut proposals: Vec<ProposalRow> = Vec::with_capacity(rows.len());
    for r in &rows {
        let proposal_id_opt: Option<String> = r.try_get("proposal_id").ok();
        let Some(proposal_id) = proposal_id_opt.filter(|s| !s.is_empty()) else {
            continue;
        };

        // Filter by lifecycle status (basic): executed | not_executed | all.
        let executed_block: Option<i64> = r.try_get("executed_block").ok();
        let executed = executed_block.is_some();
        if let Some(filter) = q.status.as_deref() {
            match filter {
                "executed" => if !executed { continue; }
                "not_executed" | "active" => if executed { continue; }
                "all" => {}
                other => return Err(ApiError::bad_request(format!(
                    "invalid status {other:?}; use executed | not_executed | active | all"
                ))),
            }
        }

        let executed_at: Option<DateTime<Utc>> = r.try_get("executed_at").ok();
        let tally = vote_tally_for(&state, &proposal_id).await?;

        let created_block: i64 = r.get("block_number");
        let created_at: DateTime<Utc> = r.get("block_timestamp");
        let created_tx_hash: String = r.get("tx_hash");
        let proposer: Option<String> = r.try_get("proposer").ok();
        let vote_start: Option<String> = r.try_get("vote_start").ok();
        let vote_end: Option<String> = r.try_get("vote_end").ok();
        let description: Option<String> = r.try_get("description").ok();

        proposals.push(ProposalRow {
            proposal_id,
            proposer,
            vote_start,
            vote_end,
            description,
            created_block: created_block.to_string(),
            created_at,
            created_tx_hash,
            executed,
            executed_block: executed_block.map(|n| n.to_string()),
            executed_at,
            vote_tally: tally,
        });
    }

    Ok(Json(ProposalListResponse { data: proposals }))
}

#[utoipa::path(
    get,
    path = "/governance/proposals/{proposal_id}",
    tag = "Governance",
    params(
        ("proposal_id" = String, Path, description = "Governor proposal identifier.")
    ),
    responses(
        (status = 200, description = "Single governance proposal with execution state and vote tallies.", body = ProposalRow),
        (status = 404, description = "No proposal exists for the requested identifier.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn get_one(
    State(state): State<AppState>,
    Path(proposal_id): Path<String>,
) -> Result<Json<ProposalRow>, ApiError> {
    // Re-use list machinery by selecting one. Cheap because each step queries a
    // small set, and the data volume of governance events is sparse.
    let row = sqlx::query(
        r#"SELECT pc.block_number, pc.block_timestamp, pc.tx_hash,
                  pc.raw_event -> 'decoded' ->> 'proposalId'  AS proposal_id,
                  pc.raw_event -> 'decoded' ->> 'proposer'    AS proposer,
                  pc.raw_event -> 'decoded' ->> 'voteStart'   AS vote_start,
                  pc.raw_event -> 'decoded' ->> 'voteEnd'     AS vote_end,
                  pc.raw_event -> 'decoded' ->> 'description' AS description,
                  pe.block_number AS executed_block,
                  pe.block_timestamp AS executed_at
             FROM raw_protocol_events pc
             LEFT JOIN raw_protocol_events pe
               ON pe.chain_id = pc.chain_id
              AND pe.event_name = 'ProposalExecuted'
              AND pe.is_canonical = TRUE
              AND pe.raw_event -> 'decoded' ->> 'proposalId'
                = pc.raw_event -> 'decoded' ->> 'proposalId'
            WHERE pc.chain_id = $1
              AND pc.event_name = 'ProposalCreated'
              AND pc.is_canonical = TRUE
              AND pc.raw_event -> 'decoded' ->> 'proposalId' = $2"#,
    )
    .bind(state.chain_id)
    .bind(&proposal_id)
    .fetch_optional(&state.pg)
    .await?;
    let Some(r) = row else {
        return Err(ApiError::not_found(format!("proposal {proposal_id}")));
    };
    let executed_block: Option<i64> = r.try_get("executed_block").ok();
    let tally = vote_tally_for(&state, &proposal_id).await?;
    Ok(Json(ProposalRow {
        proposal_id: proposal_id.clone(),
        proposer: r.try_get("proposer").ok(),
        vote_start: r.try_get("vote_start").ok(),
        vote_end: r.try_get("vote_end").ok(),
        description: r.try_get("description").ok(),
        created_block: r.get::<i64, _>("block_number").to_string(),
        created_at: r.get("block_timestamp"),
        created_tx_hash: r.get("tx_hash"),
        executed: executed_block.is_some(),
        executed_block: executed_block.map(|n| n.to_string()),
        executed_at: r.try_get("executed_at").ok(),
        vote_tally: tally,
    }))
}

async fn vote_tally_for(state: &AppState, proposal_id: &str) -> Result<VoteTally, ApiError> {
    let rows = sqlx::query(
        r#"SELECT raw_event -> 'decoded' ->> 'support' AS support,
                  raw_event -> 'decoded' ->> 'weight'  AS weight
             FROM raw_protocol_events
            WHERE chain_id = $1
              AND event_name = 'VoteCast'
              AND is_canonical = TRUE
              AND raw_event -> 'decoded' ->> 'proposalId' = $2"#,
    )
    .bind(state.chain_id)
    .bind(proposal_id)
    .fetch_all(&state.pg)
    .await?;

    let zero = BigDecimal::from(0u64);
    let mut against = zero.clone();
    let mut votes_for = zero.clone();
    let mut abstain = zero.clone();
    let mut count = 0u64;
    for r in &rows {
        count += 1;
        let support: Option<String> = r.try_get("support").ok();
        let weight: Option<String> = r.try_get("weight").ok();
        let weight_bd = weight
            .as_deref()
            .and_then(|s| BigDecimal::from_str(s).ok())
            .unwrap_or_else(|| zero.clone());
        match support.as_deref() {
            Some("0") => against += &weight_bd,
            Some("1") => votes_for += &weight_bd,
            Some("2") => abstain += &weight_bd,
            _ => {}
        }
    }
    Ok(VoteTally {
        against_weight: against.to_string(),
        for_weight: votes_for.to_string(),
        abstain_weight: abstain.to_string(),
        vote_count: count.to_string(),
    })
}
