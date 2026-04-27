//! Operational endpoints — health + backfill status. SPEC §14.3.5.

use crate::{error::ApiError, state::AppState};
use axum::{extract::State, Json};
use serde::Serialize;

pub async fn health() -> &'static str {
    "ok"
}

#[derive(Debug, Serialize)]
pub struct BackfillStatus {
    pub checkpoints: Vec<Checkpoint>,
    pub raw_event_count: String,
    pub valuation_count: String,
    pub decode_failure_count: String,
    pub reorg_event_count: String,
}

#[derive(Debug, Serialize)]
pub struct Checkpoint {
    pub name: String,
    pub last_processed_block: String,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub async fn backfill_status(State(state): State<AppState>) -> Result<Json<BackfillStatus>, ApiError> {
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

    let raw_event_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM raw_protocol_events").fetch_one(&state.pg).await?;
    let valuation_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM event_valuations").fetch_one(&state.pg).await?;
    let decode_failure_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM decode_failures WHERE resolved_at IS NULL")
            .fetch_one(&state.pg)
            .await?;
    let reorg_event_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM reorg_events").fetch_one(&state.pg).await?;

    Ok(Json(BackfillStatus {
        checkpoints,
        raw_event_count: raw_event_count.to_string(),
        valuation_count: valuation_count.to_string(),
        decode_failure_count: decode_failure_count.to_string(),
        reorg_event_count: reorg_event_count.to_string(),
    }))
}
