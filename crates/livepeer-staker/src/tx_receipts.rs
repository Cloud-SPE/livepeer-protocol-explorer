//! TD-020 transaction-receipt persistence.
//!
//! Walks finalized + canonical rows in `raw_protocol_events`, fetches
//! `eth_getTransactionReceipt` for each unique tx_hash via
//! `single_call_cached` (which records the response into `rpc_call_cache`
//! for replay determinism), and writes a typed projection into
//! `tx_receipts`. Idempotent on `(chain_id, tx_hash)` so re-runs and
//! restarts converge on the same row set.
//!
//! Determinism: every row is a deterministic function of the cached RPC
//! response. The replay contract is already satisfied by `rpc_call_cache`;
//! `tx_receipts` is a derived view.

use alloy::primitives::U256;
use anyhow::{Context, Result};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use futures::stream::{self, StreamExt, TryStreamExt};
use livepeer_core::rpc::{cross_check, Provider};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::{PgPool, QueryBuilder, Row};
use std::str::FromStr;
use tracing::{debug, info, warn};

const ARBITRUM_CHAIN_ID: i64 = 42161;
const TX_RECEIPTS_CHECKPOINT: &str = "tx_receipts_backfill";
const ETH_DECIMALS: u32 = 18;

/// Default fan-out for `eth_getTransactionReceipt`. Matches the empirically-
/// safe ceiling already established by `profile-follow`'s NewRound fanout
/// (Chainstack burst tolerance — see profile.rs:NEW_ROUND_FANOUT_CONCURRENCY).
pub const DEFAULT_CONCURRENCY: usize = 12;

/// Default rows scanned per backfill iteration. PG bulk-insert parameter
/// budget at 11 columns/row gives a hard ceiling around 5950 rows; 5000 is
/// comfortable and lands one full batch in ~20 s of wall-clock at the
/// concurrency above.
pub const DEFAULT_BATCH_LIMIT: i64 = 5000;

#[derive(Debug, Default, Serialize)]
pub struct TxReceiptsBackfillSummary {
    pub candidates_seen: u64,
    pub rows_written: u64,
    pub rows_skipped_missing_receipt: u64,
    pub last_processed_block: Option<i64>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone)]
struct ReceiptCandidate {
    tx_hash: String,
    block_number: i64,
    block_timestamp: DateTime<Utc>,
}

#[derive(Debug)]
struct ReceiptRow {
    tx_hash: String,
    block_number: i64,
    block_timestamp: DateTime<Utc>,
    gas_used: U256,
    effective_gas_price: U256,
    tx_fee_wei: U256,
    tx_fee_eth: BigDecimal,
    status: i16,
    from_address: String,
    to_address: Option<String>,
}

pub async fn run_tx_receipts_backfill(
    pg: &PgPool,
    archive: &Provider,
    include_tentative: bool,
    batch_limit: i64,
    concurrency: usize,
) -> Result<TxReceiptsBackfillSummary> {
    let started = std::time::Instant::now();
    let checkpoint = load_checkpoint(pg, TX_RECEIPTS_CHECKPOINT)
        .await?
        .unwrap_or(0);
    let candidates = load_candidates(pg, checkpoint, batch_limit, include_tentative).await?;

    if candidates.is_empty() {
        // Caught up. Metric reflects the observed-zero state; no expensive
        // anti-join count query needed.
        //
        // TD-023: still tick `indexer_checkpoints.updated_at` on empty
        // polls so dashboards see a live heartbeat. The `GREATEST` clause
        // in the upsert prevents block regression.
        advance_checkpoint(pg, TX_RECEIPTS_CHECKPOINT, checkpoint).await?;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        record_iteration(0, 0, Some(checkpoint), elapsed_ms / 1000, true);
        info!(
            checkpoint,
            elapsed_ms, "tx-receipts backfill complete (no candidates)"
        );
        return Ok(TxReceiptsBackfillSummary {
            candidates_seen: 0,
            rows_written: 0,
            rows_skipped_missing_receipt: 0,
            last_processed_block: Some(checkpoint),
            elapsed_ms,
        });
    }

    let candidates_seen = candidates.len() as u64;
    let max_block = candidates
        .iter()
        .map(|c| c.block_number)
        .max()
        .expect("non-empty");

    let receipts: Vec<Option<ReceiptRow>> = stream::iter(candidates.into_iter().map(|c| {
        let pg = pg.clone();
        let archive = archive.clone();
        async move { fetch_receipt_row(&pg, &archive, c).await }
    }))
    .buffer_unordered(concurrency)
    .try_collect()
    .await?;

    let mut rows: Vec<ReceiptRow> = Vec::with_capacity(receipts.len());
    let mut skipped = 0u64;
    for r in receipts {
        match r {
            Some(row) => rows.push(row),
            None => skipped += 1,
        }
    }

    let rows_written = if rows.is_empty() {
        0
    } else {
        bulk_insert(pg, &rows).await?
    };

    advance_checkpoint(pg, TX_RECEIPTS_CHECKPOINT, max_block).await?;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    // Busy iteration: skip the anti-join `count_remaining` query — it scans
    // most of `raw_protocol_events` until backfill is well under way and
    // would dominate iteration time. Operators can query the DB directly
    // for a precise remaining count. -1 is the "unknown / busy" sentinel.
    record_iteration(-1, rows_written, Some(max_block), elapsed_ms / 1000, true);

    info!(
        candidates_seen,
        rows_written,
        rows_skipped_missing_receipt = skipped,
        last_processed_block = max_block,
        elapsed_ms,
        "tx-receipts backfill iteration"
    );

    Ok(TxReceiptsBackfillSummary {
        candidates_seen,
        rows_written,
        rows_skipped_missing_receipt: skipped,
        last_processed_block: Some(max_block),
        elapsed_ms,
    })
}

