//! Network-level endpoints (TD-027).
//!
//! - `GET /network/stats` — single dashboard payload (TVL, active orchs,
//!   24h payouts/rewards, distinct gateways, recent gas burn).
//! - `GET /rounds/{round_id}` — single-round summary using the per-round
//!   snapshots in `orch_stake_by_round` plus the daily rollups bucketed
//!   by the round's date.

use crate::{cursor::Cursor, error::ApiError, state::AppState};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use utoipa::{IntoParams, ToSchema};

const DEFAULT_ROUNDS_LIMIT: u32 = 50;
const MAX_ROUNDS_LIMIT: u32 = 500;
const DEFAULT_ROUND_EVENTS_LIMIT: u32 = 100;
const MAX_ROUND_EVENTS_LIMIT: u32 = 1000;

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
pub async fn stats(State(state): State<AppState>) -> Result<Json<NetworkStatsResponse>, ApiError> {
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
#[schema(description = "Snapshot of the round immediately before this one for delta display.")]
pub struct PrevRoundContext {
    pub round: String,
    pub active_orchestrators: u32,
    pub total_lpt_staked: String,
    pub payouts_usd_on_day: String,
    pub rewards_usd_on_day: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(
    description = "Single-round summary built from per-round snapshots + daily rollups bucketed by round date."
)]
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
    /// 30-round trailing average of `payouts_usd_on_day`. None if there are
    /// fewer than 30 prior rounds with rollup data.
    pub payouts_usd_30round_avg: Option<String>,
    /// 30-round trailing average of `rewards_usd_on_day`.
    pub rewards_usd_30round_avg: Option<String>,
    /// Snapshot of round-1 for FE delta display. None if this is the
    /// earliest round in `orch_stake_by_round`.
    pub prev_round: Option<PrevRoundContext>,
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
        return Err(ApiError::not_found(
            "round not found in orch_stake_by_round",
        ));
    }

    let block_number: i64 = rows[0].get("block_number");
    let block_timestamp: DateTime<Utc> = rows[0].get("block_timestamp");
    let active_count = rows
        .iter()
        .filter(|r| r.get::<bool, _>("is_active"))
        .count() as u32;
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

    // 30-round trailing average. Pull the daily rollups for the 30 round
    // dates immediately preceding this one (inclusive of the current round
    // day to mirror "last 30 rounds including this one"). If we have fewer
    // than 30 rows, return None.
    let avg_row = sqlx::query(
        r#"
        WITH win AS (
            SELECT DISTINCT block_timestamp::date AS day
              FROM orch_stake_by_round
             WHERE chain_id = $1 AND round <= $2
             ORDER BY day DESC
             LIMIT 30
        )
        SELECT
          COUNT(*)::int AS days,
          COALESCE(AVG((SELECT SUM(sum_commission_usd)
                          FROM orch_payouts_daily
                         WHERE chain_id = $1 AND day_utc = win.day)), 0) AS payouts_avg,
          COALESCE(AVG((SELECT SUM(sum_orch_tokens_usd)
                          FROM orch_rewards_daily
                         WHERE chain_id = $1 AND day_utc = win.day)), 0) AS rewards_avg
          FROM win
        "#,
    )
    .bind(state.chain_id)
    .bind(round_id)
    .fetch_one(&state.pg)
    .await?;

    let days_in_window: i32 = avg_row.get("days");
    let (payouts_avg, rewards_avg) = if days_in_window >= 30 {
        let p: BigDecimal = avg_row.get("payouts_avg");
        let r: BigDecimal = avg_row.get("rewards_avg");
        (
            Some(p.normalized().to_string()),
            Some(r.normalized().to_string()),
        )
    } else {
        (None, None)
    };

    // Prev-round snapshot for delta display (E). `MAX(round) WHERE round < $2`
    // handles non-contiguous rounds gracefully.
    let prev_round = match sqlx::query(
        r#"
        WITH prev AS (
            SELECT MAX(round) AS r FROM orch_stake_by_round
             WHERE chain_id = $1 AND round < $2
        ),
        agg AS (
            SELECT
              prev.r AS round,
              SUM(s.total_stake) AS total_stake,
              COUNT(*) FILTER (WHERE s.is_active) AS active_orchs,
              MAX(s.block_timestamp) AS bt
              FROM prev
              LEFT JOIN orch_stake_by_round s
                ON s.chain_id = $1 AND s.round = prev.r
             GROUP BY prev.r
        )
        SELECT a.round, a.active_orchs, a.total_stake, a.bt,
          COALESCE((SELECT SUM(sum_commission_usd) FROM orch_payouts_daily
                     WHERE chain_id = $1 AND day_utc = (a.bt::timestamptz)::date), 0) AS payouts,
          COALESCE((SELECT SUM(sum_orch_tokens_usd) FROM orch_rewards_daily
                     WHERE chain_id = $1 AND day_utc = (a.bt::timestamptz)::date), 0) AS rewards
          FROM agg a
        "#,
    )
    .bind(state.chain_id)
    .bind(round_id)
    .fetch_optional(&state.pg)
    .await?
    {
        Some(r) => match r.try_get::<Option<i64>, _>("round").ok().flatten() {
            Some(round) => {
                let total_stake: BigDecimal = r.get("total_stake");
                let active_orchs: i64 = r.get("active_orchs");
                let payouts: BigDecimal = r.get("payouts");
                let rewards: BigDecimal = r.get("rewards");
                Some(PrevRoundContext {
                    round: round.to_string(),
                    active_orchestrators: active_orchs as u32,
                    total_lpt_staked: total_stake.normalized().to_string(),
                    payouts_usd_on_day: payouts.normalized().to_string(),
                    rewards_usd_on_day: rewards.normalized().to_string(),
                })
            }
            None => None,
        },
        None => None,
    };

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
        payouts_usd_30round_avg: payouts_avg,
        rewards_usd_30round_avg: rewards_avg,
        prev_round,
    }))
}

