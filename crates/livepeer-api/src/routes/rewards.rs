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
#[schema(description = "Query parameters for the orchestrator reward leaderboard.")]
pub struct RewardLeaderboardQuery {
    pub from: String,
    pub to: String,
    /// `orch_tokens_usd`, `reward_event_count`, or `total_tokens_usd`.
    pub sort: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
    pub valuation_version: Option<String>,
}

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
#[schema(description = "Query parameters for reward summary endpoints.")]
pub struct RewardSummaryQuery {
    pub valuation_version: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RewardLeaderboardRow {
    pub orchestrator_address: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub reward_event_count: String,
    pub sum_total_tokens: String,
    pub sum_total_tokens_usd: String,
    pub sum_orch_tokens: String,
    pub sum_orch_tokens_usd: String,
    pub sum_delegators_tokens: String,
    pub sum_delegators_tokens_usd: String,
    pub usd_rows_priced: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RewardLeaderboardMeta {
    pub chain_id: String,
    pub from: String,
    pub to: String,
    pub valuation_version: String,
    pub sort: String,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RewardLeaderboardResponse {
    pub data: Vec<RewardLeaderboardRow>,
    pub meta: RewardLeaderboardMeta,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RewardSummaryResponse {
    pub period_start: String,
    pub period_end: String,
    pub valuation_version: String,
    pub reward_event_count: String,
    pub sum_total_tokens: String,
    pub sum_total_tokens_usd: String,
    pub sum_orch_tokens: String,
    pub sum_orch_tokens_usd: String,
    pub sum_delegators_tokens: String,
    pub sum_delegators_tokens_usd: String,
    pub usd_rows_priced: String,
}

#[derive(Debug, Clone, Copy)]
enum LeaderboardSort {
    OrchTokensUsd,
    RewardEventCount,
    TotalTokensUsd,
}

impl LeaderboardSort {
    fn parse(value: Option<&str>) -> Result<Self, ApiError> {
        match value.unwrap_or("orch_tokens_usd") {
            "orch_tokens_usd" => Ok(Self::OrchTokensUsd),
            "reward_event_count" => Ok(Self::RewardEventCount),
            "total_tokens_usd" => Ok(Self::TotalTokensUsd),
            other => Err(ApiError::bad_request(format!(
                "invalid sort {other:?}; use orch_tokens_usd | reward_event_count | total_tokens_usd"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::OrchTokensUsd => "orch_tokens_usd",
            Self::RewardEventCount => "reward_event_count",
            Self::TotalTokensUsd => "total_tokens_usd",
        }
    }

    fn order_sql(self) -> &'static str {
        match self {
            Self::OrchTokensUsd => "sum_orch_tokens_usd",
            Self::RewardEventCount => "reward_event_count",
            Self::TotalTokensUsd => "sum_total_tokens_usd",
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
            "R{}|{}",
            self.sort_value.normalized(),
            self.orchestrator_address
        )
    }

    fn decode(raw: &str) -> Result<Self, ApiError> {
        let stripped = raw
            .strip_prefix('R')
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
    path = "/rewards/leaderboard",
    tag = "Rewards",
    params(RewardLeaderboardQuery),
    responses(
        (status = 200, description = "Paginated reward leaderboard aggregated by orchestrator.", body = RewardLeaderboardResponse),
        (status = 400, description = "Invalid date, sort, or cursor.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn leaderboard(
    State(state): State<AppState>,
    Query(q): Query<RewardLeaderboardQuery>,
) -> Result<Json<RewardLeaderboardResponse>, ApiError> {
    let from = parse_date(&q.from, "from")?;
    let to = parse_date(&q.to, "to")?;
    if to < from {
        return Err(ApiError::bad_request("to must be >= from"));
    }
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

    let sql = format!(
        r#"WITH grouped AS (
               SELECT
                   r.orchestrator_address,
                   COALESCE(o.display_name, e.ens_name) AS display_name,
                   COALESCE(o.avatar_url, e.ens_avatar_url) AS avatar_url,
                   SUM(r.reward_event_count)::bigint AS reward_event_count,
                   SUM(r.sum_total_tokens) AS sum_total_tokens,
                   SUM(r.sum_total_tokens_usd) AS sum_total_tokens_usd,
                   SUM(r.sum_orch_tokens) AS sum_orch_tokens,
                   SUM(r.sum_orch_tokens_usd) AS sum_orch_tokens_usd,
                   SUM(r.sum_delegators_tokens) AS sum_delegators_tokens,
                   SUM(r.sum_delegators_tokens_usd) AS sum_delegators_tokens_usd,
                   SUM(r.usd_rows_priced)::bigint AS usd_rows_priced
               FROM orch_rewards_daily r
          LEFT JOIN orchestrator_ens e
                 ON e.chain_id = r.chain_id
                AND e.address = r.orchestrator_address
          LEFT JOIN name_avatar_overrides o
                 ON o.chain_id = r.chain_id
                AND o.address = r.orchestrator_address
              WHERE r.chain_id = $1
                AND r.day_utc >= $2
                AND r.day_utc <= $3
                AND r.valuation_version = $4
              GROUP BY
                   r.orchestrator_address,
                   COALESCE(o.display_name, e.ens_name),
                   COALESCE(o.avatar_url, e.ens_avatar_url)
           )
           SELECT *
             FROM grouped
            WHERE ($5::numeric IS NULL)
               OR (
                    ({order_sql} < $5)
                 OR ({order_sql} = $5 AND orchestrator_address > $6)
               )
         ORDER BY {order_sql} DESC, orchestrator_address ASC
            LIMIT $7"#,
        order_sql = sort.order_sql(),
    );

    let rows = sqlx::query(&sql)
        .bind(state.chain_id)
        .bind(from)
        .bind(to)
        .bind(&valuation_version)
        .bind(cursor.as_ref().map(|c| c.sort_value.clone()))
        .bind(cursor.as_ref().map(|c| c.orchestrator_address.clone()))
        .bind(limit + 1)
        .fetch_all(&state.pg)
        .await?;

    let has_more = rows.len() as i64 > limit;
    let page_rows = rows.into_iter().take(limit as usize).collect::<Vec<_>>();
    let mut data = Vec::with_capacity(page_rows.len());
    let mut next_cursor = None;

    for row in page_rows {
        let body = RewardLeaderboardRow {
            orchestrator_address: row.get("orchestrator_address"),
            display_name: row.get("display_name"),
            avatar_url: row.get("avatar_url"),
            reward_event_count: row.get::<i64, _>("reward_event_count").to_string(),
            sum_total_tokens: decimal_text(&row, "sum_total_tokens"),
            sum_total_tokens_usd: decimal_text(&row, "sum_total_tokens_usd"),
            sum_orch_tokens: decimal_text(&row, "sum_orch_tokens"),
            sum_orch_tokens_usd: decimal_text(&row, "sum_orch_tokens_usd"),
            sum_delegators_tokens: decimal_text(&row, "sum_delegators_tokens"),
            sum_delegators_tokens_usd: decimal_text(&row, "sum_delegators_tokens_usd"),
            usd_rows_priced: row.get::<i64, _>("usd_rows_priced").to_string(),
        };
        if has_more {
            let sort_value = match sort {
                LeaderboardSort::OrchTokensUsd => parse_decimal(&body.sum_orch_tokens_usd)?,
                LeaderboardSort::RewardEventCount => parse_decimal(&body.reward_event_count)?,
                LeaderboardSort::TotalTokensUsd => parse_decimal(&body.sum_total_tokens_usd)?,
            };
            next_cursor = Some(
                LeaderboardCursor {
                    sort_value,
                    orchestrator_address: body.orchestrator_address.clone(),
                }
                .encode(),
            );
        }
        data.push(body);
    }

    Ok(Json(RewardLeaderboardResponse {
        data,
        meta: RewardLeaderboardMeta {
            chain_id: state.chain_id.to_string(),
            from: from.to_string(),
            to: to.to_string(),
            valuation_version,
            sort: sort.as_str().to_string(),
            next_cursor: if has_more { next_cursor } else { None },
        },
    }))
}

#[utoipa::path(
    get,
    path = "/rewards/summary/daily/{date}",
    tag = "Rewards",
    params(("date" = String, Path, description = "Any date inside the desired UTC day"), RewardSummaryQuery),
    responses((status = 200, description = "Daily reward summary.", body = RewardSummaryResponse))
)]
pub async fn summary_daily(
    State(state): State<AppState>,
    Path(date): Path<String>,
    Query(q): Query<RewardSummaryQuery>,
) -> Result<Json<RewardSummaryResponse>, ApiError> {
    let day = parse_date(&date, "date")?;
    let value = summary_for_range(&state, day, day, q.valuation_version).await?;
    Ok(Json(value))
}

#[utoipa::path(
    get,
    path = "/rewards/summary/weekly/{date}",
    tag = "Rewards",
    params(("date" = String, Path, description = "Any date inside the desired ISO week"), RewardSummaryQuery),
    responses((status = 200, description = "Weekly reward summary (Mon-Sun UTC).", body = RewardSummaryResponse))
)]
pub async fn summary_weekly(
    State(state): State<AppState>,
    Path(date): Path<String>,
    Query(q): Query<RewardSummaryQuery>,
) -> Result<Json<RewardSummaryResponse>, ApiError> {
    let day = parse_date(&date, "date")?;
    let start = day - Duration::days(day.weekday().num_days_from_monday() as i64);
    let end = start + Duration::days(6);
    let value = summary_for_range(&state, start, end, q.valuation_version).await?;
    Ok(Json(value))
}

#[utoipa::path(
    get,
    path = "/rewards/summary/monthly/{date}",
    tag = "Rewards",
    params(("date" = String, Path, description = "Any date inside the desired UTC calendar month"), RewardSummaryQuery),
    responses((status = 200, description = "Monthly reward summary.", body = RewardSummaryResponse))
)]
pub async fn summary_monthly(
    State(state): State<AppState>,
    Path(date): Path<String>,
    Query(q): Query<RewardSummaryQuery>,
) -> Result<Json<RewardSummaryResponse>, ApiError> {
    let day = parse_date(&date, "date")?;
    let start = day
        .with_day(1)
        .ok_or_else(|| ApiError::bad_request("invalid date"))?;
    let end = if start.month() == 12 {
        NaiveDate::from_ymd_opt(start.year() + 1, 1, 1)
            .ok_or_else(|| ApiError::bad_request("invalid date"))?
            - Duration::days(1)
    } else {
        NaiveDate::from_ymd_opt(start.year(), start.month() + 1, 1)
            .ok_or_else(|| ApiError::bad_request("invalid date"))?
            - Duration::days(1)
    };
    let value = summary_for_range(&state, start, end, q.valuation_version).await?;
    Ok(Json(value))
}

async fn summary_for_range(
    state: &AppState,
    start: NaiveDate,
    end: NaiveDate,
    valuation_version: Option<String>,
) -> Result<RewardSummaryResponse, ApiError> {
    let valuation_version = valuation_version.unwrap_or_else(|| state.default_version.clone());
    let row = sqlx::query(
        r#"SELECT
               COALESCE(SUM(reward_event_count), 0)::bigint AS reward_event_count,
               COALESCE(SUM(sum_total_tokens), 0) AS sum_total_tokens,
               COALESCE(SUM(sum_total_tokens_usd), 0) AS sum_total_tokens_usd,
               COALESCE(SUM(sum_orch_tokens), 0) AS sum_orch_tokens,
               COALESCE(SUM(sum_orch_tokens_usd), 0) AS sum_orch_tokens_usd,
               COALESCE(SUM(sum_delegators_tokens), 0) AS sum_delegators_tokens,
               COALESCE(SUM(sum_delegators_tokens_usd), 0) AS sum_delegators_tokens_usd,
               COALESCE(SUM(usd_rows_priced), 0)::bigint AS usd_rows_priced
          FROM orch_rewards_daily
         WHERE chain_id = $1
           AND day_utc >= $2
           AND day_utc <= $3
           AND valuation_version = $4"#,
    )
    .bind(state.chain_id)
    .bind(start)
    .bind(end)
    .bind(&valuation_version)
    .fetch_one(&state.pg)
    .await?;

    Ok(RewardSummaryResponse {
        period_start: start.to_string(),
        period_end: end.to_string(),
        valuation_version,
        reward_event_count: row.get::<i64, _>("reward_event_count").to_string(),
        sum_total_tokens: decimal_text(&row, "sum_total_tokens"),
        sum_total_tokens_usd: decimal_text(&row, "sum_total_tokens_usd"),
        sum_orch_tokens: decimal_text(&row, "sum_orch_tokens"),
        sum_orch_tokens_usd: decimal_text(&row, "sum_orch_tokens_usd"),
        sum_delegators_tokens: decimal_text(&row, "sum_delegators_tokens"),
        sum_delegators_tokens_usd: decimal_text(&row, "sum_delegators_tokens_usd"),
        usd_rows_priced: row.get::<i64, _>("usd_rows_priced").to_string(),
    })
}

fn parse_date(raw: &str, field: &str) -> Result<NaiveDate, ApiError> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .map_err(|_| ApiError::bad_request(format!("invalid {field} {raw:?}; use YYYY-MM-DD")))
}

fn normalize_addr(raw: &str) -> Result<String, ApiError> {
    let lowered = raw.to_lowercase();
    if lowered.len() != 42 || !lowered.starts_with("0x") {
        return Err(ApiError::bad_request("invalid address"));
    }
    Ok(lowered)
}

fn decimal_text(row: &sqlx::postgres::PgRow, column: &str) -> String {
    row.get::<BigDecimal, _>(column).normalized().to_string()
}

fn parse_decimal(raw: &str) -> Result<BigDecimal, ApiError> {
    BigDecimal::from_str(raw)
        .map_err(|_| ApiError::internal("failed to parse numeric cursor value"))
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
    async fn leaderboard_and_summaries_read_from_reward_rollup_table() {
        let ctx = TestContext::new().await;
        let orch_a = "0x3333333333333333333333333333333333333333";
        let orch_b = "0x4444444444444444444444444444444444444444";

        sqlx::query(
            r#"INSERT INTO orchestrator_ens (chain_id, address, ens_name, ens_avatar_url, ens_last_resolved_at)
               VALUES ($1, $2, 'gamma.eth', 'https://ens.gamma/avatar.png', now()),
                      ($1, $3, 'delta.eth', 'https://ens.delta/avatar.png', now())"#,
        )
        .bind(ctx.chain_id)
        .bind(orch_a)
        .bind(orch_b)
        .execute(&ctx.pg)
        .await
        .unwrap();
        sqlx::query(
            r#"INSERT INTO orch_rewards_daily (
                   chain_id, day_utc, orchestrator_address, valuation_version,
                   reward_event_count, sum_total_tokens, sum_total_tokens_usd,
                   sum_orch_tokens, sum_orch_tokens_usd, sum_delegators_tokens,
                   sum_delegators_tokens_usd, usd_rows_priced, source_max_event_id, updated_at
               ) VALUES
                   ($1, '2026-02-01', $2, 'test-version', 2, 10, 20, 4, 8, 6, 12, 2, 21, now()),
                   ($1, '2026-02-02', $2, 'test-version', 1, 5, 10, 2, 4, 3, 6, 1, 22, now()),
                   ($1, '2026-02-01', $3, 'test-version', 3, 9, 18, 3, 6, 6, 12, 3, 23, now())"#,
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
                    .uri("/api/v1/rewards/leaderboard?from=2026-02-01&to=2026-02-28&valuation_version=test-version&limit=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["data"][0]["orchestrator_address"], orch_a);
        assert_eq!(body["data"][0]["sum_orch_tokens_usd"], "12");
        let cursor = body["meta"]["next_cursor"].as_str().unwrap().to_string();

        let response = ctx
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/rewards/leaderboard?from=2026-02-01&to=2026-02-28&valuation_version=test-version&limit=1&cursor={cursor}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = json_body(response).await;
        assert_eq!(body["data"][0]["orchestrator_address"], orch_b);

        let response = ctx
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/rewards/summary/daily/2026-02-01?valuation_version=test-version")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = json_body(response).await;
        assert_eq!(body["reward_event_count"], "5");
        assert_eq!(body["sum_orch_tokens_usd"], "14");
    }

    struct TestContext {
        app: axum::Router,
        pg: PgPool,
        chain_id: i64,
    }

    impl TestContext {
        async fn new() -> Self {
            let pg = db::connect(&test_database_url(), 5).await.unwrap();
            ensure_reward_table(&pg).await;
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
                                r#"DELETE FROM orchestrator_ens WHERE chain_id = $1;
                                   DELETE FROM orch_rewards_daily WHERE chain_id = $1;"#,
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

    async fn ensure_reward_table(pg: &PgPool) {
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS orch_rewards_daily (
                   chain_id BIGINT NOT NULL,
                   day_utc DATE NOT NULL,
                   orchestrator_address TEXT NOT NULL,
                   valuation_version TEXT NOT NULL,
                   reward_event_count BIGINT NOT NULL,
                   sum_total_tokens NUMERIC(38, 18) NOT NULL,
                   sum_total_tokens_usd NUMERIC(38, 18) NOT NULL,
                   sum_orch_tokens NUMERIC(38, 18) NOT NULL,
                   sum_orch_tokens_usd NUMERIC(38, 18) NOT NULL,
                   sum_delegators_tokens NUMERIC(38, 18) NOT NULL,
                   sum_delegators_tokens_usd NUMERIC(38, 18) NOT NULL,
                   usd_rows_priced BIGINT NOT NULL,
                   source_max_event_id BIGINT NOT NULL,
                   updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                   PRIMARY KEY (chain_id, day_utc, orchestrator_address, valuation_version)
               )"#,
        )
        .execute(pg)
        .await
        .unwrap();
    }

    fn unique_chain_id() -> i64 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i64;
        960_000 + (nanos % 100_000)
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
            user.expect("POSTGRES_USER"),
            password.expect("POSTGRES_PASSWORD"),
            port.expect("POSTGRES_PORT"),
            db_name.expect("POSTGRES_DB")
        )
    }
}
