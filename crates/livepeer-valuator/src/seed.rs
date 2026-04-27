//! Seed-hit valuation path. SPEC §8.5 / §11.7.
//!
//! For every unvalued event with `asset` ∈ {LPT, ETH}, look up
//! `seeded_event_prices` by `(chain_id, tx_hash, asset)`. On hit:
//!
//!   amount_native       = chain's amount_normalized (re-derived per Q-OD-1)
//!   native_usd_price    = seed.asset_usd_price
//!   amount_usd          = amount_native × native_usd_price
//!   pricing_method      = 'seed_lookup'
//!   source              = 'trusted_historical_seed_v1'
//!   pricing_chain JSONB = single-step provenance referencing the seed row
//!
//! Idempotent via `ON CONFLICT (event_id, valuation_version, asset) DO NOTHING`.
//! Multi-asset events (`asset IS NULL`, e.g. EarningsClaimed) are skipped here —
//! S8.3 handles them.

use anyhow::{Context, Result};
use bigdecimal::BigDecimal;
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::str::FromStr;
use tracing::{debug, info};

const ARBITRUM_CHAIN_ID: i64 = 42161;
const PRICING_METHOD: &str = "seed_lookup";
const SOURCE: &str = "trusted_historical_seed_v1";
const STATUS: &str = "priced";

#[derive(Debug, Default)]
pub struct SeedRunSummary {
    pub events_considered: u64,
    pub seed_hits: u64,
    pub seed_misses: u64,
    pub priced_this_run: u64,
    pub multi_asset_skipped: u64,
}

#[derive(Debug)]
struct CandidateEvent {
    event_id: i64,
    block_number: i64,
    tx_hash: String,
    asset: Option<String>,
    amount_normalized: Option<BigDecimal>,
}

#[derive(Debug)]
struct SeedRow {
    amount_usd: BigDecimal,
    asset_usd_price: BigDecimal,
    raw: serde_json::Value,
}

/// Walk all unvalued, valuable, canonical events at the given valuation_version
/// and price each via seed lookup. Skips events without seed coverage and
/// multi-asset events.
pub async fn run_seed_pass(
    pg: &PgPool,
    valuation_version: &str,
    include_tentative: bool,
) -> Result<SeedRunSummary> {
    let candidates = fetch_candidates(pg, valuation_version, include_tentative).await?;
    info!(
        candidates = candidates.len(),
        valuation_version, include_tentative, "seed-pass starting"
    );

    let mut summary = SeedRunSummary {
        events_considered: candidates.len() as u64,
        ..Default::default()
    };

    for ev in &candidates {
        let Some(asset) = &ev.asset else {
            summary.multi_asset_skipped += 1;
            continue;
        };

        let seed = lookup_seed(pg, &ev.tx_hash, asset).await?;
        let Some(seed) = seed else {
            summary.seed_misses += 1;
            continue;
        };
        summary.seed_hits += 1;

        let amount_native = ev
            .amount_normalized
            .clone()
            .context("event has NULL amount_normalized but is asset-tagged")?;
        let amount_usd = &amount_native * &seed.asset_usd_price;
        let pricing_chain = serde_json::json!({
            "steps": [{
                "asset": asset,
                "quote": "USD",
                "price": seed.asset_usd_price.to_string(),
                "source": SOURCE,
                "block_number": ev.block_number,
                "raw_seed": seed.raw,
                "note": "amount_native re-derived from chain per SPEC §8.7 / Q-OD-1; price + amount_usd_authoritative_for_seed_only fields from SQLite seed",
            }],
            "result": {
                "asset": asset,
                "quote": "USD",
                "price": seed.asset_usd_price.to_string(),
                "amount_native": amount_native.to_string(),
                "amount_usd": amount_usd.to_string(),
            },
            "_seed_amount_usd_for_audit": seed.amount_usd.to_string(),
        });

        let mut tx = pg.begin().await?;
        let inserted = insert_valuation(
            &mut tx,
            ev.event_id,
            valuation_version,
            asset,
            ev.block_number,
            &amount_native,
            &seed.asset_usd_price,
            &amount_usd,
            &pricing_chain,
        )
        .await?;
        // Always record an attempt — useful for retries + observability.
        insert_attempt(
            &mut tx,
            ev.event_id,
            valuation_version,
            asset,
            STATUS,
            None,
        )
        .await?;
        tx.commit().await?;

        if inserted {
            summary.priced_this_run += 1;
            debug!(
                event_id = ev.event_id,
                asset = %asset,
                amount_native = %amount_native,
                amount_usd = %amount_usd,
                "priced via seed"
            );
        }
    }

    info!(?summary, "seed-pass complete");
    Ok(summary)
}