// ─────────────────────────────────────────────────────────────────────────
// /rounds — index (A)
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "One row of the rounds index, sorted by round DESC.")]
pub struct RoundIndexRow {
    pub round: String,
    pub started_block: String,
    pub started_at: DateTime<Utc>,
    pub active_orchestrators: u32,
    pub total_lpt_staked: String,
    pub payouts_usd_on_day: String,
    pub rewards_usd_on_day: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RoundsIndexMeta {
    pub chain_id: String,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Paginated rounds index sorted by round DESC.")]
pub struct RoundsIndexResponse {
    pub data: Vec<RoundIndexRow>,
    pub meta: RoundsIndexMeta,
}

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
pub struct RoundsIndexQuery {
    /// Opaque cursor — encodes the round number to resume below.
    pub cursor: Option<String>,
    /// Max rows to return (default 50, max 500).
    pub limit: Option<u32>,
}

fn encode_round_cursor(round: i64) -> String {
    format!("R{round}")
}

fn decode_round_cursor(s: &str) -> Result<i64, ApiError> {
    s.strip_prefix('R')
        .ok_or_else(|| ApiError::bad_request("invalid cursor"))
        .and_then(|n| {
            n.parse::<i64>()
                .map_err(|_| ApiError::bad_request("invalid cursor numeric"))
        })
}

#[utoipa::path(
    get,
    path = "/rounds",
    tag = "Network",
    params(RoundsIndexQuery),
    responses(
        (status = 200, description = "Paginated index of rounds, newest first.", body = RoundsIndexResponse),
        (status = 400, description = "Invalid cursor.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn rounds_index(
    State(state): State<AppState>,
    Query(q): Query<RoundsIndexQuery>,
) -> Result<Json<RoundsIndexResponse>, ApiError> {
    let limit = q
        .limit
        .unwrap_or(DEFAULT_ROUNDS_LIMIT)
        .min(MAX_ROUNDS_LIMIT) as i64;
    let cursor_round = q.cursor.as_deref().map(decode_round_cursor).transpose()?;

    let rows = sqlx::query(
        r#"
        WITH rounds AS (
            SELECT round,
                   MAX(block_number)    AS started_block,
                   MAX(block_timestamp) AS started_at,
                   SUM(total_stake)     AS total_stake,
                   COUNT(*) FILTER (WHERE is_active) AS active_orchs
              FROM orch_stake_by_round
             WHERE chain_id = $1
               AND ($2::bigint IS NULL OR round < $2)
             GROUP BY round
             ORDER BY round DESC
             LIMIT $3
        )
        SELECT r.round, r.started_block, r.started_at, r.total_stake, r.active_orchs,
          COALESCE((SELECT SUM(sum_commission_usd) FROM orch_payouts_daily
                     WHERE chain_id = $1 AND day_utc = (r.started_at::timestamptz)::date), 0) AS payouts,
          COALESCE((SELECT SUM(sum_orch_tokens_usd) FROM orch_rewards_daily
                     WHERE chain_id = $1 AND day_utc = (r.started_at::timestamptz)::date), 0) AS rewards
          FROM rounds r
         ORDER BY r.round DESC
        "#,
    )
    .bind(state.chain_id)
    .bind(cursor_round)
    .bind(limit)
    .fetch_all(&state.pg)
    .await?;

    let data: Vec<RoundIndexRow> = rows
        .iter()
        .map(|r| {
            let stake: BigDecimal = r.get("total_stake");
            let payouts: BigDecimal = r.get("payouts");
            let rewards: BigDecimal = r.get("rewards");
            RoundIndexRow {
                round: r.get::<i64, _>("round").to_string(),
                started_block: r.get::<i64, _>("started_block").to_string(),
                started_at: r.get("started_at"),
                active_orchestrators: r.get::<i64, _>("active_orchs") as u32,
                total_lpt_staked: stake.normalized().to_string(),
                payouts_usd_on_day: payouts.normalized().to_string(),
                rewards_usd_on_day: rewards.normalized().to_string(),
            }
        })
        .collect();

    let next_cursor = data
        .last()
        .map(|row| encode_round_cursor(row.round.parse().unwrap_or(0)));

    Ok(Json(RoundsIndexResponse {
        data,
        meta: RoundsIndexMeta {
            chain_id: state.chain_id.to_string(),
            next_cursor,
        },
    }))
}

// ─────────────────────────────────────────────────────────────────────────
// /rounds/{id}/events — per-round activity (C)
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "One event that fired during the round window.")]
pub struct RoundEventRow {
    pub event_id: String,
    pub event_name: String,
    pub contract_name: String,
    pub block_number: String,
    pub block_timestamp: DateTime<Utc>,
    pub tx_hash: String,
    pub log_index: i32,
    pub from_address: Option<String>,
    pub to_address: Option<String>,
    pub asset: Option<String>,
    pub amount_normalized: Option<String>,
    pub decoded: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RoundEventsMeta {
    pub chain_id: String,
    pub round: String,
    pub from_block: String,
    pub to_block: Option<String>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(
    description = "Paginated events that fired between this round's NewRound block and the next."
)]
pub struct RoundEventsResponse {
    pub data: Vec<RoundEventRow>,
    pub meta: RoundEventsMeta,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Aggregate event counts by event_name within the round window.")]
pub struct RoundEventCountsResponse {
    pub chain_id: String,
    pub round: String,
    pub from_block: String,
    pub to_block: Option<String>,
    pub counts: std::collections::BTreeMap<String, u64>,
    pub total: u64,
}

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
pub struct RoundEventsQuery {
    /// Comma-separated event-name filter (default: all monetary + lifecycle events).
    pub kinds: Option<String>,
    /// Opaque `(block_number, log_index)` cursor.
    pub cursor: Option<String>,
    /// Max rows (default 100, max 1000).
    pub limit: Option<u32>,
}

const DEFAULT_ROUND_EVENT_KINDS: &[&str] = &[
    "Bond",
    "Unbond",
    "Rebond",
    "TransferBond",
    "EarningsClaimed",
    "Reward",
    "WinningTicketRedeemed",
    "WinningTicketTransfer",
    "TranscoderUpdate",
    "TranscoderActivated",
    "TranscoderDeactivated",
];

fn parse_kinds(raw: Option<&str>) -> Vec<String> {
    match raw {
        Some(s) => s
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect(),
        None => DEFAULT_ROUND_EVENT_KINDS
            .iter()
            .map(|s| s.to_string())
            .collect(),
    }
}

/// Resolve the (from_block, to_block) range for a round. `to_block` is the
/// next round's started block minus 1; None if there's no next round yet.
async fn resolve_round_range(
    pool: &sqlx::PgPool,
    chain_id: i64,
    round_id: i64,
) -> Result<(i64, Option<i64>), ApiError> {
    let row = sqlx::query(
        r#"
        SELECT
          (SELECT MIN(block_number) FROM orch_stake_by_round
            WHERE chain_id = $1 AND round = $2) AS from_block,
          (SELECT MIN(block_number) FROM orch_stake_by_round
            WHERE chain_id = $1 AND round > $2) AS next_round_block
        "#,
    )
    .bind(chain_id)
    .bind(round_id)
    .fetch_one(pool)
    .await?;

    let from_block: Option<i64> = row.try_get("from_block").ok().flatten();
    let next_block: Option<i64> = row.try_get("next_round_block").ok().flatten();
    let from_block =
        from_block.ok_or_else(|| ApiError::not_found("round not found in orch_stake_by_round"))?;
    let to_block = next_block.map(|b| b - 1);
    Ok((from_block, to_block))
}

#[utoipa::path(
    get,
    path = "/rounds/{round_id}/events",
    tag = "Network",
    params(
        ("round_id" = i64, Path, description = "Round id."),
        RoundEventsQuery
    ),
    responses(
        (status = 200, description = "Paginated events that fired during the round window.", body = RoundEventsResponse),
        (status = 400, description = "Invalid cursor.", body = crate::error::ErrorEnvelope),
        (status = 404, description = "Round not found.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn round_events(
    State(state): State<AppState>,
    Path(round_id): Path<i64>,
    Query(q): Query<RoundEventsQuery>,
) -> Result<Json<RoundEventsResponse>, ApiError> {
    let limit = q
        .limit
        .unwrap_or(DEFAULT_ROUND_EVENTS_LIMIT)
        .min(MAX_ROUND_EVENTS_LIMIT) as i64;
    let cursor = q.cursor.as_deref().map(Cursor::decode).transpose()?;
    let kinds = parse_kinds(q.kinds.as_deref());

    let (from_block, to_block) = resolve_round_range(&state.pg, state.chain_id, round_id).await?;

    let rows = sqlx::query(
        r#"
        SELECT id, event_name, contract_name, block_number, block_timestamp,
               tx_hash, log_index, from_address, to_address,
               asset, amount_normalized, raw_event -> 'decoded' AS decoded
          FROM raw_protocol_events
         WHERE chain_id = $1
           AND is_canonical = TRUE
           AND block_number >= $2
           AND ($3::bigint IS NULL OR block_number <= $3)
           AND event_name = ANY($4::text[])
           AND ($5::bigint IS NULL
                OR (block_number, log_index) < ($5, $6))
         ORDER BY block_number DESC, log_index DESC
         LIMIT $7
        "#,
    )
    .bind(state.chain_id)
    .bind(from_block)
    .bind(to_block)
    .bind(&kinds)
    .bind(cursor.as_ref().map(|c| c.block_number))
    .bind(cursor.as_ref().map(|c| c.log_index).unwrap_or(0))
    .bind(limit)
    .fetch_all(&state.pg)
    .await?;

    let data: Vec<RoundEventRow> = rows
        .iter()
        .map(|r| RoundEventRow {
            event_id: r.get::<i64, _>("id").to_string(),
            event_name: r.get("event_name"),
            contract_name: r.get("contract_name"),
            block_number: r.get::<i64, _>("block_number").to_string(),
            block_timestamp: r.get("block_timestamp"),
            tx_hash: r.get("tx_hash"),
            log_index: r.get("log_index"),
            from_address: r.try_get("from_address").ok(),
            to_address: r.try_get("to_address").ok(),
            asset: r.try_get("asset").ok(),
            amount_normalized: r
                .try_get::<Option<BigDecimal>, _>("amount_normalized")
                .ok()
                .flatten()
                .map(|v| v.normalized().to_string()),
            decoded: r.try_get("decoded").ok(),
        })
        .collect();

    let next_cursor = data.last().map(|row| {
        Cursor {
            block_number: row.block_number.parse().unwrap_or(0),
            log_index: row.log_index,
        }
        .encode()
    });

    Ok(Json(RoundEventsResponse {
        data,
        meta: RoundEventsMeta {
            chain_id: state.chain_id.to_string(),
            round: round_id.to_string(),
            from_block: from_block.to_string(),
            to_block: to_block.map(|b| b.to_string()),
            next_cursor,
        },
    }))
}

#[utoipa::path(
    get,
    path = "/rounds/{round_id}/event-counts",
    tag = "Network",
    params(("round_id" = i64, Path, description = "Round id.")),
    responses(
        (status = 200, description = "Aggregate event counts within the round window.", body = RoundEventCountsResponse),
        (status = 404, description = "Round not found.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn round_event_counts(
    State(state): State<AppState>,
    Path(round_id): Path<i64>,
) -> Result<Json<RoundEventCountsResponse>, ApiError> {
    let (from_block, to_block) = resolve_round_range(&state.pg, state.chain_id, round_id).await?;

    let rows = sqlx::query(
        r#"
        SELECT event_name, COUNT(*) AS n
          FROM raw_protocol_events
         WHERE chain_id = $1
           AND is_canonical = TRUE
           AND block_number >= $2
           AND ($3::bigint IS NULL OR block_number <= $3)
         GROUP BY event_name
        "#,
    )
    .bind(state.chain_id)
    .bind(from_block)
    .bind(to_block)
    .fetch_all(&state.pg)
    .await?;

    let mut counts = std::collections::BTreeMap::new();
    let mut total: u64 = 0;
    for r in &rows {
        let n: i64 = r.get("n");
        counts.insert(r.get::<String, _>("event_name"), n as u64);
        total += n as u64;
    }

    Ok(Json(RoundEventCountsResponse {
        chain_id: state.chain_id.to_string(),
        round: round_id.to_string(),
        from_block: from_block.to_string(),
        to_block: to_block.map(|b| b.to_string()),
        counts,
        total,
    }))
}
