//! S6.1 — fetch Reward logs from archive, decode, write raw_protocol_events,
//! advance checkpoint atomically. One event type, one contract, one block range.
//!
//! S6.2 will generalize: all event types, dynamic batch sizing, full §6 catalog,
//! strict-decode allowlist. S6.3 adds dead-letter handling. This is the smallest
//! end-to-end slice that proves the pipeline.

use crate::events::Reward;
use alloy::primitives::{B256, FixedBytes, LogData};
use alloy::sol_types::SolEvent;
use anyhow::{Context, Result};
use bigdecimal::BigDecimal;
use chrono::DateTime;
use livepeer_core::rpc::{cross_check, Provider};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, QueryBuilder};
use std::collections::HashMap;
use std::str::FromStr;
use tracing::{info, warn};

const ARBITRUM_CHAIN_ID: i64 = 42161;
const VALUATION_VERSION: &str = "v1";
const BATCH_INSERT_SIZE: usize = 500;

/// Raw log record as returned by `eth_getLogs`. We deserialize into this; alloy then
/// decodes the typed event from `topics + data`.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RawLog {
    address: String,
    topics: Vec<String>,
    data: String,
    block_number: String,    // hex
    block_hash: String,
    transaction_hash: String,
    log_index: String,       // hex
}

/// Backfill all Reward events in [from_block, to_block] (inclusive).
///
/// Pipeline: eth_getLogs → decode → fetch + cache block timestamps →
/// insert raw_protocol_events (batched, idempotent) + advance checkpoint (one tx).
pub async fn backfill_rewards(
    pg: &PgPool,
    archive: &Provider,
    bonding_manager: &str,
    abi_hash: &str,
    from_block: u64,
    to_block: u64,
) -> Result<u64> {
    info!(from_block, to_block, contract = bonding_manager, "fetching Reward logs");
    let topic0 = format!("0x{:x}", Reward::SIGNATURE_HASH);
    let logs_value = archive
        .eth_get_logs(bonding_manager, &topic0, from_block, to_block)
        .await?;
    let raw_logs: Vec<RawLog> = serde_json::from_value(logs_value).context("decoding eth_getLogs response")?;
    info!(count = raw_logs.len(), "logs fetched");

    if raw_logs.is_empty() {
        // Still advance the checkpoint — we processed the range.
        advance_checkpoint(pg, to_block).await?;
        return Ok(0);
    }

    // Fetch block timestamps for every unique block touched. Cached via
    // single_call_cached; on re-run these are no-op.
    let mut block_ts: HashMap<u64, i64> = HashMap::new();
    let unique_blocks: std::collections::BTreeSet<u64> = raw_logs
        .iter()
        .map(|l| u64_from_hex(&l.block_number))
        .collect::<Result<_>>()?;
    for n in &unique_blocks {
        let outcome = cross_check::single_call_cached(
            pg,
            archive,
            "eth_getBlockByNumber",
            &serde_json::json!([format!("0x{:x}", n), false]),
            Some(*n as i64),
        )
        .await?;
        let header: serde_json::Value = serde_json::from_slice(&outcome.response_bytes)?;
        let ts_hex = header
            .get("timestamp")
            .and_then(|v| v.as_str())
            .context("block has no timestamp")?;
        block_ts.insert(*n, i64_from_hex(ts_hex)?);
    }
    info!(blocks = block_ts.len(), "block timestamps cached");

    // Decode + transform.
    let mut prepared: Vec<PreparedRow> = Vec::with_capacity(raw_logs.len());
    for raw in &raw_logs {
        let block_number = u64_from_hex(&raw.block_number)?;
        let log_index = u32_from_hex(&raw.log_index)?;
        let topics_b256: Vec<FixedBytes<32>> = raw
            .topics
            .iter()
            .map(|t| FixedBytes::<32>::from_str(t.trim_start_matches("0x")))
            .collect::<std::result::Result<_, _>>()
            .context("decoding topic bytes")?;
        let data_bytes =
            alloy::hex::decode(raw.data.trim_start_matches("0x")).context("decoding data hex")?;
        let log_data = LogData::new(topics_b256, data_bytes.into())
            .context("malformed LogData (topics/data shape)")?;
        // validate=true → asserts topic0 matches; we filtered on it but verify anyway.
        let decoded = Reward::decode_log_data(&log_data, true)
            .with_context(|| format!("decoding Reward at tx {}, log_index {log_index}", raw.transaction_hash))?;

        let amount_raw_str = decoded.amount.to_string();
        let amount_raw = BigDecimal::from_str(&amount_raw_str).unwrap_or_default();
        let amount_normalized = amount_raw.clone() / BigDecimal::from(10u64.pow(18));

        let ts_secs = *block_ts.get(&block_number).context("missing block timestamp")?;
        let block_timestamp = DateTime::from_timestamp(ts_secs, 0).context("invalid timestamp")?;

        let raw_event = serde_json::to_value(raw)?;

        prepared.push(PreparedRow {
            tx_hash: raw.transaction_hash.to_lowercase(),
            log_index: log_index as i32,
            block_number: block_number as i64,
            block_hash: raw.block_hash.to_lowercase(),
            block_timestamp,
            contract_address: raw.address.to_lowercase(),
            event_signature: format!("0x{:x}", Reward::SIGNATURE_HASH),
            transcoder: format!("0x{:040x}", decoded.transcoder),
            amount_raw,
            amount_normalized,
            raw_event,
        });
    }

    // Insert in batches inside one transaction; advance checkpoint atomically.
    let mut tx = pg.begin().await?;
    let mut inserted = 0u64;
    for chunk in prepared.chunks(BATCH_INSERT_SIZE) {
        let mut qb = QueryBuilder::new(
            "INSERT INTO raw_protocol_events \
             (chain_id, tx_hash, log_index, block_number, block_hash, block_timestamp, \
              contract_address, contract_name, event_name, event_signature, \
              asset, amount_raw, amount_normalized, is_valuable, \
              from_address, to_address, finality, is_canonical, \
              raw_event, abi_hash_used) ",
        );
        qb.push_values(chunk.iter(), |mut b, row| {
            b.push_bind(ARBITRUM_CHAIN_ID);
            b.push_bind(&row.tx_hash);
            b.push_bind(row.log_index);
            b.push_bind(row.block_number);
            b.push_bind(&row.block_hash);
            b.push_bind(row.block_timestamp);
            b.push_bind(&row.contract_address);
            b.push_bind("BondingManager");
            b.push_bind("Reward");
            b.push_bind(&row.event_signature);
            b.push_bind("LPT");
            b.push_bind(&row.amount_raw);
            b.push_bind(&row.amount_normalized);
            b.push_bind(true); // is_valuable
            b.push_bind(Option::<String>::None); // from_address
            b.push_bind(&row.transcoder); // to_address (recipient of reward)
            b.push_bind("tentative");
            b.push_bind(true); // is_canonical
            b.push_bind(&row.raw_event);
            b.push_bind(abi_hash);
        });
        qb.push(" ON CONFLICT (chain_id, tx_hash, log_index) DO NOTHING");
        let result = qb.build().execute(&mut *tx).await?;
        inserted += result.rows_affected();
    }

    sqlx::query(
        r#"INSERT INTO indexer_checkpoints
                (name, chain_id, last_processed_block, updated_at)
           VALUES ($1, $2, $3, now())
           ON CONFLICT (name) DO UPDATE
              SET last_processed_block = GREATEST(indexer_checkpoints.last_processed_block, EXCLUDED.last_processed_block),
                  updated_at = now()"#,
    )
    .bind("main")
    .bind(ARBITRUM_CHAIN_ID)
    .bind(to_block as i64)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    if inserted as usize != prepared.len() {
        warn!(
            seen = prepared.len(),
            inserted,
            "some rows already present (idempotent re-insert)"
        );
    }
    let _ = VALUATION_VERSION; // reserved for the valuator slice

    Ok(inserted)
}

