//! Operational endpoints — health + backfill status + frontend config.
//! SPEC §14.3.5.

use crate::{error::ApiError, state::AppState};
use axum::{extract::State, http::header, response::IntoResponse, Json};
use prometheus::{Encoder, TextEncoder};
use serde::Serialize;
use utoipa::ToSchema;

/// Runtime config consumed by the frontend bundle. The shape mirrors
/// `frontend-ui/src/types/config.ts::AppConfig` exactly. All fields are
/// env-overridable per the table in `frontend_config()`. Keys are
/// camelCase to match what the FE expects without re-shaping.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FrontendConfig {
    /// Empty string = relative URLs (same-origin). Almost always what you
    /// want when the FE is served by this API process.
    pub base_api_url: String,
    pub explorer_tx_base: String,
    pub explorer_address_base: String,
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

/// Per-deploy frontend config. Each field reads from a `FE_*` env var
/// and falls back to a sensible default.
///
/// | Env var                       | Default                                                                                  |
/// |-------------------------------|------------------------------------------------------------------------------------------|
/// | `FE_BASE_API_URL`             | `""`  (relative URLs, same-origin)                                                       |
/// | `FE_EXPLORER_TX_BASE`         | `https://arbiscan.io/tx/`                                                                |
/// | `FE_EXPLORER_ADDRESS_BASE`    | `https://arbiscan.io/address/`                                                           |
#[utoipa::path(
    get,
    path = "/config.json",
    tag = "Operational",
    responses(
        (status = 200, description = "Frontend runtime config, env-driven per-deploy.", body = FrontendConfig)
    )
)]
pub async fn frontend_config() -> Json<FrontendConfig> {
    Json(FrontendConfig {
        base_api_url: env_or("FE_BASE_API_URL", ""),
        explorer_tx_base: env_or("FE_EXPLORER_TX_BASE", "https://arbiscan.io/tx/"),
        explorer_address_base: env_or("FE_EXPLORER_ADDRESS_BASE", "https://arbiscan.io/address/"),
    })
}

#[utoipa::path(
    get,
    path = "/health",
    tag = "Operational",
    responses(
        (status = 200, description = "Liveness probe for the API process.", body = String, content_type = "text/plain")
    )
)]
pub async fn health(State(state): State<AppState>) -> &'static str {
    state
        .metrics
        .api_requests_total
        .with_label_values(&["/health", "2xx"])
        .inc();
    "ok"
}

/// Standard Prometheus exposition. SPEC §17.2.
#[utoipa::path(
    get,
    path = "/metrics",
    tag = "Operational",
    responses(
        (status = 200, description = "Prometheus metrics for the API process.", body = String, content_type = "text/plain")
    )
)]
pub async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let mut buf = Vec::new();
    let families = state.metrics.registry.gather();
    if encoder.encode(&families, &mut buf).is_err() {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "encode failed",
        )
            .into_response();
    }
    (
        [(header::CONTENT_TYPE, encoder.format_type().to_string())],
        String::from_utf8(buf).unwrap_or_default(),
    )
        .into_response()
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Operational summary of checkpoint progress and approximate table sizes.")]
pub struct BackfillStatus {
    pub checkpoints: Vec<Checkpoint>,
    pub raw_event_count: String,
    pub valuation_count: String,
    pub decode_failure_count: String,
    pub reorg_event_count: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Single checkpoint entry from indexer_checkpoints.")]
pub struct Checkpoint {
    pub name: String,
    pub last_processed_block: String,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[utoipa::path(
    get,
    path = "/backfills/status",
    tag = "Operational",
    responses(
        (status = 200, description = "Checkpoint and approximate row-count summary for the indexing pipeline.", body = BackfillStatus),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn backfill_status(
    State(state): State<AppState>,
) -> Result<Json<BackfillStatus>, ApiError> {
    use sqlx::Row;
    let checkpoint_rows = sqlx::query(
        "SELECT name, last_processed_block, updated_at FROM indexer_checkpoints ORDER BY name",
    )
    .fetch_all(&state.pg)
    .await?;
    let checkpoints = checkpoint_rows
        .iter()
        .map(|r| Checkpoint {
            name: r.get(0),
            last_processed_block: r.get::<i64, _>(1).to_string(),
            updated_at: r.get(2),
        })
        .collect();

    // Operational status favors low latency over exactness on the two largest tables.
    // reltuples is refreshed by VACUUM/ANALYZE and is good enough for an ops dashboard.
    let approx_rows = sqlx::query(
        r#"SELECT relname, GREATEST(reltuples::bigint, 0)
             FROM pg_class
            WHERE relname = ANY($1)"#,
    )
    .bind(vec!["raw_protocol_events", "event_valuations"])
    .fetch_all(&state.pg)
    .await?;
    let mut raw_event_count = 0i64;
    let mut valuation_count = 0i64;
    for r in &approx_rows {
        let relname: String = r.get(0);
        let count: i64 = r.get(1);
        match relname.as_str() {
            "raw_protocol_events" => raw_event_count = count,
            "event_valuations" => valuation_count = count,
            _ => {}
        }
    }
    let decode_failure_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM decode_failures WHERE resolved_at IS NULL")
            .fetch_one(&state.pg)
            .await?;
    let reorg_event_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reorg_events")
        .fetch_one(&state.pg)
        .await?;

    Ok(Json(BackfillStatus {
        checkpoints,
        raw_event_count: raw_event_count.to_string(),
        valuation_count: valuation_count.to_string(),
        decode_failure_count: decode_failure_count.to_string(),
        reorg_event_count: reorg_event_count.to_string(),
    }))
}
