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
const SEED_BATCH_SIZE: i64 = 100_000;

#[derive(Debug, Default)]
pub struct SeedRunSummary {
    pub events_considered: u64,
    pub seed_hits: u64,
    pub seed_misses: u64,
    pub priced_this_run: u64,
    pub multi_asset_skipped: u64,
}

async fn has_seed_candidates(
    pg: &PgPool,
    valuation_version: &str,
    include_tentative: bool,
) -> Result<bool> {
    let finality_filter = if include_tentative {
        ""
    } else {
        "AND r.finality = 'finalized'"
    };
    let exists_sql = format!(
        r#"SELECT EXISTS (
             SELECT 1
               FROM raw_protocol_events r
              WHERE r.chain_id = $1
                AND r.is_valuable = TRUE
                AND r.is_canonical = TRUE
                AND r.asset IS NOT NULL
                AND r.amount_normalized IS NOT NULL
                {finality_filter}
                AND EXISTS (
                      SELECT 1
                        FROM seeded_event_prices s
                       WHERE s.chain_id = r.chain_id
                         AND s.tx_hash = r.tx_hash
                         AND s.asset = r.asset
                  )
                AND NOT EXISTS (
                      SELECT 1
                        FROM event_valuations v
                       WHERE v.event_id = r.id
                         AND v.valuation_version = $2
                         AND v.asset = r.asset
                  )
              LIMIT 1
           )"#,
    );
    let exists = sqlx::query_scalar::<_, bool>(&exists_sql)
        .bind(ARBITRUM_CHAIN_ID)
        .bind(valuation_version)
        .fetch_one(pg)
        .await?;
    Ok(exists)
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
    info!(
        valuation_version,
        include_tentative, "seed-pass starting (bulk)"
    );

    // Change-detector gate. Seed candidacy only changes when new seeds are
    // imported (seeds are historical; live events are never seeded), so skip
    // the expensive candidate scan when max(seeded_event_prices.id) is unchanged
    // since the last seed run. Bypassed when cold (no valuations for this
    // version — post-truncate/replay) or include_tentative. Break-glass: delete
    // the SEED cursor row (or `--reset-seed-cursor`) to force a full re-scan.
    // seeded_event_prices is append-only (ON CONFLICT DO NOTHING) and has no
    // serial id, so its row count is a cheap, monotonic change signal: it rises
    // exactly when new seeds are imported.
    let seed_key = crate::cursor::pass_key(valuation_version, "SEED");
    let cur_seed_count: i64 = sqlx::query_scalar("SELECT count(*) FROM seeded_event_prices")
        .fetch_one(pg)
        .await?;
    if !include_tentative {
        let stored: Option<i64> = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT seed_max_id FROM valuator_cursors WHERE pass_key = $1",
        )
        .bind(&seed_key)
        .fetch_optional(pg)
        .await?
        .flatten();
        let has_valuations: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM event_valuations WHERE valuation_version = $1)",
        )
        .bind(valuation_version)
        .fetch_one(pg)
        .await?;
        if has_valuations && stored == Some(cur_seed_count) {
            let summary = SeedRunSummary::default();
            info!(?summary, "seed-pass skipped (no new seeds imported)");
            return Ok(summary);
        }
    }

    if !has_seed_candidates(pg, valuation_version, include_tentative).await? {
        let summary = SeedRunSummary::default();
        info!(?summary, "seed-pass skipped (no candidates)");
        return Ok(summary);
    }

    // 1. Bulk INSERT priced rows in deterministic chunks so the cold path can
    //    make visible progress instead of holding one giant transaction open.
    //    The candidate ordering is stable across replays.
    let batch_sql = format!(
        r#"WITH candidate AS (
               SELECT
                  r.id,
                  r.asset,
                  r.chain_id,
                  r.block_number,
                  r.amount_normalized,
                  s.asset_usd_price,
                  s.raw,
                  s.amount_usd
                FROM raw_protocol_events r
                JOIN seeded_event_prices s
                  ON s.chain_id = r.chain_id
                 AND s.tx_hash  = r.tx_hash
                 AND s.asset    = r.asset
               WHERE r.chain_id     = $1
                 AND r.is_valuable  = TRUE
                 AND r.is_canonical = TRUE
                 AND r.asset IS NOT NULL
                 AND r.amount_normalized IS NOT NULL
                 {finality_filter}
                 AND NOT EXISTS (
                       SELECT 1
                         FROM event_valuations v
                        WHERE v.event_id          = r.id
                          AND v.valuation_version = $2
                          AND v.asset             = r.asset
                   )
               ORDER BY r.block_number, r.log_index, r.id
               LIMIT $6
           ),
           inserted AS (
               INSERT INTO event_valuations
                   (event_id, valuation_version, asset, pricing_method,
                    chain_id, block_number,
                    amount_native, native_usd_price, amount_usd,
                    pricing_chain, status, source)
               SELECT
                  c.id,
                  $2 AS valuation_version,
                  c.asset,
                  $3 AS pricing_method,
                  c.chain_id,
                  c.block_number,
                  c.amount_normalized,
                  c.asset_usd_price,
                  c.amount_normalized * c.asset_usd_price,
                  jsonb_build_object(
                    'steps', jsonb_build_array(jsonb_build_object(
                      'asset',        c.asset,
                      'quote',        'USD',
                      'price',        c.asset_usd_price::text,
                      'source',       $4::text,
                      'block_number', c.block_number,
                      'raw_seed',     c.raw,
                      'note',         'amount_native re-derived from chain per SPEC §8.7 / Q-OD-1; price + amount_usd_authoritative_for_seed_only fields from SQLite seed'
                    )),
                    'result', jsonb_build_object(
                      'asset',         c.asset,
                      'quote',         'USD',
                      'price',         c.asset_usd_price::text,
                      'amount_native', c.amount_normalized::text,
                      'amount_usd',    (c.amount_normalized * c.asset_usd_price)::text
                    ),
                    '_seed_amount_usd_for_audit', c.amount_usd::text
                  ) AS pricing_chain,
                  $5 AS status,
                  $4 AS source
                 FROM candidate c
                ON CONFLICT (event_id, valuation_version, asset) DO NOTHING
                RETURNING event_id, valuation_version, asset
           ),
           next_n AS (
               SELECT i.event_id, i.valuation_version, i.asset,
                      COALESCE(MAX(va.attempt_number), 0) + 1 AS n
                 FROM inserted i
                 LEFT JOIN valuation_attempts va
                   ON va.event_id          = i.event_id
                  AND va.valuation_version = i.valuation_version
                  AND va.asset             = i.asset
                GROUP BY i.event_id, i.valuation_version, i.asset
           ),
           attempts AS (
               INSERT INTO valuation_attempts
               (event_id, valuation_version, asset, attempt_number, result_status, error_detail)
               SELECT event_id, valuation_version, asset, n, $5, NULL
                 FROM next_n
               ON CONFLICT (event_id, valuation_version, asset, attempt_number) DO NOTHING
           )
           SELECT COUNT(*)::bigint FROM inserted"#,
    );

    let mut priced_this_run = 0u64;
    loop {
        let inserted_this_batch: i64 = sqlx::query_scalar(&batch_sql)
            .bind(ARBITRUM_CHAIN_ID)
            .bind(valuation_version)
            .bind(PRICING_METHOD)
            .bind(SOURCE)
            .bind(STATUS_PRICED)
            .bind(SEED_BATCH_SIZE)
            .fetch_one(pg)
            .await?;
        if inserted_this_batch == 0 {
            break;
        }
        priced_this_run += inserted_this_batch as u64;
        info!(
            valuation_version,
            batch_rows = inserted_this_batch,
            priced_this_run,
            "seed-pass chunk committed"
        );
    }

    // Detailed seed inventory is intentionally omitted from the critical path.
    // On full-history datasets the upfront summary query was taking tens of
    // seconds before the pass did any real work. The operationally useful
    // signal is how many rows we priced this run.
    // Record the seed change-detector marker for the next cycle.
    if !include_tentative {
        sqlx::query(
            "INSERT INTO valuator_cursors (pass_key, watermark, seed_max_id, updated_at)
                 VALUES ($1, now(), $2, now())
             ON CONFLICT (pass_key)
                 DO UPDATE SET watermark = now(), seed_max_id = EXCLUDED.seed_max_id, updated_at = now()",
        )
        .bind(&seed_key)
        .bind(cur_seed_count)
        .execute(pg)
        .await?;
    }

    let summary = SeedRunSummary {
        events_considered: 0,
        seed_hits: 0,
        seed_misses: 0,
        priced_this_run,
        multi_asset_skipped: 0,
    };
    info!(?summary, "seed-pass complete (bulk)");
    Ok(summary)
}
