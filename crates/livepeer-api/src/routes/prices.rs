//! Prices endpoints. SPEC §14.3.3.
//!
//! Backed by `token_prices_by_block` (populated by the valuator's on-chain reads).
//! v1 ships `/prices/{asset}/{quote}/block/{block}` and `/prices/{asset}/{quote}/latest`.
//! `/prices/.../range` lazy-fill is a v1.5 follow-up — needs the on-chain reader to
//! be split out from the valuator pass and run on demand.

use crate::{error::ApiError, state::AppState};
use axum::{extract::{Path, Query, State}, Json};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Cached token price row keyed by asset, quote, and block number.")]
pub struct PriceRow {
    pub chain_id: String,
    pub asset: String,
    pub quote: String,
    pub block_number: String,
    pub block_hash: String,
    pub block_timestamp: DateTime<Utc>,
    pub price: String,
    pub source: String,
    pub pool_address: Option<String>,
    pub oracle_address: Option<String>,
}

#[utoipa::path(
    get,
    path = "/prices/{asset}/{quote}/block/{block}",
    tag = "Prices",
    params(
        ("asset" = String, Path, description = "Base asset symbol, typically LPT or ETH."),
        ("quote" = String, Path, description = "Quote asset symbol, currently USD."),
        ("block" = i64, Path, description = "Exact block number to query.")
    ),
    responses(
        (status = 200, description = "Cached token price at the exact requested block.", body = PriceRow),
        (status = 404, description = "No cached price exists for the requested block.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn at_block(
    State(state): State<AppState>,
    Path((asset, quote, block)): Path<(String, String, i64)>,
) -> Result<Json<PriceRow>, ApiError> {
    let asset_u = asset.to_uppercase();
    let quote_u = quote.to_uppercase();
    // Multiple sources per (asset, quote, block) are possible (e.g. spot + TWAP);
    // pick the first ordering by source name for stability.
    let row = sqlx::query(
        r#"SELECT chain_id, asset, quote, block_number, block_hash, block_timestamp,
                  price, source, pool_address, oracle_address
             FROM token_prices_by_block
            WHERE chain_id = $1 AND asset = $2 AND quote = $3 AND block_number = $4
            ORDER BY source
            LIMIT 1"#,
    )
    .bind(state.chain_id)
    .bind(&asset_u)
    .bind(&quote_u)
    .bind(block)
    .fetch_optional(&state.pg)
    .await?;
    let Some(r) = row else {
        return Err(ApiError::not_found(format!(
            "no cached price for {asset_u}/{quote_u} at block {block}"
        )));
    };
    Ok(Json(to_price_row(&r)))
}

#[utoipa::path(
    get,
    path = "/prices/{asset}/{quote}/latest",
    tag = "Prices",
    params(
        ("asset" = String, Path, description = "Base asset symbol, typically LPT or ETH."),
        ("quote" = String, Path, description = "Quote asset symbol, currently USD.")
    ),
    responses(
        (status = 200, description = "Most recent cached token price for the pair.", body = PriceRow),
        (status = 404, description = "No cached price exists for the requested pair.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn latest(
    State(state): State<AppState>,
    Path((asset, quote)): Path<(String, String)>,
) -> Result<Json<PriceRow>, ApiError> {
    let asset_u = asset.to_uppercase();
    let quote_u = quote.to_uppercase();
    let row = sqlx::query(
        r#"SELECT chain_id, asset, quote, block_number, block_hash, block_timestamp,
                  price, source, pool_address, oracle_address
             FROM token_prices_by_block
            WHERE chain_id = $1 AND asset = $2 AND quote = $3
            ORDER BY block_number DESC, source
            LIMIT 1"#,
    )
    .bind(state.chain_id)
    .bind(&asset_u)
    .bind(&quote_u)
    .fetch_optional(&state.pg)
    .await?;
    let Some(r) = row else {
        return Err(ApiError::not_found(format!("no cached price for {asset_u}/{quote_u}")));
    };
    Ok(Json(to_price_row(&r)))
}

#[derive(Debug, Deserialize, IntoParams, ToSchema)]
#[schema(description = "Query parameters for listing cached prices across a block range.")]
pub struct RangeQuery {
    /// Inclusive starting block for the query.
    pub from_block: i64,
    /// Inclusive ending block for the query.
    pub to_block: i64,
    /// Optional source filter such as `chainlink_eth_usd` or `uniswap_v3_twap`.
    pub source: Option<String>,
    /// Maximum number of price rows to return.
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Range response for cached token prices.")]
pub struct RangeResponse {
    pub data: Vec<PriceRow>,
}

/// SPEC §14.3.3 — list cached prices in a block range. v1 ships read-only
/// (no lazy-fill); the valuator populates `token_prices_by_block` as events
/// get priced.
#[utoipa::path(
    get,
    path = "/prices/{asset}/{quote}/range",
    tag = "Prices",
    params(
        ("asset" = String, Path, description = "Base asset symbol, typically LPT or ETH."),
        ("quote" = String, Path, description = "Quote asset symbol, currently USD."),
        RangeQuery
    ),
    responses(
        (status = 200, description = "Cached token prices across a block range.", body = RangeResponse),
        (status = 400, description = "Invalid range parameters.", body = crate::error::ErrorEnvelope),
        (status = 500, description = "Unexpected server error.", body = crate::error::ErrorEnvelope)
    )
)]
pub async fn range(
    State(state): State<AppState>,
    Path((asset, quote)): Path<(String, String)>,
    Query(q): Query<RangeQuery>,
) -> Result<Json<RangeResponse>, ApiError> {
    if q.to_block < q.from_block {
        return Err(ApiError::bad_request("to_block < from_block"));
    }
    if q.to_block - q.from_block > 1_000_000 {
        return Err(ApiError::bad_request("range > 1,000,000 blocks; narrow your query"));
    }
    let asset_u = asset.to_uppercase();
    let quote_u = quote.to_uppercase();
    let limit = q.limit.unwrap_or(1000).min(10_000) as i64;
    let mut where_clauses = vec![
        "chain_id = $1".to_string(),
        "asset = $2".to_string(),
        "quote = $3".to_string(),
        "block_number BETWEEN $4 AND $5".to_string(),
    ];
    let mut bind_source: Option<String> = None;
    if let Some(s) = q.source {
        where_clauses.push("source = $6".to_string());
        bind_source = Some(s);
    }
    let sql = format!(
        r#"SELECT chain_id, asset, quote, block_number, block_hash, block_timestamp,
                  price, source, pool_address, oracle_address
             FROM token_prices_by_block
            WHERE {where_clauses}
            ORDER BY block_number ASC, source
            LIMIT {limit}"#,
        where_clauses = where_clauses.join(" AND "),
    );
    let mut q = sqlx::query(&sql)
        .bind(state.chain_id)
        .bind(&asset_u)
        .bind(&quote_u)
        .bind(q.from_block)
        .bind(q.to_block);
    if let Some(s) = bind_source {
        q = q.bind(s);
    }
    let rows = q.fetch_all(&state.pg).await?;
    Ok(Json(RangeResponse {
        data: rows.iter().map(to_price_row).collect(),
    }))
}

fn to_price_row(r: &sqlx::postgres::PgRow) -> PriceRow {
    PriceRow {
        chain_id: r.get::<i64, _>("chain_id").to_string(),
        asset: r.get("asset"),
        quote: r.get("quote"),
        block_number: r.get::<i64, _>("block_number").to_string(),
        block_hash: r.get("block_hash"),
        block_timestamp: r.get("block_timestamp"),
        price: r.get::<BigDecimal, _>("price").to_string(),
        source: r.get("source"),
        pool_address: r.try_get("pool_address").ok(),
        oracle_address: r.try_get("oracle_address").ok(),
    }
}
