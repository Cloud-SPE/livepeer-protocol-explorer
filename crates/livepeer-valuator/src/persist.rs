//! Shared persistence helpers used by the seed-hit (S8.1) and on-chain (S8.2) paths.

use anyhow::Result;
use bigdecimal::BigDecimal;
use sqlx::{Postgres, Transaction};

pub const ARBITRUM_CHAIN_ID: i64 = 42161;
pub const STATUS_PRICED: &str = "priced";

#[allow(clippy::too_many_arguments)]
pub async fn insert_valuation(
    tx: &mut Transaction<'_, Postgres>,
    event_id: i64,
    valuation_version: &str,
    asset: &str,
    pricing_method: &str,
    source: &str,
    status: &str,
    block_number: i64,
    amount_native: &BigDecimal,
    native_usd_price: &BigDecimal,
    amount_usd: &BigDecimal,
    pricing_chain: &serde_json::Value,
) -> Result<bool> {
    let result = sqlx::query(
        r#"INSERT INTO event_valuations
              (event_id, valuation_version, asset, pricing_method,
               chain_id, block_number,
               amount_native, native_usd_price, amount_usd,
               pricing_chain, status, source)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
           ON CONFLICT (event_id, valuation_version, asset) DO NOTHING"#,
    )
    .bind(event_id)
    .bind(valuation_version)
    .bind(asset)
    .bind(pricing_method)
    .bind(ARBITRUM_CHAIN_ID)
    .bind(block_number)
    .bind(amount_native)
    .bind(native_usd_price)
    .bind(amount_usd)
    .bind(pricing_chain)
    .bind(status)
    .bind(source)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn insert_attempt(
    tx: &mut Transaction<'_, Postgres>,
    event_id: i64,
    valuation_version: &str,
    asset: &str,
    result_status: &str,
    error_detail: Option<serde_json::Value>,
) -> Result<()> {
    sqlx::query(
        r#"WITH next_n AS (
              SELECT COALESCE(MAX(attempt_number), 0) + 1 AS n
                FROM valuation_attempts
               WHERE event_id          = $1
                 AND valuation_version = $2
                 AND asset             = $3
            )
            INSERT INTO valuation_attempts
                (event_id, valuation_version, asset, attempt_number,
                 result_status, error_detail)
            SELECT $1, $2, $3, n, $4, $5 FROM next_n
            ON CONFLICT (event_id, valuation_version, asset, attempt_number) DO NOTHING"#,
    )
    .bind(event_id)
    .bind(valuation_version)
    .bind(asset)
    .bind(result_status)
    .bind(error_detail)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
