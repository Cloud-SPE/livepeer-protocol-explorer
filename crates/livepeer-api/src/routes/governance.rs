//! Governance convenience endpoint. SPEC §14.3.7.
//! Joins `ProposalCreated` + `ProposalExecuted` + per-proposal `VoteCast` tallies.

use crate::cursor::Cursor;
use crate::{error::ApiError, state::AppState};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
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
#[schema(
    description = "Convenience view of a Governor proposal assembled from raw protocol events."
)]
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

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
#[schema(description = "Query parameters for governance vote history.")]
pub struct VotesQuery {
    pub proposal_id: Option<String>,
    pub voter: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "One canonical Governor vote event.")]
pub struct VoteRow {
    pub event_id: String,
    pub event_name: String,
    pub proposal_id: String,
    pub voter: String,
    pub support: String,
    pub weight: String,
    pub reason: Option<String>,
    pub block_number: String,
    pub block_timestamp: DateTime<Utc>,
    pub tx_hash: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Coverage metadata for the governor vote stream.")]
pub struct VotesCoverage {
    pub domain: String,
    pub backfill_complete: bool,
    pub last_processed_block: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Paginated governance vote history response.")]
pub struct VoteListResponse {
    pub data: Vec<VoteRow>,
    pub next_cursor: Option<String>,
    pub meta: VotesCoverage,
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
    let status = parse_status_filter(q.status.as_deref())?;
    let rows = sqlx::query(
        r#"WITH proposal_rows AS (
               SELECT pc.block_number,
                      pc.block_timestamp,
                      pc.tx_hash,
                      pc.log_index,
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
                  AND COALESCE(pc.raw_event -> 'decoded' ->> 'proposalId', '') <> ''
                  AND (
                        $2 = 'all'
                        OR ($2 = 'executed' AND pe.block_number IS NOT NULL)
                        OR ($2 IN ('not_executed', 'active') AND pe.block_number IS NULL)
                  )
                ORDER BY pc.block_number DESC, pc.log_index DESC
                LIMIT $3
           ),
           vote_tallies AS (
               SELECT rv.raw_event -> 'decoded' ->> 'proposalId' AS proposal_id,
                      COALESCE(SUM(CASE WHEN rv.raw_event -> 'decoded' ->> 'support' = '0'
                                        THEN (rv.raw_event -> 'decoded' ->> 'weight')::numeric
                                        ELSE 0 END), 0)::text AS against_weight,
                      COALESCE(SUM(CASE WHEN rv.raw_event -> 'decoded' ->> 'support' = '1'
                                        THEN (rv.raw_event -> 'decoded' ->> 'weight')::numeric
                                        ELSE 0 END), 0)::text AS for_weight,
                      COALESCE(SUM(CASE WHEN rv.raw_event -> 'decoded' ->> 'support' = '2'
                                        THEN (rv.raw_event -> 'decoded' ->> 'weight')::numeric
                                        ELSE 0 END), 0)::text AS abstain_weight,
                      COUNT(*)::bigint::text AS vote_count
                 FROM raw_protocol_events rv
                 JOIN proposal_rows p
                   ON p.proposal_id = rv.raw_event -> 'decoded' ->> 'proposalId'
                WHERE rv.chain_id = $1
                  AND rv.event_name = 'VoteCast'
                  AND rv.is_canonical = TRUE
                GROUP BY rv.raw_event -> 'decoded' ->> 'proposalId'
           )
           SELECT p.block_number, p.block_timestamp, p.tx_hash, p.proposal_id,
                  p.proposer, p.vote_start, p.vote_end, p.description,
                  p.executed_block, p.executed_at,
                  COALESCE(vt.against_weight, '0') AS against_weight,
                  COALESCE(vt.for_weight, '0') AS for_weight,
                  COALESCE(vt.abstain_weight, '0') AS abstain_weight,
                  COALESCE(vt.vote_count, '0') AS vote_count
             FROM proposal_rows p
             LEFT JOIN vote_tallies vt USING (proposal_id)
            ORDER BY p.block_number DESC, p.log_index DESC"#,
    )
    .bind(state.chain_id)
    .bind(status)
    .bind(limit)
    .fetch_all(&state.pg)
    .await?;

