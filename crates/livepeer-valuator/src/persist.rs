//! Shared persistence helpers used by the seed-hit (S8.1) and on-chain (S8.2) paths.

use anyhow::Result;
use bigdecimal::BigDecimal;
use sqlx::{PgPool, Postgres, Row, Transaction};
use tracing::error;

pub const ARBITRUM_CHAIN_ID: i64 = 42161;
pub const STATUS_PRICED: &str = "priced";
pub const STATUS_PRICED_WITH_WARNING: &str = "priced_with_warning";
pub const STATUS_FAILED_MISSING_ORACLE: &str = "failed_missing_oracle";
pub const STATUS_FAILED_MISSING_POOL: &str = "failed_missing_pool";
pub const STATUS_FAILED_SEQUENCER_OUTAGE: &str = "failed_sequencer_outage";
pub const STATUS_DETERMINISM_VIOLATION: &str = "failed_determinism_violation";

/// Idempotent UPSERT into `token_prices_by_block`. PK is
/// `(chain_id, asset, quote, block_number, source)`. ON CONFLICT DO NOTHING.
/// SPEC §11.6 — populated by the valuator's on-chain reads so the `/prices` API
/// can serve the same data the valuator computed.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_price(
    pool: &sqlx::PgPool,
    asset: &str,
    quote: &str,
    block_number: i64,
    block_hash: &str,
    block_timestamp: chrono::DateTime<chrono::Utc>,
    price: &BigDecimal,
    source: &str,
    pool_address: Option<&str>,
    oracle_address: Option<&str>,
    raw: Option<&serde_json::Value>,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO token_prices_by_block
              (chain_id, asset, quote, block_number, block_hash, block_timestamp,
               price, source, pool_address, oracle_address, raw)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
           ON CONFLICT (chain_id, asset, quote, block_number, source) DO NOTHING"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(asset)
    .bind(quote)
    .bind(block_number)
    .bind(block_hash)
    .bind(block_timestamp)
    .bind(price)
    .bind(source)
    .bind(pool_address)
    .bind(oracle_address)
    .bind(raw)
    .execute(pool)
    .await?;
    Ok(())
}

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
    native_usd_price: Option<&BigDecimal>,
    amount_usd: Option<&BigDecimal>,
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

/// Determinism guard per SPEC §10.5. If `event_valuations` already has a row at
/// `(event_id, valuation_version, asset)` and the stored numeric values DIFFER
/// from what we just computed, that's a violation:
///
///   1. Log CRITICAL.
///   2. Insert a `valuation_attempts` row with `result_status =
///      'failed_determinism_violation'` and a JSON diff in `error_detail`.
///   3. Return `Ok(false)` (no insert; preserves the existing row).
///
/// If the row matches, this is just a redundant computation — log debug and skip.
/// If the row doesn't exist yet, INSERT and return `Ok(true)`.
///
/// The four numeric columns + status + source + pricing_method are compared.
/// `pricing_chain` JSONB is informational and not part of the determinism check —
/// formatting differences (key order, whitespace) would create false positives.
#[allow(clippy::too_many_arguments)]
pub async fn insert_valuation_checked(
    pg: &PgPool,
    event_id: i64,
    valuation_version: &str,
    asset: &str,
    pricing_method: &str,
    source: &str,
    status: &str,
    block_number: i64,
    amount_native: &BigDecimal,
    native_usd_price: Option<&BigDecimal>,
    amount_usd: Option<&BigDecimal>,
    pricing_chain: &serde_json::Value,
) -> Result<DeterminismOutcome> {
    let existing: Option<StoredValuation> = sqlx::query(
        r#"SELECT pricing_method, source, status,
                  amount_native, native_usd_price, amount_usd
             FROM event_valuations
            WHERE event_id = $1 AND valuation_version = $2 AND asset = $3"#,
    )
    .bind(event_id)
    .bind(valuation_version)
    .bind(asset)
    .fetch_optional(pg)
    .await?
    .map(|r| StoredValuation {
        pricing_method: r.get(0),
        source: r.get(1),
        status: r.get(2),
        amount_native: r.get(3),
        native_usd_price: r.get(4),
        amount_usd: r.get(5),
    });

    match existing {
        Some(stored) => {
            let diff = stored.diff(
                pricing_method,
                source,
                status,
                amount_native,
                native_usd_price,
                amount_usd,
            );
            if diff.is_null() {
                return Ok(DeterminismOutcome::Idempotent);
            }
            error!(
                event_id,
                valuation_version,
                asset,
                diff = %diff,
                "DETERMINISM VIOLATION — recomputed values differ from stored \
                 (SPEC §10.5). Stored row is preserved; attempt logged."
            );
            let mut tx = pg.begin().await?;
            insert_attempt(
                &mut tx,
                event_id,
                valuation_version,
                asset,
                STATUS_DETERMINISM_VIOLATION,
                Some(diff.clone()),
            )
            .await?;
            tx.commit().await?;
            Ok(DeterminismOutcome::Violation { diff })
        }
        None => {
            let mut tx = pg.begin().await?;
            insert_valuation(
                &mut tx,
                event_id,
                valuation_version,
                asset,
                pricing_method,
                source,
                status,
                block_number,
                amount_native,
                native_usd_price,
                amount_usd,
                pricing_chain,
            )
            .await?;
            insert_attempt(&mut tx, event_id, valuation_version, asset, status, None).await?;
            tx.commit().await?;
            Ok(DeterminismOutcome::Inserted)
        }
    }
}

