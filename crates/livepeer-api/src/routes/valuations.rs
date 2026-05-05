//! Valuations endpoints. SPEC §14.3.2.

use crate::{error::ApiError, routes::events::ValuationInline, state::AppState};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use bigdecimal::BigDecimal;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Default, Deserialize, IntoParams, ToSchema)]
#[schema(description = "Filters for querying versioned event valuation outcomes.")]
pub struct ValuationsQuery {
    /// Optional lower block bound.
    pub from_block: Option<i64>,
    /// Optional upper block bound.
    pub to_block: Option<i64>,
    /// Filter valuations to a specific pricing version.
    pub version: Option<String>,
    /// Filter valuations by asset symbol.
    pub asset: Option<String>,
    /// Maximum number of rows to return.
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Top-level valuations collection response.")]
pub struct ValuationListResponse {
    pub data: Vec<ValuationRow>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(
    description = "Single immutable valuation outcome row for one (event_id, valuation_version, asset) tuple."
)]
pub struct ValuationRow {
    pub event_id: String,
    pub valuation_version: String,
    pub asset: String,
    pub block_number: String,
    pub amount_native: String,
    pub native_usd_price: Option<String>,
    pub amount_usd: Option<String>,
    pub source: String,
    pub pricing_method: String,
    pub status: String,
}

#[utoipa::path(
    get,
    path = "/events/{id}/valuation",
    tag = "Valuations",
    params(
        ("id" = i64, Path, description = "Event identifier."),
        ValuationsQuery
    ),
    responses(
        (status = 200, description = "All valuation rows attached to a single event.", body = Vec<crate::routes::events::ValuationInline>),
        (status = 404, description = "No valuations exist for the requested event.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn for_event(
    State(state): State<AppState>,
    Path(event_id): Path<i64>,
    Query(q): Query<ValuationsQuery>,
) -> Result<Json<Vec<ValuationInline>>, ApiError> {
    let version_filter = q.version.is_some();
    let sql = format!(
        r#"SELECT asset, valuation_version, amount_native, native_usd_price,
                  amount_usd, source, pricing_method, status
             FROM event_valuations
            WHERE event_id = $1
              {filter}
            ORDER BY asset"#,
        filter = if version_filter {
            "AND valuation_version = $2"
        } else {
            ""
        }
    );
    let mut query = sqlx::query(&sql).bind(event_id);
    if let Some(v) = q.version.as_ref() {
        query = query.bind(v);
    }
    let rows = query.fetch_all(&state.pg).await?;
    if rows.is_empty() {
        return Err(ApiError::not_found(format!(
            "no valuations for event {event_id}"
        )));
    }
    Ok(Json(
        rows.iter()
            .map(|r| ValuationInline {
                asset: r.get(0),
                valuation_version: r.get(1),
                amount_native: r.get::<BigDecimal, _>(2).to_string(),
                native_usd_price: r.get::<Option<BigDecimal>, _>(3).map(|v| v.to_string()),
                amount_usd: r.get::<Option<BigDecimal>, _>(4).map(|v| v.to_string()),
                source: r.get(5),
                pricing_method: r.get(6),
                status: r.get(7),
            })
            .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/valuations",
    tag = "Valuations",
    params(ValuationsQuery),
    responses(
        (status = 200, description = "Valuation rows across events, optionally filtered by version, asset, and block range.", body = ValuationListResponse),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn list(
    State(state): State<AppState>,
    Query(q): Query<ValuationsQuery>,
) -> Result<Json<ValuationListResponse>, ApiError> {
    let limit = q.limit.unwrap_or(100).min(1000) as i64;
    let mut where_clauses: Vec<String> = vec!["chain_id = $1".to_string()];
    let mut binds: Vec<BindV> = vec![];
    let mut idx = 2u8;
    if let Some(v) = q.version {
        where_clauses.push(format!("valuation_version = ${idx}"));
        binds.push(BindV::Str(v));
        idx += 1;
    }
    if let Some(a) = q.asset {
        where_clauses.push(format!("asset = ${idx}"));
        binds.push(BindV::Str(a.to_uppercase()));
        idx += 1;
    }
    if let Some(f) = q.from_block {
        where_clauses.push(format!("block_number >= ${idx}"));
        binds.push(BindV::I64(f));
        idx += 1;
    }
    if let Some(t) = q.to_block {
        where_clauses.push(format!("block_number <= ${idx}"));
        binds.push(BindV::I64(t));
        idx += 1;
    }
    let _ = idx;

    let sql = format!(
        r#"SELECT event_id, valuation_version, asset, block_number,
                  amount_native, native_usd_price, amount_usd,
                  source, pricing_method, status
             FROM event_valuations
            WHERE {where_clauses}
            ORDER BY block_number, event_id, asset
            LIMIT {limit}"#,
        where_clauses = where_clauses.join(" AND "),
    );
    let mut query = sqlx::query(&sql).bind(state.chain_id);
    for b in &binds {
        query = match b {
            BindV::I64(v) => query.bind(*v),
            BindV::Str(s) => query.bind(s),
        };
    }
    let rows = query.fetch_all(&state.pg).await?;

    Ok(Json(ValuationListResponse {
        data: rows
            .iter()
            .map(|r| ValuationRow {
                event_id: r.get::<i64, _>(0).to_string(),
                valuation_version: r.get(1),
                asset: r.get(2),
                block_number: r.get::<i64, _>(3).to_string(),
                amount_native: r.get::<BigDecimal, _>(4).to_string(),
                native_usd_price: r.get::<Option<BigDecimal>, _>(5).map(|v| v.to_string()),
                amount_usd: r.get::<Option<BigDecimal>, _>(6).map(|v| v.to_string()),
                source: r.get(7),
                pricing_method: r.get(8),
                status: r.get(9),
            })
            .collect(),
    }))
}

enum BindV {
    I64(i64),
    Str(String),
}