async fn fetch_candidates(
    pg: &PgPool,
    valuation_version: &str,
    include_tentative: bool,
) -> Result<Vec<CandidateEvent>> {
    // Per SPEC §9.1 the valuator only consumes finality='finalized' rows. Without a
    // finality watcher running yet, all rows are 'tentative' — the override flag
    // exists for development end-to-end testing.
    let finality_filter = if include_tentative {
        "" // accept any finality
    } else {
        "AND r.finality = 'finalized'"
    };
    let sql = format!(
        r#"SELECT r.id, r.block_number, r.tx_hash, r.asset, r.amount_normalized
             FROM raw_protocol_events r
             LEFT JOIN event_valuations v
               ON v.event_id          = r.id
              AND v.valuation_version = $2
              AND (v.asset = r.asset OR (v.asset IS NULL AND r.asset IS NULL))
            WHERE r.chain_id      = $1
              AND r.is_valuable   = TRUE
              AND r.is_canonical  = TRUE
              {finality_filter}
              AND v.event_id IS NULL
            ORDER BY r.block_number, r.log_index"#
    );
    let rows = sqlx::query(&sql)
        .bind(ARBITRUM_CHAIN_ID)
        .bind(valuation_version)
        .fetch_all(pg)
        .await?;

    Ok(rows
        .into_iter()
        .map(|r| CandidateEvent {
            event_id: r.get(0),
            block_number: r.get(1),
            tx_hash: r.get(2),
            asset: r.get(3),
            amount_normalized: r.get(4),
        })
        .collect())
}

async fn lookup_seed(pg: &PgPool, tx_hash: &str, asset: &str) -> Result<Option<SeedRow>> {
    let row = sqlx::query(
        r#"SELECT amount_usd, asset_usd_price, raw
             FROM seeded_event_prices
            WHERE chain_id = $1 AND tx_hash = $2 AND asset = $3
            LIMIT 1"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(tx_hash.to_lowercase())
    .bind(asset)
    .fetch_optional(pg)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(SeedRow {
        amount_usd: row.get::<BigDecimal, _>(0),
        asset_usd_price: row.get::<BigDecimal, _>(1),
        raw: row.try_get(2).unwrap_or(serde_json::Value::Null),
    }))
}

#[allow(clippy::too_many_arguments)]
async fn insert_valuation(
    tx: &mut Transaction<'_, Postgres>,
    event_id: i64,
    valuation_version: &str,
    asset: &str,
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
    .bind(PRICING_METHOD)
    .bind(ARBITRUM_CHAIN_ID)
    .bind(block_number)
    .bind(amount_native)
    .bind(native_usd_price)
    .bind(amount_usd)
    .bind(pricing_chain)
    .bind(STATUS)
    .bind(SOURCE)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected() > 0)
}

async fn insert_attempt(
    tx: &mut Transaction<'_, Postgres>,
    event_id: i64,
    valuation_version: &str,
    asset: &str,
    result_status: &str,
    error_detail: Option<serde_json::Value>,
) -> Result<()> {
    // attempt_number = next per (event_id, version, asset). Use a CTE to compute it.
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

// Strip warnings for the unused FromStr import in some build configs.
#[allow(dead_code)]
fn _bd(s: &str) -> Option<BigDecimal> {
    BigDecimal::from_str(s).ok()
}
