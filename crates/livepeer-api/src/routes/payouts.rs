use crate::{error::ApiError, state::AppState};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use bigdecimal::BigDecimal;
use chrono::{Datelike, Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::str::FromStr;
use utoipa::{IntoParams, ToSchema};

const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 1_000;

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
#[schema(description = "Query parameters for the orchestrator payout leaderboard.")]
pub struct PayoutLeaderboardQuery {
    pub from: String,
    pub to: String,
    /// `ai`, `transcoding`, or `both`.
    pub job_type: Option<String>,
    /// `commission_usd`, `ticket_count`, or `face_value_usd`.
    pub sort: Option<String>,
    /// Opaque cursor for stable pagination.
    pub cursor: Option<String>,
    /// Maximum number of rows to return.
    pub limit: Option<u32>,
    /// Optional valuation version. Defaults to the server default version.
    pub valuation_version: Option<String>,
}

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
#[schema(description = "Query parameters for payout summary endpoints.")]
pub struct PayoutSummaryQuery {
    /// `ai`, `transcoding`, or `both`.
    pub job_type: Option<String>,
    /// Optional valuation version. Defaults to the server default version.
    pub valuation_version: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(
    description = "One orchestrator payout leaderboard row aggregated over the requested date range."
)]
pub struct PayoutLeaderboardRow {
    pub orchestrator_address: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub ticket_count: String,
    pub sum_face_value_native: String,
    pub sum_face_value_usd: String,
    pub sum_commission_native: String,
    pub sum_commission_usd: String,
    pub sum_delegators_share_native: String,
    pub sum_delegators_share_usd: String,
    pub distinct_gateways: String,
    pub usd_rows_priced: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Metadata for payout leaderboard responses.")]
pub struct PayoutLeaderboardMeta {
    pub chain_id: String,
    pub from: String,
    pub to: String,
    pub valuation_version: String,
    pub job_type: String,
    pub sort: String,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Paginated orchestrator payout leaderboard.")]
pub struct PayoutLeaderboardResponse {
    pub data: Vec<PayoutLeaderboardRow>,
    pub meta: PayoutLeaderboardMeta,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Payout summary row for a requested period.")]
pub struct PayoutSummaryResponse {
    pub period_start: String,
    pub period_end: String,
    pub valuation_version: String,
    pub job_type: String,
    pub ticket_count: String,
    pub sum_face_value_native: String,
    pub sum_face_value_usd: String,
    pub sum_commission_native: String,
    pub sum_commission_usd: String,
    pub sum_delegators_share_native: String,
    pub sum_delegators_share_usd: String,
    pub distinct_gateways: String,
    pub usd_rows_priced: String,
}

#[derive(Debug, Clone, Copy)]
enum JobType {
    Ai,
    Transcoding,
    Both,
}

impl JobType {
    fn parse(value: Option<&str>) -> Result<Self, ApiError> {
        match value.unwrap_or("both") {
            "ai" => Ok(Self::Ai),
            "transcoding" => Ok(Self::Transcoding),
            "both" => Ok(Self::Both),
            other => Err(ApiError::bad_request(format!(
                "invalid job_type {other:?}; use ai | transcoding | both"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Ai => "ai",
            Self::Transcoding => "transcoding",
            Self::Both => "both",
        }
    }

    fn sql_filter(self) -> Option<&'static str> {
        match self {
            Self::Ai => Some("ai"),
            Self::Transcoding => Some("transcoding"),
            Self::Both => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum LeaderboardSort {
    CommissionUsd,
    TicketCount,
    FaceValueUsd,
}

impl LeaderboardSort {
    fn parse(value: Option<&str>) -> Result<Self, ApiError> {
        match value.unwrap_or("commission_usd") {
            "commission_usd" => Ok(Self::CommissionUsd),
            "ticket_count" => Ok(Self::TicketCount),
            "face_value_usd" => Ok(Self::FaceValueUsd),
            other => Err(ApiError::bad_request(format!(
                "invalid sort {other:?}; use commission_usd | ticket_count | face_value_usd"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::CommissionUsd => "commission_usd",
            Self::TicketCount => "ticket_count",
            Self::FaceValueUsd => "face_value_usd",
        }
    }

    fn order_sql(self) -> &'static str {
        match self {
            Self::CommissionUsd => "sum_commission_usd",
            Self::TicketCount => "ticket_count",
            Self::FaceValueUsd => "sum_face_value_usd",
        }
    }
}

#[derive(Debug, Clone)]
struct LeaderboardCursor {
    sort_value: BigDecimal,
    orchestrator_address: String,
}

impl LeaderboardCursor {
    fn encode(&self) -> String {
        format!(
            "P{}|{}",
            self.sort_value.normalized(),
            self.orchestrator_address
        )
    }

    fn decode(raw: &str) -> Result<Self, ApiError> {
        let stripped = raw
            .strip_prefix('P')
            .ok_or_else(|| ApiError::bad_request("invalid cursor"))?;
        let (sort_value, orchestrator_address) = stripped
            .split_once('|')
            .ok_or_else(|| ApiError::bad_request("invalid cursor"))?;
        Ok(Self {
            sort_value: BigDecimal::from_str(sort_value)
                .map_err(|_| ApiError::bad_request("invalid cursor numeric"))?,
            orchestrator_address: normalize_addr(orchestrator_address)?,
        })
    }
}

#[utoipa::path(
    get,
    path = "/payouts/leaderboard",
    tag = "Payouts",
    params(PayoutLeaderboardQuery),
    responses(
        (status = 200, description = "Paginated payout leaderboard aggregated by orchestrator.", body = PayoutLeaderboardResponse),
        (status = 400, description = "Invalid date, job_type, sort, or cursor.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn leaderboard(
    State(state): State<AppState>,
    Query(q): Query<PayoutLeaderboardQuery>,
) -> Result<Json<PayoutLeaderboardResponse>, ApiError> {
    let from = parse_date(&q.from, "from")?;
    let to = parse_date(&q.to, "to")?;
    if to < from {
        return Err(ApiError::bad_request("to must be >= from"));
    }
    let job_type = JobType::parse(q.job_type.as_deref())?;
    let sort = LeaderboardSort::parse(q.sort.as_deref())?;
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as i64;
    let valuation_version = q
        .valuation_version
        .clone()
        .unwrap_or_else(|| state.default_version.clone());
    let cursor = q
        .cursor
        .as_deref()
        .map(LeaderboardCursor::decode)
        .transpose()?;

    // distinct_gateways for the leaderboard is "distinct gateways that paid
    // THIS orch over the date range." The rollup column is per-day, so
    // SUM(distinct_gateways) double-counts gateways that paid the same orch
    // on multiple days. Compute the true distinct count from
    // raw_protocol_events as a correlated sub-query, scoped by orch +
    // window + (optional) broadcaster_kind.
    let sql = format!(
        r#"WITH grouped AS (
               SELECT
                   p.orchestrator_address,
                   COALESCE(o.display_name, e.ens_name) AS display_name,
                   COALESCE(o.avatar_url, e.ens_avatar_url) AS avatar_url,
                   SUM(p.ticket_count)::bigint AS ticket_count,
                   SUM(p.sum_face_value_native) AS sum_face_value_native,
                   SUM(p.sum_face_value_usd) AS sum_face_value_usd,
                   SUM(p.sum_commission_native) AS sum_commission_native,
                   SUM(p.sum_commission_usd) AS sum_commission_usd,
                   SUM(p.sum_delegators_share_native) AS sum_delegators_share_native,
                   SUM(p.sum_delegators_share_usd) AS sum_delegators_share_usd,
                   SUM(p.usd_rows_priced)::bigint AS usd_rows_priced,
                   COALESCE((
                     SELECT COUNT(DISTINCT r.from_address)
                       FROM raw_protocol_events r
                  LEFT JOIN broadcaster_classifications bc
                         ON bc.chain_id = r.chain_id
                        AND bc.address  = r.from_address
                      WHERE r.chain_id          = $1
                        AND r.event_name        = 'WinningTicketRedeemed'
                        AND r.is_canonical      = TRUE
                        AND r.finality          = 'finalized'
                        AND r.to_address        = p.orchestrator_address
                        AND r.from_address     IS NOT NULL
                        AND r.block_timestamp >= $2::timestamptz
                        AND r.block_timestamp <  ($3::date + 1)::timestamptz
                        AND ($5::text IS NULL
                             OR COALESCE(bc.kind, 'transcoding') = $5)
                   ), 0)::bigint AS distinct_gateways
               FROM orch_payouts_daily p
          LEFT JOIN orchestrator_ens e
                 ON e.chain_id = p.chain_id
                AND e.address = p.orchestrator_address
          LEFT JOIN name_avatar_overrides o
                 ON o.chain_id = p.chain_id
                AND o.address = p.orchestrator_address
              WHERE p.chain_id = $1
                AND p.day_utc >= $2
                AND p.day_utc <= $3
                AND p.valuation_version = $4
                AND ($5::text IS NULL OR p.broadcaster_kind = $5)
              GROUP BY
                   p.orchestrator_address,
                   COALESCE(o.display_name, e.ens_name),
                   COALESCE(o.avatar_url, e.ens_avatar_url)
           )
           SELECT *
             FROM grouped
            WHERE ($6::numeric IS NULL OR {sort_sql} < $6 OR ({sort_sql} = $6 AND orchestrator_address > $7))
         ORDER BY {sort_sql} DESC, orchestrator_address ASC
            LIMIT $8"#,
        sort_sql = sort.order_sql(),
    );
    let rows = sqlx::query(&sql)
        .bind(state.chain_id)
        .bind(from)
        .bind(to)
        .bind(&valuation_version)
        .bind(job_type.sql_filter())
        .bind(cursor.as_ref().map(|c| c.sort_value.clone()))
        .bind(cursor.as_ref().map(|c| c.orchestrator_address.clone()))
        .bind(limit)
        .fetch_all(&state.pg)
        .await?;

    let data: Vec<PayoutLeaderboardRow> = rows.iter().map(to_leaderboard_row).collect();
    let next_cursor = data.last().map(|row| {
        let sort_value = match sort {
            LeaderboardSort::CommissionUsd => row.sum_commission_usd.clone(),
            LeaderboardSort::TicketCount => row.ticket_count.clone(),
            LeaderboardSort::FaceValueUsd => row.sum_face_value_usd.clone(),
        };
        LeaderboardCursor {
            sort_value: BigDecimal::from_str(&sort_value).unwrap_or_default(),
            orchestrator_address: row.orchestrator_address.clone(),
        }
        .encode()
    });

    Ok(Json(PayoutLeaderboardResponse {
        data,
        meta: PayoutLeaderboardMeta {
            chain_id: state.chain_id.to_string(),
            from: from.to_string(),
            to: to.to_string(),
            valuation_version,
            job_type: job_type.as_str().to_string(),
            sort: sort.as_str().to_string(),
            next_cursor,
        },
    }))
}

#[utoipa::path(
    get,
    path = "/payouts/summary/daily/{date}",
    tag = "Payouts",
    params(
        ("date" = String, Path, description = "Any ISO date within the requested day."),
        PayoutSummaryQuery
    ),
    responses(
        (status = 200, description = "Daily payout summary.", body = PayoutSummaryResponse),
        (status = 400, description = "Invalid date or job_type.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn summary_daily(
    State(state): State<AppState>,
    Path(date): Path<String>,
    Query(q): Query<PayoutSummaryQuery>,
) -> Result<Json<PayoutSummaryResponse>, ApiError> {
    let day = parse_date(&date, "date")?;
    summary_for_range(state, day, day, q).await
}

#[utoipa::path(
    get,
    path = "/payouts/summary/weekly/{date}",
    tag = "Payouts",
    params(
        ("date" = String, Path, description = "Any ISO date within the requested week."),
        PayoutSummaryQuery
    ),
    responses(
        (status = 200, description = "Weekly payout summary (Mon-Sun containing the date).", body = PayoutSummaryResponse),
        (status = 400, description = "Invalid date or job_type.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn summary_weekly(
    State(state): State<AppState>,
    Path(date): Path<String>,
    Query(q): Query<PayoutSummaryQuery>,
) -> Result<Json<PayoutSummaryResponse>, ApiError> {
    let date = parse_date(&date, "date")?;
    let weekday = date.weekday().num_days_from_monday() as i64;
    let start = date - Duration::days(weekday);
    let end = start + Duration::days(6);
    summary_for_range(state, start, end, q).await
}

#[utoipa::path(
    get,
    path = "/payouts/summary/monthly/{date}",
    tag = "Payouts",
    params(
        ("date" = String, Path, description = "Any ISO date within the requested month."),
        PayoutSummaryQuery
    ),
    responses(
        (status = 200, description = "Monthly payout summary (calendar month containing the date).", body = PayoutSummaryResponse),
        (status = 400, description = "Invalid date or job_type.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn summary_monthly(
    State(state): State<AppState>,
    Path(date): Path<String>,
    Query(q): Query<PayoutSummaryQuery>,
) -> Result<Json<PayoutSummaryResponse>, ApiError> {
    let date = parse_date(&date, "date")?;
    let start = date
        .with_day(1)
        .ok_or_else(|| ApiError::bad_request("invalid date"))?;
    let next_month = if start.month() == 12 {
        NaiveDate::from_ymd_opt(start.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(start.year(), start.month() + 1, 1)
    }
    .ok_or_else(|| ApiError::bad_request("invalid date"))?;
    let end = next_month - Duration::days(1);
    summary_for_range(state, start, end, q).await
}

async fn summary_for_range(
    state: AppState,
    start: NaiveDate,
    end: NaiveDate,
    q: PayoutSummaryQuery,
) -> Result<Json<PayoutSummaryResponse>, ApiError> {
    let job_type = JobType::parse(q.job_type.as_deref())?;
    let valuation_version = q
        .valuation_version
        .clone()
        .unwrap_or_else(|| state.default_version.clone());
    // Aggregate the rollup (cheap, additive metrics) and compute true
    // distinct-gateway count from raw_protocol_events as a sub-query.
    // SUM(distinct_gateways) over rollup rows would double-count gateways
    // that paid multiple orchs on the same day; the rollup only stores
    // per-(day, orch) distinct counts, not addresses.
    let row = sqlx::query(
        r#"SELECT
               COALESCE(SUM(ticket_count), 0)::bigint AS ticket_count,
               COALESCE(SUM(sum_face_value_native), 0) AS sum_face_value_native,
               COALESCE(SUM(sum_face_value_usd), 0) AS sum_face_value_usd,
               COALESCE(SUM(sum_commission_native), 0) AS sum_commission_native,
               COALESCE(SUM(sum_commission_usd), 0) AS sum_commission_usd,
               COALESCE(SUM(sum_delegators_share_native), 0) AS sum_delegators_share_native,
               COALESCE(SUM(sum_delegators_share_usd), 0) AS sum_delegators_share_usd,
               COALESCE(SUM(usd_rows_priced), 0)::bigint AS usd_rows_priced,
               COALESCE((
                 SELECT COUNT(DISTINCT r.from_address)
                   FROM raw_protocol_events r
              LEFT JOIN broadcaster_classifications bc
                     ON bc.chain_id = r.chain_id
                    AND bc.address  = r.from_address
                  WHERE r.chain_id      = $1
                    AND r.event_name    = 'WinningTicketRedeemed'
                    AND r.is_canonical  = TRUE
                    AND r.finality      = 'finalized'
                    AND r.from_address IS NOT NULL
                    AND r.block_timestamp >= $2::timestamptz
                    AND r.block_timestamp <  ($3::date + 1)::timestamptz
                    AND ($5::text IS NULL
                         OR COALESCE(bc.kind, 'transcoding') = $5)
               ), 0)::bigint AS distinct_gateways
          FROM orch_payouts_daily
         WHERE chain_id = $1
           AND day_utc >= $2
           AND day_utc <= $3
           AND valuation_version = $4
           AND ($5::text IS NULL OR broadcaster_kind = $5)"#,
    )
    .bind(state.chain_id)
    .bind(start)
    .bind(end)
    .bind(&valuation_version)
    .bind(job_type.sql_filter())
    .fetch_one(&state.pg)
    .await?;

    Ok(Json(PayoutSummaryResponse {
        period_start: start.to_string(),
        period_end: end.to_string(),
        valuation_version,
        job_type: job_type.as_str().to_string(),
        ticket_count: row.get::<i64, _>("ticket_count").to_string(),
        sum_face_value_native: row
            .get::<BigDecimal, _>("sum_face_value_native")
            .normalized()
            .to_string(),
        sum_face_value_usd: row
            .get::<BigDecimal, _>("sum_face_value_usd")
            .normalized()
            .to_string(),
        sum_commission_native: row
            .get::<BigDecimal, _>("sum_commission_native")
            .normalized()
            .to_string(),
        sum_commission_usd: row
            .get::<BigDecimal, _>("sum_commission_usd")
            .normalized()
            .to_string(),
        sum_delegators_share_native: row
            .get::<BigDecimal, _>("sum_delegators_share_native")
            .normalized()
            .to_string(),
        sum_delegators_share_usd: row
            .get::<BigDecimal, _>("sum_delegators_share_usd")
            .normalized()
            .to_string(),
        distinct_gateways: row.get::<i64, _>("distinct_gateways").to_string(),
        usd_rows_priced: row.get::<i64, _>("usd_rows_priced").to_string(),
    }))
}

fn to_leaderboard_row(r: &sqlx::postgres::PgRow) -> PayoutLeaderboardRow {
    PayoutLeaderboardRow {
        orchestrator_address: r.get("orchestrator_address"),
        display_name: r.try_get("display_name").ok(),
        avatar_url: r.try_get("avatar_url").ok(),
        ticket_count: r.get::<i64, _>("ticket_count").to_string(),
        sum_face_value_native: r
            .get::<BigDecimal, _>("sum_face_value_native")
            .normalized()
            .to_string(),
        sum_face_value_usd: r
            .get::<BigDecimal, _>("sum_face_value_usd")
            .normalized()
            .to_string(),
        sum_commission_native: r
            .get::<BigDecimal, _>("sum_commission_native")
            .normalized()
            .to_string(),
        sum_commission_usd: r
            .get::<BigDecimal, _>("sum_commission_usd")
            .normalized()
            .to_string(),
        sum_delegators_share_native: r
            .get::<BigDecimal, _>("sum_delegators_share_native")
            .normalized()
            .to_string(),
        sum_delegators_share_usd: r
            .get::<BigDecimal, _>("sum_delegators_share_usd")
            .normalized()
            .to_string(),
        distinct_gateways: r.get::<i64, _>("distinct_gateways").to_string(),
        usd_rows_priced: r.get::<i64, _>("usd_rows_priced").to_string(),
    }
}

fn parse_date(value: &str, field: &str) -> Result<NaiveDate, ApiError> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| ApiError::bad_request(format!("invalid {field}: {value:?}; use YYYY-MM-DD")))
}

fn normalize_addr(s: &str) -> Result<String, ApiError> {
    let lower = s.to_lowercase();
    if lower.starts_with("0x") && lower.len() == 42 {
        Ok(lower)
    } else {
        Err(ApiError::bad_request(format!("invalid address: {s}")))
    }
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

    // Integration test — needs a live Postgres + .env / DATABASE_URL.
    // CI's plain `cargo test --workspace` skips this; run locally with
    // `cargo test -p livepeer-api -- --ignored`.
    #[tokio::test]
    #[ignore = "requires DATABASE_URL or workspace-root .env"]
    async fn leaderboard_and_summaries_read_from_rollup_table() {
        let ctx = TestContext::new().await;
        let orch_a = "0x1111111111111111111111111111111111111111";
        let orch_b = "0x2222222222222222222222222222222222222222";
        sqlx::query(
            r#"INSERT INTO orchestrator_ens (chain_id, address, ens_name, ens_avatar_url, ens_last_resolved_at)
               VALUES ($1, $2, 'alpha.eth', 'https://ens.alpha/avatar.png', now()),
                      ($1, $3, 'beta.eth', 'https://ens.beta/avatar.png', now())"#,
        )
        .bind(ctx.chain_id)
        .bind(orch_a)
        .bind(orch_b)
        .execute(&ctx.pg)
        .await
        .unwrap();
        sqlx::query(
            r#"INSERT INTO name_avatar_overrides (chain_id, address, display_name, avatar_url, notes, updated_at)
               VALUES ($1, $2, 'override-alpha', 'https://override.alpha/avatar.png', 'fixture', now())"#,
        )
        .bind(ctx.chain_id)
        .bind(orch_a)
        .execute(&ctx.pg)
        .await
        .unwrap();
        sqlx::query(
            r#"INSERT INTO orch_payouts_daily (
                   chain_id, day_utc, orchestrator_address, valuation_version, broadcaster_kind,
                   ticket_count, sum_face_value_native, sum_face_value_usd, sum_commission_native,
                   sum_commission_usd, sum_delegators_share_native, sum_delegators_share_usd,
                   distinct_gateways, usd_rows_priced, source_max_event_id, updated_at
               ) VALUES
                   ($1, '2026-01-01', $2, 'test-version', 'ai', 5, 10, 20, 7, 14, 3, 6, 2, 5, 10, now()),
                   ($1, '2026-01-02', $2, 'test-version', 'transcoding', 1, 2, 3, 1, 2, 1, 1, 1, 1, 11, now()),
                   ($1, '2026-01-01', $3, 'test-version', 'transcoding', 4, 8, 16, 4, 8, 4, 8, 1, 4, 12, now())"#,
        )
        .bind(ctx.chain_id)
        .bind(orch_a)
        .bind(orch_b)
        .execute(&ctx.pg)
        .await
        .unwrap();

        let response = ctx
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/payouts/leaderboard?from=2026-01-01&to=2026-01-31&valuation_version=test-version&limit=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        let data = body["data"].as_array().unwrap();
        assert_eq!(data.len(), 1);
        assert_eq!(data[0]["orchestrator_address"], orch_a);
        assert_eq!(data[0]["display_name"], "override-alpha");
        assert_eq!(data[0]["sum_commission_usd"], "16");
        let cursor = body["meta"]["next_cursor"].as_str().unwrap().to_string();

        let response = ctx
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/payouts/leaderboard?from=2026-01-01&to=2026-01-31&valuation_version=test-version&limit=1&cursor={cursor}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = json_body(response).await;
        let data = body["data"].as_array().unwrap();
        assert_eq!(data.len(), 1);
        assert_eq!(data[0]["orchestrator_address"], orch_b);

        let response = ctx
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/payouts/summary/daily/2026-01-01?job_type=ai&valuation_version=test-version")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["ticket_count"], "5");
        assert_eq!(body["sum_commission_usd"], "14");

        let response = ctx
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/payouts/summary/weekly/2026-01-01?valuation_version=test-version")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = json_body(response).await;
        assert_eq!(body["ticket_count"], "10");
        assert_eq!(body["sum_face_value_native"], "20");

        let response = ctx
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/payouts/summary/monthly/2026-01-15?job_type=transcoding&valuation_version=test-version")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = json_body(response).await;
        assert_eq!(body["ticket_count"], "5");
        assert_eq!(body["sum_commission_usd"], "10");
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
                default_version: "test-version".to_string(),
                chain_id,
                ticket_broker_address: "0x0000000000000000000000000000000000000000".to_string(),
                archive,
                metrics: Arc::new(Metrics::new()),
                avatar_dir: None,
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
                            let _ = sqlx::query(
                                r#"DELETE FROM name_avatar_overrides WHERE chain_id = $1;
                                   DELETE FROM orchestrator_ens WHERE chain_id = $1;
                                   DELETE FROM orch_payouts_daily WHERE chain_id = $1;"#,
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
        950_000 + (nanos % 100_000)
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
