use crate::{error::ApiError, state::AppState};
use axum::{
    extract::{Query, State},
    Json,
};
use chrono::{Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use utoipa::{IntoParams, ToSchema};

const MAX_RANGE_DAYS: i64 = 730;

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
#[schema(description = "Query parameters for ticket daily timeseries.")]
pub struct TicketsTimeseriesQuery {
    pub start: String,
    pub end: String,
    /// `ai`, `transcoding`, or `both`.
    pub job_type: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TicketSeriesRow {
    pub date: String,
    pub count: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TicketsTimeseriesResponse {
    pub start: String,
    pub end: String,
    pub job_type: String,
    pub ai: Vec<TicketSeriesRow>,
    pub transcoding: Vec<TicketSeriesRow>,
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
}

#[utoipa::path(
    get,
    path = "/tickets/timeseries/daily",
    tag = "Tickets",
    params(TicketsTimeseriesQuery),
    responses(
        (status = 200, description = "Daily ticket counts split by gateway kind.", body = TicketsTimeseriesResponse),
        (status = 400, description = "Invalid date range or job_type.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn timeseries_daily(
    State(state): State<AppState>,
    Query(q): Query<TicketsTimeseriesQuery>,
) -> Result<Json<TicketsTimeseriesResponse>, ApiError> {
    let start = parse_date(&q.start, "start")?;
    let end = parse_date(&q.end, "end")?;
    if end < start {
        return Err(ApiError::bad_request("end must be >= start"));
    }
    if end.signed_duration_since(start).num_days() > MAX_RANGE_DAYS {
        return Err(ApiError::bad_request("date range must be <= 730 days"));
    }
    let job_type = JobType::parse(q.job_type.as_deref())?;

    let rows = sqlx::query(
        r#"SELECT day_utc, broadcaster_kind, ticket_count
             FROM tickets_daily
            WHERE chain_id = $1
              AND day_utc >= $2
              AND day_utc <= $3
         ORDER BY day_utc ASC, broadcaster_kind ASC"#,
    )
    .bind(state.chain_id)
    .bind(start)
    .bind(end)
    .fetch_all(&state.pg)
    .await?;

    let mut counts = std::collections::HashMap::<(NaiveDate, String), i64>::new();
    for row in rows {
        counts.insert(
            (row.get("day_utc"), row.get("broadcaster_kind")),
            row.get("ticket_count"),
        );
    }

    let mut ai = Vec::new();
    let mut transcoding = Vec::new();
    let mut current = start;
    while current <= end {
        if !matches!(job_type, JobType::Transcoding) {
            ai.push(TicketSeriesRow {
                date: current.to_string(),
                count: counts
                    .get(&(current, "ai".to_string()))
                    .copied()
                    .unwrap_or(0)
                    .to_string(),
            });
        }
        if !matches!(job_type, JobType::Ai) {
            transcoding.push(TicketSeriesRow {
                date: current.to_string(),
                count: counts
                    .get(&(current, "transcoding".to_string()))
                    .copied()
                    .unwrap_or(0)
                    .to_string(),
            });
        }
        current += Duration::days(1);
    }

    Ok(Json(TicketsTimeseriesResponse {
        start: start.to_string(),
        end: end.to_string(),
        job_type: job_type.as_str().to_string(),
        ai,
        transcoding,
    }))
}

fn parse_date(raw: &str, field: &str) -> Result<NaiveDate, ApiError> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .map_err(|_| ApiError::bad_request(format!("invalid {field} {raw:?}; use YYYY-MM-DD")))
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

    #[tokio::test]
    async fn daily_timeseries_reads_ticket_rollup_table() {
        let ctx = TestContext::new().await;
        sqlx::query(
            r#"INSERT INTO tickets_daily (
                   chain_id, day_utc, broadcaster_kind,
                   ticket_count, distinct_orchestrators, distinct_gateways,
                   source_max_event_id, updated_at
               ) VALUES
                   ($1, '2026-03-01', 'ai', 3, 2, 1, 31, now()),
                   ($1, '2026-03-01', 'transcoding', 7, 4, 2, 32, now()),
                   ($1, '2026-03-02', 'transcoding', 2, 1, 1, 33, now())"#,
        )
        .bind(ctx.chain_id)
        .execute(&ctx.pg)
        .await
        .unwrap();

        let response = ctx
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/tickets/timeseries/daily?start=2026-03-01&end=2026-03-03")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["ai"][0]["count"], "3");
        assert_eq!(body["ai"][1]["count"], "0");
        assert_eq!(body["transcoding"][0]["count"], "7");
        assert_eq!(body["transcoding"][1]["count"], "2");
        assert_eq!(body["transcoding"][2]["count"], "0");
    }

    struct TestContext {
        app: axum::Router,
        pg: PgPool,
        chain_id: i64,
    }

    impl TestContext {
        async fn new() -> Self {
            let pg = db::connect(&test_database_url(), 5).await.unwrap();
            ensure_ticket_table(&pg).await;
            let chain_id = unique_chain_id();
            let archive = Provider::new("test", "http://127.0.0.1:9").unwrap();
            let state = AppState {
                pg: pg.clone(),
                default_version: "test-version".to_string(),
                chain_id,
                ticket_broker_address: "0x0000000000000000000000000000000000000000".to_string(),
                archive,
                metrics: Arc::new(Metrics::new()),
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
                            let _ = sqlx::query("DELETE FROM tickets_daily WHERE chain_id = $1")
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

    async fn ensure_ticket_table(pg: &PgPool) {
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS tickets_daily (
                   chain_id BIGINT NOT NULL,
                   day_utc DATE NOT NULL,
                   broadcaster_kind TEXT NOT NULL CHECK (broadcaster_kind IN ('ai', 'transcoding')),
                   ticket_count BIGINT NOT NULL,
                   distinct_orchestrators INT NOT NULL,
                   distinct_gateways INT NOT NULL,
                   source_max_event_id BIGINT NOT NULL,
                   updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                   PRIMARY KEY (chain_id, day_utc, broadcaster_kind)
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
        970_000 + (nanos % 100_000)
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