async fn advance_checkpoint(pg: &PgPool, to_block: u64) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO indexer_checkpoints
                (name, chain_id, last_processed_block, updated_at)
           VALUES ('main', $1, $2, now())
           ON CONFLICT (name) DO UPDATE
              SET last_processed_block = GREATEST(indexer_checkpoints.last_processed_block, EXCLUDED.last_processed_block),
                  updated_at = now()"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(to_block as i64)
    .execute(pg)
    .await?;
    Ok(())
}

#[derive(Debug)]
struct PreparedRow {
    tx_hash: String,
    log_index: i32,
    block_number: i64,
    block_hash: String,
    block_timestamp: DateTime<chrono::Utc>,
    contract_address: String,
    event_signature: String,
    transcoder: String,
    amount_raw: BigDecimal,
    amount_normalized: BigDecimal,
    raw_event: serde_json::Value,
}

fn u64_from_hex(s: &str) -> Result<u64> {
    let s = s.trim_start_matches("0x");
    Ok(u64::from_str_radix(s, 16)?)
}
fn u32_from_hex(s: &str) -> Result<u32> {
    let s = s.trim_start_matches("0x");
    Ok(u32::from_str_radix(s, 16)?)
}
fn i64_from_hex(s: &str) -> Result<i64> {
    let s = s.trim_start_matches("0x");
    Ok(i64::from_str_radix(s, 16)?)
}

// Quiet `unused` for the import we need only when sol! is present.
#[allow(dead_code)]
fn _b256_assert(b: B256) -> B256 {
    b
}