async fn load_candidates(
    pg: &PgPool,
    checkpoint: i64,
    limit: i64,
    include_tentative: bool,
) -> Result<Vec<ReceiptCandidate>> {
    let finality_clause = if include_tentative {
        ""
    } else {
        "AND r.finality = 'finalized'"
    };
    // DISTINCT here is important — a single tx can emit multiple events
    // (e.g. Bond + EarningsClaimed in the same call). They all share the
    // same tx_hash and block_number so deduping is a no-op on those keys.
    //
    // `block_number >= $2` (not >) so that if a single block contains
    // more distinct txs than `batch_limit`, the leftover txs at the
    // boundary block aren't silently skipped on the next iteration. The
    // NOT EXISTS clause handles the dedupe so re-scanning the boundary
    // block is just a quick anti-join probe, not a re-insert.
    let sql = format!(
        r#"
        SELECT DISTINCT r.tx_hash, r.block_number, r.block_timestamp
          FROM raw_protocol_events r
         WHERE r.chain_id = $1
           AND r.is_canonical = TRUE
           {finality_clause}
           AND r.block_number >= $2
           AND NOT EXISTS (
                 SELECT 1 FROM tx_receipts t
                  WHERE t.chain_id = r.chain_id
                    AND t.tx_hash  = r.tx_hash
               )
         ORDER BY r.block_number ASC, r.tx_hash ASC
         LIMIT $3
        "#
    );

    let rows = sqlx::query(&sql)
        .bind(ARBITRUM_CHAIN_ID)
        .bind(checkpoint)
        .bind(limit)
        .fetch_all(pg)
        .await
        .context("loading tx_receipts candidates")?;

    Ok(rows
        .into_iter()
        .map(|r| ReceiptCandidate {
            tx_hash: r.get::<String, _>("tx_hash"),
            block_number: r.get::<i64, _>("block_number"),
            block_timestamp: r.get::<DateTime<Utc>, _>("block_timestamp"),
        })
        .collect())
}

async fn fetch_receipt_row(
    pg: &PgPool,
    archive: &Provider,
    c: ReceiptCandidate,
) -> Result<Option<ReceiptRow>> {
    let outcome = cross_check::single_call_cached(
        pg,
        archive,
        "eth_getTransactionReceipt",
        &json!([c.tx_hash]),
        None,
    )
    .await
    .with_context(|| format!("eth_getTransactionReceipt({})", c.tx_hash))?;

    let receipt: Value =
        serde_json::from_slice(&outcome.response_bytes).context("decoding receipt JSON")?;

    if receipt.is_null() {
        // Should be rare on finalized canonical rows; log and skip.
        warn!(tx_hash = %c.tx_hash, "receipt returned null; skipping");
        return Ok(None);
    }

    let gas_used_hex = receipt
        .get("gasUsed")
        .and_then(Value::as_str)
        .with_context(|| format!("missing gasUsed on receipt for {}", c.tx_hash))?;
    let gas_price_hex = receipt
        .get("effectiveGasPrice")
        .and_then(Value::as_str)
        .or_else(|| receipt.get("gasPrice").and_then(Value::as_str))
        .with_context(|| {
            format!(
                "missing effectiveGasPrice/gasPrice on receipt for {}",
                c.tx_hash
            )
        })?;
    let status_hex = receipt
        .get("status")
        .and_then(Value::as_str)
        .with_context(|| format!("missing status on receipt for {}", c.tx_hash))?;
    let from_address = receipt
        .get("from")
        .and_then(Value::as_str)
        .with_context(|| format!("missing from on receipt for {}", c.tx_hash))?
        .to_lowercase();
    let to_address = receipt
        .get("to")
        .and_then(Value::as_str)
        .map(|s| s.to_lowercase());

    let gas_used = parse_u256_hex(gas_used_hex)
        .with_context(|| format!("parsing gasUsed for {}", c.tx_hash))?;
    let effective_gas_price = parse_u256_hex(gas_price_hex)
        .with_context(|| format!("parsing effectiveGasPrice for {}", c.tx_hash))?;
    let status = parse_status_hex(status_hex)
        .with_context(|| format!("parsing status for {}", c.tx_hash))?;
    let tx_fee_wei = gas_used.saturating_mul(effective_gas_price);
    let tx_fee_eth = wei_to_eth_decimal(tx_fee_wei);

    debug!(
        tx_hash = %c.tx_hash,
        gas_used = %gas_used,
        status,
        "decoded receipt"
    );

    Ok(Some(ReceiptRow {
        tx_hash: c.tx_hash,
        block_number: c.block_number,
        block_timestamp: c.block_timestamp,
        gas_used,
        effective_gas_price,
        tx_fee_wei,
        tx_fee_eth,
        status,
        from_address,
        to_address,
    }))
}

