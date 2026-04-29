//! Bulk-insert buffers for the on-chain valuation passes (TD-009).
//!
//! The per-event commit pattern in `onchain.rs` and `multi_asset.rs` issues
//! 5+ Postgres round-trips per priced event (BEGIN, insert_valuation,
//! insert_attempt's MAX(attempt_number)+1 CTE, COMMIT, plus 1-2 upsert_price
//! statements). At ~10ms per round-trip on a 1.1M-row LPT pass that's hours
//! of wall-clock just on RTT.
//!
//! `BulkBuffers` accumulates rows during the loop and flushes via
//! `QueryBuilder.push_values` (multi-row INSERT). One flush per ~500 events
//! takes the per-event RTT cost from milliseconds down to microseconds.
//!
//! Callers must invoke `flush()` periodically (every N events) and one final
//! time at the end of the pass to drain residual rows.

use anyhow::Result;
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, QueryBuilder};

const FLUSH_THRESHOLD: usize = 500;

#[derive(Debug, Clone)]
pub struct ValuationRow {
    pub event_id: i64,
    pub valuation_version: String,
    pub asset: String,
    pub pricing_method: String,
    pub source: String,
    pub status: String,
    pub chain_id: i64,
    pub block_number: i64,
    pub amount_native: BigDecimal,
    pub native_usd_price: BigDecimal,
    pub amount_usd: BigDecimal,
    pub pricing_chain: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct AttemptRow {
    pub event_id: i64,
    pub valuation_version: String,
    pub asset: String,
    pub result_status: String,
    pub error_detail: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct PriceRow {
    pub chain_id: i64,
    pub asset: String,
    pub quote: String,
    pub block_number: i64,
    pub block_hash: String,
    pub block_timestamp: DateTime<Utc>,
    pub price: BigDecimal,
    pub source: String,
    pub pool_address: Option<String>,
    pub oracle_address: Option<String>,
    pub raw: Option<serde_json::Value>,
}

#[derive(Debug, Default)]
pub struct BulkBuffers {
    valuations: Vec<ValuationRow>,
    attempts: Vec<AttemptRow>,
    prices: Vec<PriceRow>,
}

impl BulkBuffers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_valuation(&mut self, v: ValuationRow) {
        self.valuations.push(v);
    }

    pub fn push_attempt(&mut self, a: AttemptRow) {
        self.attempts.push(a);
    }

    pub fn push_price(&mut self, p: PriceRow) {
        self.prices.push(p);
    }

    /// Convenience: enqueue a priced valuation row + its paired
    /// `valuation_attempts` row (status='priced'). Mirrors the
    /// original per-event `commit_priced` semantics.
    #[allow(clippy::too_many_arguments)]
    pub fn push_priced(
        &mut self,
        chain_id: i64,
        event_id: i64,
        valuation_version: &str,
        asset: &str,
        pricing_method: &str,
        source: &str,
        block_number: i64,
        amount_native: &BigDecimal,
        native_usd_price: &BigDecimal,
        amount_usd: &BigDecimal,
        pricing_chain: &serde_json::Value,
        status: &str,
    ) {
        self.valuations.push(ValuationRow {
            event_id,
            valuation_version: valuation_version.to_string(),
            asset: asset.to_string(),
            pricing_method: pricing_method.to_string(),
            source: source.to_string(),
            status: status.to_string(),
            chain_id,
            block_number,
            amount_native: amount_native.clone(),
            native_usd_price: native_usd_price.clone(),
            amount_usd: amount_usd.clone(),
            pricing_chain: pricing_chain.clone(),
        });
        self.attempts.push(AttemptRow {
            event_id,
            valuation_version: valuation_version.to_string(),
            asset: asset.to_string(),
            result_status: status.to_string(),
            error_detail: None,
        });
    }

    /// Convenience: enqueue a failed-attempt-only row (no valuation written).
    /// Used for `failed_sequencer_outage`, `failed_missing_oracle`,
    /// `failed_missing_pool` outcomes.
    pub fn push_failed_attempt(
        &mut self,
        event_id: i64,
        valuation_version: &str,
        asset: &str,
        result_status: &str,
        error_detail: Option<serde_json::Value>,
    ) {
        self.attempts.push(AttemptRow {
            event_id,
            valuation_version: valuation_version.to_string(),
            asset: asset.to_string(),
            result_status: result_status.to_string(),
            error_detail,
        });
    }

    pub fn pending(&self) -> usize {
        self.valuations.len() + self.attempts.len() + self.prices.len()
    }

    /// Flush if buffers exceed `FLUSH_THRESHOLD` items in any one bucket.
    pub async fn maybe_flush(&mut self, pg: &PgPool) -> Result<()> {
        if self.valuations.len() >= FLUSH_THRESHOLD
            || self.attempts.len() >= FLUSH_THRESHOLD
            || self.prices.len() >= FLUSH_THRESHOLD
        {
            self.flush(pg).await?;
        }
        Ok(())
    }

    /// Drain all buffers via bulk INSERTs.
    pub async fn flush(&mut self, pg: &PgPool) -> Result<()> {
        if !self.valuations.is_empty() {
            flush_valuations(pg, std::mem::take(&mut self.valuations)).await?;
        }
        if !self.prices.is_empty() {
            flush_prices(pg, std::mem::take(&mut self.prices)).await?;
        }
        // Attempts depend on the current MAX(attempt_number) per (event, version,
        // asset) so the bulk INSERT uses a CTE — see flush_attempts.
        if !self.attempts.is_empty() {
            flush_attempts(pg, std::mem::take(&mut self.attempts)).await?;
        }
        Ok(())
    }
}

async fn flush_valuations(pg: &PgPool, rows: Vec<ValuationRow>) -> Result<()> {
    for chunk in rows.chunks(FLUSH_THRESHOLD) {
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO event_valuations \
             (event_id, valuation_version, asset, pricing_method, \
              chain_id, block_number, amount_native, native_usd_price, \
              amount_usd, pricing_chain, status, source) ",
        );
        qb.push_values(chunk.iter(), |mut b, r| {
            b.push_bind(r.event_id);
            b.push_bind(&r.valuation_version);
            b.push_bind(&r.asset);
            b.push_bind(&r.pricing_method);
            b.push_bind(r.chain_id);
            b.push_bind(r.block_number);
            b.push_bind(&r.amount_native);
            b.push_bind(&r.native_usd_price);
            b.push_bind(&r.amount_usd);
            b.push_bind(&r.pricing_chain);
            b.push_bind(&r.status);
            b.push_bind(&r.source);
        });
        qb.push(" ON CONFLICT (event_id, valuation_version, asset) DO NOTHING");
        qb.build().execute(pg).await?;
    }
    Ok(())
}

