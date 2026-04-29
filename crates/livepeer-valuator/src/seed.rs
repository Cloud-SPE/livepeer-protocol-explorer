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
//!
//! ## Bulk implementation (TD-009)
//!
//! Runs as a single `INSERT … SELECT … FROM raw_protocol_events r JOIN
//! seeded_event_prices s …` plus a paired bulk `valuation_attempts` insert
//! for the priced rows. Replaces the prior per-event loop that issued
//! 4-5 round-trips × ~2.3M candidates (≈1h 18m). On the same dataset the
//! bulk path completes in seconds.

use crate::persist::{ARBITRUM_CHAIN_ID, STATUS_PRICED};
use anyhow::Result;
use sqlx::PgPool;
use tracing::info;

const PRICING_METHOD: &str = "seed_lookup";
const SOURCE: &str = "trusted_historical_seed_v1";

#[derive(Debug, Default)]
pub struct SeedRunSummary {
    pub events_considered: u64,
    pub seed_hits: u64,
    pub seed_misses: u64,
    pub priced_this_run: u64,
    pub multi_asset_skipped: u64,
}

/// Walk all unvalued, valuable, canonical events at the given valuation_version
/// and price each via seed lookup. Skips events without seed coverage and
/// multi-asset events. Bulk SQL — single INSERT…SELECT for valuations, paired
/// bulk INSERT for valuation_attempts.
pub async fn run_seed_pass(
    pg: &PgPool,
    valuation_version: &str,
    include_tentative: bool,
) -> Result<SeedRunSummary> {
    let finality_filter = if include_tentative {
        ""
    } else {
        "AND r.finality = 'finalized'"
    };

    // 1. Inventory the candidate set so we can report meaningful summary fields
    //    (events_considered / seed_hits / seed_misses / multi_asset_skipped) the
    //    same way the per-event implementation did.
    let inventory_sql = format!(
        r#"SELECT
              COUNT(*) FILTER (WHERE r.is_valuable AND r.is_canonical {finality_filter_inv})                   AS considered,
              COUNT(*) FILTER (WHERE r.is_valuable AND r.is_canonical AND r.asset IS NULL {finality_filter_inv}) AS multi_asset,
              COUNT(*) FILTER (WHERE r.is_valuable AND r.is_canonical AND r.asset IS NOT NULL
                                AND s.tx_hash IS NOT NULL {finality_filter_inv})                                 AS seed_hits,
              COUNT(*) FILTER (WHERE r.is_valuable AND r.is_canonical AND r.asset IS NOT NULL
                                AND s.tx_hash IS NULL    {finality_filter_inv})                                  AS seed_misses
            FROM raw_protocol_events r
            LEFT JOIN event_valuations v
              ON v.event_id          = r.id
             AND v.valuation_version = $2
             AND (v.asset = r.asset OR (v.asset IS NULL AND r.asset IS NULL))
            LEFT JOIN seeded_event_prices s
              ON s.chain_id = r.chain_id
             AND s.tx_hash  = r.tx_hash
             AND s.asset    = r.asset
            WHERE r.chain_id = $1
              AND v.event_id IS NULL"#,
        finality_filter_inv = finality_filter.replace("r.finality", "r.finality"),
    );
    let row: (i64, i64, i64, i64) = sqlx::query_as(&inventory_sql)
        .bind(ARBITRUM_CHAIN_ID)
        .bind(valuation_version)
        .fetch_one(pg)
        .await?;
    let (considered, multi_asset, seed_hits, seed_misses) = row;

    info!(
        candidates = considered,
        seed_hits, seed_misses, multi_asset_skipped = multi_asset,
        valuation_version, include_tentative,
        "seed-pass starting (bulk)"
    );

    // 2. Bulk INSERT priced rows. Construct pricing_chain JSONB inline; matches
    //    what the per-event loop used to build for the same fields. Idempotent
    //    via ON CONFLICT (event_id, valuation_version, asset).
    let insert_sql = format!(
        r#"INSERT INTO event_valuations
              (event_id, valuation_version, asset, pricing_method,
               chain_id, block_number,
               amount_native, native_usd_price, amount_usd,
               pricing_chain, status, source)
           SELECT
              r.id,
              $2 AS valuation_version,
              r.asset,
              $3 AS pricing_method,
              r.chain_id,
              r.block_number,
              r.amount_normalized,
              s.asset_usd_price,
              r.amount_normalized * s.asset_usd_price,
              jsonb_build_object(
                'steps', jsonb_build_array(jsonb_build_object(
                  'asset',        r.asset,
                  'quote',        'USD',
                  'price',        s.asset_usd_price::text,
                  'source',       $4::text,
                  'block_number', r.block_number,
                  'raw_seed',     s.raw,
                  'note',         'amount_native re-derived from chain per SPEC §8.7 / Q-OD-1; price + amount_usd_authoritative_for_seed_only fields from SQLite seed'
                )),
                'result', jsonb_build_object(
                  'asset',         r.asset,
                  'quote',         'USD',
                  'price',         s.asset_usd_price::text,
                  'amount_native', r.amount_normalized::text,
                  'amount_usd',    (r.amount_normalized * s.asset_usd_price)::text
                ),
                '_seed_amount_usd_for_audit', s.amount_usd::text
              ) AS pricing_chain,
              $5 AS status,
              $4 AS source
            FROM raw_protocol_events r
            JOIN seeded_event_prices s
              ON s.chain_id = r.chain_id
             AND s.tx_hash  = r.tx_hash
             AND s.asset    = r.asset
            LEFT JOIN event_valuations v
              ON v.event_id          = r.id
             AND v.valuation_version = $2
             AND v.asset             = r.asset
            WHERE r.chain_id     = $1
              AND r.is_valuable  = TRUE
              AND r.is_canonical = TRUE
              AND r.asset IS NOT NULL
              AND r.amount_normalized IS NOT NULL
              {finality_filter}
              AND v.event_id IS NULL
           ON CONFLICT (event_id, valuation_version, asset) DO NOTHING"#,
    );
    let result = sqlx::query(&insert_sql)
        .bind(ARBITRUM_CHAIN_ID)
        .bind(valuation_version)
        .bind(PRICING_METHOD)
        .bind(SOURCE)
        .bind(STATUS_PRICED)
        .execute(pg)
        .await?;
    let priced_this_run = result.rows_affected();

    // 3. Bulk INSERT a 'priced' valuation_attempts row for every freshly-priced
    //    event. Only run if rows were actually inserted in step 2 — on a warm
    //    DB (resumed run) the INSERT…SELECT above will have inserted 0 rows
    //    via ON CONFLICT DO NOTHING, and the attempt rows for those events
    //    already exist from the original run. Re-running this CTE on a warm
    //    DB scans 1M+ event_valuations × 1M+ valuation_attempts and adds
    //    nothing useful; it can take minutes on a bloated dataset.
    if priced_this_run > 0 {
        let attempts_sql = r#"
            WITH priced AS (
              SELECT v.event_id, v.valuation_version, v.asset
                FROM event_valuations v
               WHERE v.valuation_version = $2
                 AND v.source            = $3
                 AND v.chain_id          = $1
            ),
            next_n AS (
              SELECT p.event_id, p.valuation_version, p.asset,
                     COALESCE(MAX(va.attempt_number), 0) + 1 AS n
                FROM priced p
                LEFT JOIN valuation_attempts va
                  ON va.event_id          = p.event_id
                 AND va.valuation_version = p.valuation_version
                 AND va.asset             = p.asset
               GROUP BY p.event_id, p.valuation_version, p.asset
            )
            INSERT INTO valuation_attempts
                (event_id, valuation_version, asset, attempt_number, result_status, error_detail)
            SELECT event_id, valuation_version, asset, n, $4, NULL
              FROM next_n
            ON CONFLICT (event_id, valuation_version, asset, attempt_number) DO NOTHING"#;
        sqlx::query(attempts_sql)
            .bind(ARBITRUM_CHAIN_ID)
            .bind(valuation_version)
            .bind(SOURCE)
            .bind(STATUS_PRICED)
            .execute(pg)
            .await?;
    }

    let summary = SeedRunSummary {
        events_considered: considered as u64,
        seed_hits: seed_hits as u64,
        seed_misses: seed_misses as u64,
        priced_this_run,
        multi_asset_skipped: multi_asset as u64,
    };
    info!(?summary, "seed-pass complete (bulk)");
    Ok(summary)
}