async fn bulk_insert(pg: &PgPool, rows: &[ReceiptRow]) -> Result<u64> {
    let mut qb: QueryBuilder<sqlx::Postgres> = QueryBuilder::new(
        "INSERT INTO tx_receipts \
         (chain_id, tx_hash, block_number, block_timestamp, gas_used, \
          effective_gas_price, tx_fee_wei, tx_fee_eth, status, from_address, to_address) ",
    );
    qb.push_values(rows.iter(), |mut b, r| {
        b.push_bind(ARBITRUM_CHAIN_ID)
            .push_bind(&r.tx_hash)
            .push_bind(r.block_number)
            .push_bind(r.block_timestamp)
            .push_bind(BigDecimal::from_str(&r.gas_used.to_string()).unwrap_or_default())
            .push_bind(BigDecimal::from_str(&r.effective_gas_price.to_string()).unwrap_or_default())
            .push_bind(BigDecimal::from_str(&r.tx_fee_wei.to_string()).unwrap_or_default())
            .push_bind(&r.tx_fee_eth)
            .push_bind(r.status)
            .push_bind(&r.from_address)
            .push_bind(r.to_address.as_ref());
    });
    qb.push(" ON CONFLICT (chain_id, tx_hash) DO NOTHING");
    let result = qb
        .build()
        .execute(pg)
        .await
        .context("bulk-inserting tx_receipts")?;
    Ok(result.rows_affected())
}

fn parse_u256_hex(raw: &str) -> Result<U256> {
    U256::from_str_radix(raw.trim_start_matches("0x"), 16)
        .map_err(|e| anyhow::anyhow!("invalid hex u256 ({raw}): {e}"))
}

fn parse_status_hex(raw: &str) -> Result<i16> {
    let v = u64::from_str_radix(raw.trim_start_matches("0x"), 16)
        .map_err(|e| anyhow::anyhow!("invalid hex status ({raw}): {e}"))?;
    if v > i16::MAX as u64 {
        anyhow::bail!("status out of range: {v}");
    }
    Ok(v as i16)
}

fn wei_to_eth_decimal(wei: U256) -> BigDecimal {
    let raw = BigDecimal::from_str(&wei.to_string()).unwrap_or_default();
    raw / BigDecimal::from(10u128.pow(ETH_DECIMALS))
}

async fn load_checkpoint(pg: &PgPool, name: &str) -> Result<Option<i64>> {
    let block = sqlx::query_scalar::<_, i64>(
        "SELECT last_processed_block FROM indexer_checkpoints WHERE name = $1",
    )
    .bind(name)
    .fetch_optional(pg)
    .await?;
    Ok(block)
}

async fn advance_checkpoint(pg: &PgPool, name: &str, block: i64) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO indexer_checkpoints (name, chain_id, last_processed_block, updated_at)
           VALUES ($1, $2, $3, now())
           ON CONFLICT (name) DO UPDATE
              SET last_processed_block = GREATEST(indexer_checkpoints.last_processed_block, EXCLUDED.last_processed_block),
                  updated_at = now()"#,
    )
    .bind(name)
    .bind(ARBITRUM_CHAIN_ID)
    .bind(block)
    .execute(pg)
    .await?;
    Ok(())
}

fn record_iteration(
    candidates_remaining: i64,
    rows_written: u64,
    last_block: Option<i64>,
    elapsed_seconds: u64,
    succeeded: bool,
) {
    crate::metrics::record_tx_receipts_iteration(
        candidates_remaining,
        rows_written,
        last_block,
        elapsed_seconds as i64,
        succeeded,
    );
}