async fn flush_prices(pg: &PgPool, rows: Vec<PriceRow>) -> Result<()> {
    for chunk in rows.chunks(FLUSH_THRESHOLD) {
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "INSERT INTO token_prices_by_block \
             (chain_id, asset, quote, block_number, block_hash, block_timestamp, \
              price, source, pool_address, oracle_address, raw) ",
        );
        qb.push_values(chunk.iter(), |mut b, r| {
            b.push_bind(r.chain_id);
            b.push_bind(&r.asset);
            b.push_bind(&r.quote);
            b.push_bind(r.block_number);
            b.push_bind(&r.block_hash);
            b.push_bind(r.block_timestamp);
            b.push_bind(&r.price);
            b.push_bind(&r.source);
            b.push_bind(&r.pool_address);
            b.push_bind(&r.oracle_address);
            b.push_bind(&r.raw);
        });
        qb.push(" ON CONFLICT (chain_id, asset, quote, block_number, source) DO NOTHING");
        qb.build().execute(pg).await?;
    }
    Ok(())
}

/// Bulk insert valuation_attempts. attempt_number is per
/// (event_id, valuation_version, asset) and is computed inside the SQL via a
/// CTE that joins each new attempt row against the existing max.
async fn flush_attempts(pg: &PgPool, rows: Vec<AttemptRow>) -> Result<()> {
    for chunk in rows.chunks(FLUSH_THRESHOLD) {
        // Build a VALUES list (event_id, version, asset, status, detail) and
        // join it against the existing max for each key. Within a single batch,
        // duplicate keys are deduped by ROW_NUMBER so each (event, version,
        // asset) triple contributes one new row at MAX+1.
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "WITH input AS ( \
               SELECT * FROM ( ",
        );
        qb.push("VALUES ");
        let mut first = true;
        for r in chunk.iter() {
            if !first {
                qb.push(", ");
            }
            first = false;
            qb.push("(");
            qb.push_bind(r.event_id);
            qb.push("::bigint, ");
            qb.push_bind(&r.valuation_version);
            qb.push("::text, ");
            qb.push_bind(&r.asset);
            qb.push("::text, ");
            qb.push_bind(&r.result_status);
            qb.push("::text, ");
            qb.push_bind(&r.error_detail);
            qb.push("::jsonb)");
        }
        qb.push(
            " ) AS v(event_id, valuation_version, asset, result_status, error_detail) \
             ), \
             ranked AS ( \
               SELECT event_id, valuation_version, asset, result_status, error_detail, \
                      ROW_NUMBER() OVER ( \
                        PARTITION BY event_id, valuation_version, asset \
                        ORDER BY result_status, error_detail::text NULLS FIRST \
                      ) AS rn \
                 FROM input \
             ), \
             next_n AS ( \
               SELECT r.event_id, r.valuation_version, r.asset, \
                      r.result_status, r.error_detail, \
                      COALESCE(MAX(va.attempt_number), 0) + r.rn AS attempt_number \
                 FROM ranked r \
                 LEFT JOIN valuation_attempts va \
                   ON va.event_id          = r.event_id \
                  AND va.valuation_version = r.valuation_version \
                  AND va.asset             = r.asset \
                GROUP BY r.event_id, r.valuation_version, r.asset, \
                         r.result_status, r.error_detail, r.rn \
             ) \
             INSERT INTO valuation_attempts \
                 (event_id, valuation_version, asset, attempt_number, result_status, error_detail) \
             SELECT event_id, valuation_version, asset, attempt_number, result_status, error_detail \
               FROM next_n \
             ON CONFLICT (event_id, valuation_version, asset, attempt_number) DO NOTHING",
        );
        qb.build().execute(pg).await?;
    }
    Ok(())
}
