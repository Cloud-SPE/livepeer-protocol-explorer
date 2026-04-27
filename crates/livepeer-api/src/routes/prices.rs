//! Prices endpoints. SPEC §14.3.3.
//!
//! Backed by `token_prices_by_block` (populated by the valuator's on-chain reads).
//! v1 ships `/prices/{asset}/{quote}/block/{block}` and `/prices/{asset}/{quote}/latest`.
//! `/prices/.../range` lazy-fill is a v1.5 follow-up — needs the on-chain reader to
//! be split out from the valuator pass and run on demand.

use crate::{error::ApiError, state::AppState};
use axum::{extract::{Path, State}, Json};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::Row;

#[derive(Debug, Serialize)]
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