    let mut proposals: Vec<ProposalRow> = Vec::with_capacity(rows.len());
    for r in &rows {
        let proposal_id_opt: Option<String> = r.try_get("proposal_id").ok();
        let Some(proposal_id) = proposal_id_opt.filter(|s| !s.is_empty()) else {
            continue;
        };

        let executed_block: Option<i64> = r.try_get("executed_block").ok();
        let executed = executed_block.is_some();
        let executed_at: Option<DateTime<Utc>> = r.try_get("executed_at").ok();

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
            vote_tally: VoteTally {
                against_weight: r.get("against_weight"),
                for_weight: r.get("for_weight"),
                abstain_weight: r.get("abstain_weight"),
                vote_count: r.get("vote_count"),
            },
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
    let row = sqlx::query(
        r#"WITH proposal_row AS (
               SELECT pc.block_number,
                      pc.block_timestamp,
                      pc.tx_hash,
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
                  AND pc.raw_event -> 'decoded' ->> 'proposalId' = $2
           ),
           vote_tally AS (
               SELECT COALESCE(SUM(CASE WHEN rv.raw_event -> 'decoded' ->> 'support' = '0'
                                        THEN (rv.raw_event -> 'decoded' ->> 'weight')::numeric
                                        ELSE 0 END), 0)::text AS against_weight,
                      COALESCE(SUM(CASE WHEN rv.raw_event -> 'decoded' ->> 'support' = '1'
                                        THEN (rv.raw_event -> 'decoded' ->> 'weight')::numeric
                                        ELSE 0 END), 0)::text AS for_weight,
                      COALESCE(SUM(CASE WHEN rv.raw_event -> 'decoded' ->> 'support' = '2'
                                        THEN (rv.raw_event -> 'decoded' ->> 'weight')::numeric
                                        ELSE 0 END), 0)::text AS abstain_weight,
                      COUNT(*)::bigint::text AS vote_count
                 FROM raw_protocol_events rv
                WHERE rv.chain_id = $1
                  AND rv.event_name = 'VoteCast'
                  AND rv.is_canonical = TRUE
                  AND rv.raw_event -> 'decoded' ->> 'proposalId' = $2
           )
           SELECT p.block_number, p.block_timestamp, p.tx_hash, p.proposal_id,
                  p.proposer, p.vote_start, p.vote_end, p.description,
                  p.executed_block, p.executed_at,
                  vt.against_weight, vt.for_weight, vt.abstain_weight, vt.vote_count
             FROM proposal_row p
             CROSS JOIN vote_tally vt"#,
    )
    .bind(state.chain_id)
    .bind(&proposal_id)
    .fetch_optional(&state.pg)
    .await?;
    let Some(r) = row else {
        return Err(ApiError::not_found(format!("proposal {proposal_id}")));
    };
    let executed_block: Option<i64> = r.try_get("executed_block").ok();
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
        vote_tally: VoteTally {
            against_weight: r.get("against_weight"),
            for_weight: r.get("for_weight"),
            abstain_weight: r.get("abstain_weight"),
            vote_count: r.get("vote_count"),
        },
    }))
}