#[derive(Debug)]
pub enum DeterminismOutcome {
    Inserted,
    Idempotent,
    Violation { diff: serde_json::Value },
}

#[derive(Debug)]
struct StoredValuation {
    pricing_method: String,
    source: String,
    status: String,
    amount_native: BigDecimal,
    native_usd_price: Option<BigDecimal>,
    amount_usd: Option<BigDecimal>,
}

impl StoredValuation {
    fn diff(
        &self,
        pricing_method: &str,
        source: &str,
        status: &str,
        amount_native: &BigDecimal,
        native_usd_price: Option<&BigDecimal>,
        amount_usd: Option<&BigDecimal>,
    ) -> serde_json::Value {
        let mut entries = serde_json::Map::new();
        if self.pricing_method != pricing_method {
            entries.insert(
                "pricing_method".to_string(),
                serde_json::json!({ "stored": self.pricing_method, "recomputed": pricing_method }),
            );
        }
        if self.source != source {
            entries.insert(
                "source".to_string(),
                serde_json::json!({ "stored": self.source, "recomputed": source }),
            );
        }
        if self.status != status {
            entries.insert(
                "status".to_string(),
                serde_json::json!({ "stored": self.status, "recomputed": status }),
            );
        }
        if &self.amount_native != amount_native {
            entries.insert(
                "amount_native".to_string(),
                serde_json::json!({
                    "stored": self.amount_native.to_string(),
                    "recomputed": amount_native.to_string(),
                }),
            );
        }
        if self.native_usd_price.as_ref() != native_usd_price {
            entries.insert(
                "native_usd_price".to_string(),
                serde_json::json!({
                    "stored": self.native_usd_price.as_ref().map(ToString::to_string),
                    "recomputed": native_usd_price.map(ToString::to_string),
                }),
            );
        }
        if self.amount_usd.as_ref() != amount_usd {
            entries.insert(
                "amount_usd".to_string(),
                serde_json::json!({
                    "stored": self.amount_usd.as_ref().map(ToString::to_string),
                    "recomputed": amount_usd.map(ToString::to_string),
                }),
            );
        }
        if entries.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::Object(entries)
        }
    }
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

#[cfg(test)]
mod determinism_tests {
    use super::*;
    use bigdecimal::BigDecimal;
    use std::str::FromStr;

    fn stored() -> StoredValuation {
        StoredValuation {
            pricing_method: "seed_lookup".to_string(),
            source: "trusted_historical_seed_v1".to_string(),
            status: "priced".to_string(),
            amount_native: BigDecimal::from_str("18.199267584391068228").unwrap(),
            native_usd_price: Some(BigDecimal::from_str("2.191").unwrap()),
            amount_usd: Some(BigDecimal::from_str("39.882633471224994").unwrap()),
        }
    }

    #[test]
    fn diff_is_null_when_all_match() {
        let s = stored();
        let d = s.diff(
            &s.pricing_method,
            &s.source,
            &s.status,
            &s.amount_native,
            s.native_usd_price.as_ref(),
            s.amount_usd.as_ref(),
        );
        assert!(d.is_null(), "expected no diff, got: {d}");
    }

    #[test]
    fn diff_reports_amount_usd_mismatch() {
        let s = stored();
        let new_amount_usd = BigDecimal::from_str("999.99").unwrap();
        let d = s.diff(
            &s.pricing_method,
            &s.source,
            &s.status,
            &s.amount_native,
            s.native_usd_price.as_ref(),
            Some(&new_amount_usd),
        );
        let obj = d.as_object().expect("diff is an object");
        assert!(obj.contains_key("amount_usd"));
        assert_eq!(obj.len(), 1, "only amount_usd should differ");
    }

    #[test]
    fn diff_reports_multiple_mismatches() {
        let s = stored();
        let other_method = "uniswap_v3_twap_30min_x_chainlink_eth";
        let other_source = "uniswap_v3_dual_rpc";
        let new_price = BigDecimal::from_str("2.5").unwrap();
        let d = s.diff(
            other_method,
            other_source,
            &s.status,
            &s.amount_native,
            Some(&new_price),
            s.amount_usd.as_ref(),
        );
        let obj = d.as_object().expect("diff is an object");
        assert!(obj.contains_key("pricing_method"));
        assert!(obj.contains_key("source"));
        assert!(obj.contains_key("native_usd_price"));
        assert_eq!(obj.len(), 3);
    }
}