#[utoipa::path(
    get,
    path = "/governance/votes",
    tag = "Governance",
    params(VotesQuery),
    responses(
        (status = 200, description = "Canonical VoteCast rows with optional proposal/voter filtering.", body = VoteListResponse),
        (status = 400, description = "Invalid filter or cursor.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn votes(
    State(state): State<AppState>,
    Query(q): Query<VotesQuery>,
) -> Result<Json<VoteListResponse>, ApiError> {
    let limit = q.limit.unwrap_or(100).min(1000) as i64;
    let cursor = q.cursor.as_deref().map(Cursor::decode).transpose()?;
    let voter = q.voter.as_deref().map(normalize_addr).transpose()?;
    let cursor_clause = match cursor {
        Some(_) => "AND (e.block_number, e.log_index) < ($4, $5)",
        None => "",
    };
    let sql = format!(
        r#"SELECT e.id AS event_id,
                  e.event_name,
                  e.block_number,
                  e.log_index,
                  e.block_timestamp,
                  e.tx_hash,
                  e.raw_event -> 'decoded' ->> 'proposalId' AS proposal_id,
                  e.raw_event -> 'decoded' ->> 'voter' AS voter,
                  e.raw_event -> 'decoded' ->> 'support' AS support,
                  e.raw_event -> 'decoded' ->> 'weight' AS weight,
                  e.raw_event -> 'decoded' ->> 'reason' AS reason
             FROM raw_protocol_events e
            WHERE e.chain_id = $1
              AND e.is_canonical = TRUE
              AND e.event_name IN ('VoteCast', 'VoteCastWithParams')
              AND ($2::text IS NULL OR e.raw_event -> 'decoded' ->> 'proposalId' = $2)
              AND ($3::text IS NULL OR e.from_address = $3)
              {cursor_clause}
         ORDER BY e.block_number DESC, e.log_index DESC
            LIMIT $6"#,
    );
    let mut query = sqlx::query(&sql)
        .bind(state.chain_id)
        .bind(q.proposal_id.as_deref())
        .bind(voter.as_deref());
    if let Some(cursor) = cursor {
        query = query.bind(cursor.block_number).bind(cursor.log_index);
    } else {
        query = query.bind(0_i64).bind(0_i32);
    }
    query = query.bind(limit + 1);
    let rows = query.fetch_all(&state.pg).await?;

    let has_more = rows.len() as i64 > limit;
    let page_rows = rows.into_iter().take(limit as usize).collect::<Vec<_>>();
    let mut data = Vec::with_capacity(page_rows.len());
    let mut next_cursor = None;
    for row in page_rows {
        if has_more {
            next_cursor = Some(
                Cursor {
                    block_number: row.get("block_number"),
                    log_index: row.get("log_index"),
                }
                .encode(),
            );
        }
        data.push(VoteRow {
            event_id: row.get::<i64, _>("event_id").to_string(),
            event_name: row.get("event_name"),
            proposal_id: row.get("proposal_id"),
            voter: row.get("voter"),
            support: row.get("support"),
            weight: row.get("weight"),
            reason: row.try_get("reason").ok(),
            block_number: row.get::<i64, _>("block_number").to_string(),
            block_timestamp: row.get("block_timestamp"),
            tx_hash: row.get("tx_hash"),
        });
    }

    let last_processed_block: Option<i64> = sqlx::query_scalar(
        "SELECT last_processed_block FROM indexer_checkpoints WHERE name = 'indexer_Governor'",
    )
    .fetch_optional(&state.pg)
    .await?;
    let latest_event_block: Option<i64> = sqlx::query_scalar(
        r#"SELECT MAX(block_number)
             FROM raw_protocol_events
            WHERE chain_id = $1
              AND contract_name = 'Governor'
              AND is_canonical = TRUE"#,
    )
    .bind(state.chain_id)
    .fetch_one(&state.pg)
    .await?;

    Ok(Json(VoteListResponse {
        data,
        next_cursor: if has_more { next_cursor } else { None },
        meta: VotesCoverage {
            domain: "governor".to_string(),
            backfill_complete: last_processed_block
                .zip(latest_event_block)
                .map(|(cp, max)| cp >= max)
                .unwrap_or(false),
            last_processed_block: last_processed_block.map(|n| n.to_string()),
        },
    }))
}

fn parse_status_filter(status: Option<&str>) -> Result<&str, ApiError> {
    match status.unwrap_or("all") {
        "executed" | "not_executed" | "active" | "all" => Ok(status.unwrap_or("all")),
        other => Err(ApiError::bad_request(format!(
            "invalid status {other:?}; use executed | not_executed | active | all"
        ))),
    }
}

fn normalize_addr(raw: &str) -> Result<String, ApiError> {
    let lowered = raw.to_lowercase();
    if lowered.len() != 42 || !lowered.starts_with("0x") {
        return Err(ApiError::bad_request("invalid voter address"));
    }
    Ok(lowered)
}
