//! Generalized log backfill — chunked driver, strict-decode halt vs dead-letter,
//! dynamic batch sizing, atomic per-chunk commit.
//!
//! Pipeline (SPEC §3.2, §10.2, §12.3, §13.4):
//!   drive_backfill walks [from, to] in chunks of `current_batch_size`.
//!   Each chunk: eth_getLogs(contract, all-topic0s) → dispatch by topic0 →
//!     - Decoded:        push to insert batch
//!     - DecodeFailed:   if strict (§6.2), bail entire batch (§10.2.1)
//!                       else write to decode_failures (§10.2.2)
//!   Then ONE transaction: insert events + insert dead-letters + advance
//!   checkpoint. Strict failures abort before commit so the txn rolls back
//!   cleanly (no partial state).
//!
//! Dynamic batch (§13.4): start 5000, halve on transient RPC error, double on
//! success up to 10000. Hard cap 10000.

use crate::events::BondingManager::{
    self, Bond, EarningsClaimed, Rebond, Reward, TranscoderSlashed, TransferBond, Unbond,
    WithdrawStake,
};
// BondingManager has two WithdrawFees overloads. _0 carries (delegator, recipient, amount);
// _1 is the legacy form with just (delegator) and no amount. We bind _0 explicitly.
// _1 is post-Delta-impossible on Arbitrum (deployment is post-Delta) so we don't subscribe.
use crate::events::BondingManager::WithdrawFees_0 as WithdrawFees;
use crate::events::Governor::{
    self, ProposalCanceled, ProposalCreated, ProposalExecuted, ProposalQueued, VoteCast,
    VoteCastWithParams,
};
use crate::events::LivepeerToken::{self, Burn, Mint, Transfer};
use crate::events::RoundsManager::{self, NewRound};
// TicketBroker emits "Withdrawal" (full deposit + reserve drain) — not "Withdraw".
use crate::events::TicketBroker::{
    self, DepositFunded, ReserveClaimed, ReserveFunded, Unlock, UnlockCancelled,
    WinningTicketRedeemed, WinningTicketTransfer, Withdrawal,
};
use alloy::primitives::{Address, FixedBytes, LogData, B256, U256};
use alloy::sol_types::SolEvent;
use anyhow::{anyhow, Context, Result};
use bigdecimal::BigDecimal;
use chrono::DateTime;
use livepeer_core::rpc::{cross_check, Provider};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, QueryBuilder, Transaction};
use std::collections::HashMap;
use std::str::FromStr;
use tracing::{error, info, warn};

const ARBITRUM_CHAIN_ID: i64 = 42161;
const BATCH_INSERT_SIZE: usize = 500;
const LPT_DECIMALS: u32 = 18;
const ETH_DECIMALS: u32 = 18;
const DEFAULT_BATCH_BLOCKS: u64 = 5_000;
// Chainstack's eth_getLogs hard cap is 9,000 blocks per call. Our cap matches.
const MAX_BATCH_BLOCKS: u64 = 9_000;
const MIN_BATCH_BLOCKS: u64 = 100;
// Bound retries per chunk. With 50 retries and capped exponential backoff this
// gives a chunk many minutes to ride through Chainstack throttling waves before
// the driver gives up and forces a process exit (which the script can re-launch
// cleanly via per-contract checkpoint).
const MAX_RETRIES_PER_CHUNK: u32 = 50;
const RETRY_BACKOFF_SECS: u64 = 2;
// Exponential cap applied when we're already at MIN_BATCH and still failing.
const MIN_BATCH_RETRY_BACKOFF_CAP_SECS: u64 = 60;

/// Which contract to back-fill. Maps to the proxy address from config and the abi_hash
/// from the registry. Each contract has a fixed list of topic0s of interest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractKind {
    BondingManager,
    TicketBroker,
    LivepeerToken,
    RoundsManager,
    Governor,
}

impl ContractKind {
    pub fn name(self) -> &'static str {
        match self {
            ContractKind::BondingManager => "BondingManager",
            ContractKind::TicketBroker => "TicketBroker",
            ContractKind::LivepeerToken => "LivepeerToken",
            ContractKind::RoundsManager => "RoundsManager",
            ContractKind::Governor => "Governor",
        }
    }
    #[allow(dead_code)] // useful from tests + future CLI parsing
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "BondingManager" => Some(Self::BondingManager),
            "TicketBroker" => Some(Self::TicketBroker),
            "LivepeerToken" => Some(Self::LivepeerToken),
            "RoundsManager" => Some(Self::RoundsManager),
            "Governor" => Some(Self::Governor),
            _ => None,
        }
    }
}

/// Aggregate result of a full chunked backfill over [from_block, to_block].
#[derive(Debug, Default)]
pub struct DriveSummary {
    pub chunks: u64,
    pub logs_seen: u64,
    pub events_inserted: u64,
    pub dead_lettered: u64,
    pub final_batch_size: u64,
}

/// Per-contract checkpoint name. Avoids cross-contract collisions when the
/// driver runs contracts sequentially. An optional non-empty `suffix` (e.g.
/// "patch") yields `indexer_<ContractName>_<suffix>` so a parallel patch run
/// can scan the same contract from genesis without colliding with the live
/// run's checkpoint.
pub fn checkpoint_name(contract: ContractKind, suffix: &str) -> String {
    if suffix.is_empty() {
        format!("indexer_{}", contract.name())
    } else {
        format!("indexer_{}_{}", contract.name(), suffix)
    }
}

/// Resume from `indexer_checkpoints(<per-contract-name>)` if it's past `requested_from`.
/// Returns the actual starting block.
pub async fn resume_from(
    pg: &PgPool,
    contract: ContractKind,
    suffix: &str,
    requested_from: u64,
) -> Result<u64> {
    let name = checkpoint_name(contract, suffix);
    let checkpoint: Option<i64> =
        sqlx::query_scalar("SELECT last_processed_block FROM indexer_checkpoints WHERE name = $1")
            .bind(&name)
            .fetch_optional(pg)
            .await?;
    Ok(match checkpoint {
        Some(cp) if (cp as u64) >= requested_from => (cp as u64).saturating_add(1),
        _ => requested_from,
    })
}

/// Drive a chunked backfill over `[from_block, to_block]`. Halts on strict-decode failure
/// (the chunk's transaction is rolled back by then). Halves batch size on transient RPC
/// errors and retries; doubles on success up to MAX_BATCH_BLOCKS.
pub async fn drive_backfill(
    pg: &PgPool,
    archive: &Provider,
    contract: ContractKind,
    suffix: &str,
    proxy_address: &str,
    abi_hash: &str,
    from_block: u64,
    to_block: u64,
) -> Result<DriveSummary> {
    if to_block < from_block {
        return Ok(DriveSummary::default());
    }
    let mut summary = DriveSummary::default();
    let mut current_batch = DEFAULT_BATCH_BLOCKS;
    let mut next = from_block;
    let mut retries_in_a_row: u32 = 0;
    let mut min_batch_retries: u32 = 0;
    while next <= to_block {
        let chunk_end = (next + current_batch - 1).min(to_block);
        match backfill_chunk(
            pg,
            archive,
            contract,
            suffix,
            proxy_address,
            abi_hash,
            next,
            chunk_end,
        )
        .await
        {
            Ok(chunk) => {
                summary.chunks += 1;
                summary.logs_seen += chunk.logs_seen;
                summary.events_inserted += chunk.events_inserted;
                summary.dead_lettered += chunk.dead_lettered;
                next = chunk_end + 1;
                retries_in_a_row = 0;
                min_batch_retries = 0;
                if current_batch < MAX_BATCH_BLOCKS {
                    current_batch = (current_batch * 2).min(MAX_BATCH_BLOCKS);
                }
            }
            Err(e) if is_transient(&e) && retries_in_a_row >= MAX_RETRIES_PER_CHUNK => {
                error!(
                    chunk_start = next, chunk_end, retries_in_a_row,
                    error = %e,
                    "transient RPC error budget exhausted; halting (run can resume from checkpoint)"
                );
                return Err(e);
            }
            Err(e) if is_transient(&e) && current_batch > MIN_BATCH_BLOCKS => {
                let halved = (current_batch / 2).max(MIN_BATCH_BLOCKS);
                retries_in_a_row += 1;
                warn!(
                    chunk_start = next,
                    chunk_end,
                    old_batch = current_batch,
                    new_batch = halved,
                    retry = retries_in_a_row,
                    backoff_secs = RETRY_BACKOFF_SECS,
                    error = %e,
                    "transient RPC error — halving batch size and retrying"
                );
                current_batch = halved;
                tokio::time::sleep(std::time::Duration::from_secs(RETRY_BACKOFF_SECS)).await;
                continue; // retry the same `next` with smaller chunk
            }
            Err(e) if is_transient(&e) => {
                // At MIN_BATCH already and still failing — exponential backoff
                // capped at MIN_BATCH_RETRY_BACKOFF_CAP_SECS so we don't hammer
                // a throttled provider while still riding through long waves.
                retries_in_a_row += 1;
                min_batch_retries += 1;
                if retries_in_a_row >= MAX_RETRIES_PER_CHUNK {
                    error!(chunk_start = next, chunk_end, error = %e, "min-batch transient retries exhausted; halting");
                    return Err(e);
                }
                let exp = min_batch_retries.saturating_sub(1).min(20);
                let backoff_secs = (RETRY_BACKOFF_SECS.saturating_mul(2u64.saturating_pow(exp)))
                    .min(MIN_BATCH_RETRY_BACKOFF_CAP_SECS);
                warn!(
                    chunk_start = next, chunk_end,
                    retry = retries_in_a_row,
                    min_batch_retry = min_batch_retries,
                    backoff_secs,
                    error = %e,
                    "transient RPC error at MIN_BATCH — backing off and retrying"
                );
                tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
                continue;
            }
            Err(e) => {
                error!(chunk_start = next, chunk_end, error = %e, "chunk failed; halting");
                return Err(e);
            }
        }
    }
    summary.final_batch_size = current_batch;
    Ok(summary)
}

/// Process a single chunk. Atomic: events + dead-letters + checkpoint advance, OR rollback.
struct ChunkSummary {
    logs_seen: u64,
    events_inserted: u64,
    dead_lettered: u64,
}

async fn backfill_chunk(
    pg: &PgPool,
    archive: &Provider,
    contract: ContractKind,
    suffix: &str,
    proxy_address: &str,
    abi_hash: &str,
    chunk_start: u64,
    chunk_end: u64,
) -> Result<ChunkSummary> {
    let topic0s = topic0s_for(contract);
    let logs_value =
        eth_get_logs_multi_topic(pg, archive, proxy_address, &topic0s, chunk_start, chunk_end)
            .await?;
    let raw_logs: Vec<RawLog> =
        serde_json::from_value(logs_value).context("decoding eth_getLogs response")?;

    if raw_logs.is_empty() {
        let mut tx = pg.begin().await?;
        advance_checkpoint(&mut tx, contract, suffix, chunk_end).await?;
        tx.commit().await?;
        return Ok(ChunkSummary {
            logs_seen: 0,
            events_inserted: 0,
            dead_lettered: 0,
        });
    }

    let block_ts = fetch_block_timestamps(pg, archive, &raw_logs).await?;

    // Dispatch each log.
    let mut prepared: Vec<PreparedRow> = Vec::with_capacity(raw_logs.len());
    let mut dead_letters: Vec<DeadLetterRow> = Vec::new();
    let mut strict_failures: Vec<String> = Vec::new();

    for raw in &raw_logs {
        match decode_one(contract, raw, &block_ts) {
            DispatchOutcome::Decoded(row) => prepared.push(row),
            DispatchOutcome::DecodeFailed {
                event_name,
                topic0,
                is_strict,
                error,
            } => {
                if is_strict {
                    let detail = format!(
                        "tx={} log_index={} event={} topic0=0x{:x} err={}",
                        raw.transaction_hash, raw.log_index, event_name, topic0, error
                    );
                    error!(
                        contract = contract.name(),
                        event_name,
                        tx_hash = %raw.transaction_hash,
                        log_index = %raw.log_index,
                        error = %error,
                        "STRICT decode failure on critical event — will halt batch (§10.2.1)"
                    );
                    strict_failures.push(detail);
                } else {
                    dead_letters.push(DeadLetterRow::from_raw(raw, abi_hash, &error)?);
                }
            }
            DispatchOutcome::UnknownTopic0 { topic0 } => {
                let err = format!(
                    "topic0 0x{:x} not in known set for {}",
                    topic0,
                    contract.name()
                );
                warn!(
                    contract = contract.name(),
                    tx_hash = %raw.transaction_hash,
                    "unknown topic0 — dead-lettering (defensive: should not occur given filter)"
                );
                dead_letters.push(DeadLetterRow::from_raw(raw, abi_hash, &err)?);
            }
        }
    }

    if !strict_failures.is_empty() {
        return Err(anyhow!(
            "strict-decode halt: {} critical-event decode failure(s) in [{}, {}]: {}",
            strict_failures.len(),
            chunk_start,
            chunk_end,
            strict_failures.join(" | ")
        ));
    }

    // Atomic commit.
    let mut tx = pg.begin().await?;
    let inserted = insert_events(&mut tx, &prepared, abi_hash).await?;
    let dl_inserted = insert_dead_letters(&mut tx, &dead_letters).await?;
    advance_checkpoint(&mut tx, contract, suffix, chunk_end).await?;
    tx.commit().await?;

    info!(
        contract = contract.name(),
        chunk_start,
        chunk_end,
        logs = raw_logs.len(),
        events_inserted = inserted,
        dead_lettered = dl_inserted,
        "chunk committed"
    );

    Ok(ChunkSummary {
        logs_seen: raw_logs.len() as u64,
        events_inserted: inserted,
        dead_lettered: dl_inserted,
    })
}

async fn insert_events(
    tx: &mut Transaction<'_, Postgres>,
    prepared: &[PreparedRow],
    abi_hash: &str,
) -> Result<u64> {
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
            b.push_bind(row.contract_name);
            b.push_bind(row.event_name);
            b.push_bind(&row.event_signature);
            b.push_bind(row.asset);
            b.push_bind(row.amount_raw.clone());
            b.push_bind(row.amount_normalized.clone());
            b.push_bind(row.is_valuable);
            b.push_bind(row.from_address.clone());
            b.push_bind(row.to_address.clone());
            b.push_bind("tentative");
            b.push_bind(true);
            b.push_bind(&row.raw_event);
            b.push_bind(abi_hash);
        });
        qb.push(" ON CONFLICT (chain_id, tx_hash, log_index) DO NOTHING");
        let result = qb.build().execute(&mut **tx).await?;
        inserted += result.rows_affected();
    }
    Ok(inserted)
}

async fn insert_dead_letters(
    tx: &mut Transaction<'_, Postgres>,
    dead: &[DeadLetterRow],
) -> Result<u64> {
    if dead.is_empty() {
        return Ok(0);
    }
    let mut inserted = 0u64;
    for chunk in dead.chunks(BATCH_INSERT_SIZE) {
        let mut qb = QueryBuilder::new(
            "INSERT INTO decode_failures \
             (chain_id, block_number, block_hash, tx_hash, log_index, contract_address, \
              topics, data, attempted_abi_hash, error_message) ",
        );
        qb.push_values(chunk.iter(), |mut b, row| {
            b.push_bind(ARBITRUM_CHAIN_ID);
            b.push_bind(row.block_number);
            b.push_bind(&row.block_hash);
            b.push_bind(&row.tx_hash);
            b.push_bind(row.log_index);
            b.push_bind(&row.contract_address);
            b.push_bind(&row.topics);
            b.push_bind(&row.data);
            b.push_bind(&row.attempted_abi_hash);
            b.push_bind(&row.error_message);
        });
        qb.push(" ON CONFLICT (chain_id, tx_hash, log_index) DO NOTHING");
        let result = qb.build().execute(&mut **tx).await?;
        inserted += result.rows_affected();
    }
    Ok(inserted)
}

async fn fetch_block_timestamps(
    pg: &PgPool,
    archive: &Provider,
    raw_logs: &[RawLog],
) -> Result<HashMap<u64, i64>> {
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
    Ok(block_ts)
}

/// Strictness check from SPEC §6.2 (v1.6): which (contract, topic0) pairs are
/// critical-events that must halt the batch on decode failure.
fn is_strict_event(contract: ContractKind, topic0: B256) -> bool {
    match contract {
        ContractKind::BondingManager => {
            topic0 == Bond::SIGNATURE_HASH
                || topic0 == Unbond::SIGNATURE_HASH
                || topic0 == Rebond::SIGNATURE_HASH
                || topic0 == WithdrawStake::SIGNATURE_HASH
                || topic0 == TransferBond::SIGNATURE_HASH
                || topic0 == Reward::SIGNATURE_HASH
                || topic0 == EarningsClaimed::SIGNATURE_HASH
                || topic0 == TranscoderSlashed::SIGNATURE_HASH
        }
        ContractKind::TicketBroker => {
            topic0 == WinningTicketRedeemed::SIGNATURE_HASH
                || topic0 == WinningTicketTransfer::SIGNATURE_HASH
        }
        ContractKind::LivepeerToken => topic0 == Transfer::SIGNATURE_HASH,
        // RoundsManager and Governor have no critical events in v1 — non-monetary only.
        ContractKind::RoundsManager | ContractKind::Governor => false,
    }
}

/// Best-effort heuristic: distinguish transient HTTP errors (worth halving + retry)
/// from logical errors (halt). Strict-decode halts come back as anyhow errors with
/// "strict-decode halt" in the message — those must NOT retry.
fn is_transient(e: &anyhow::Error) -> bool {
    let msg = format!("{e}");
    if msg.contains("strict-decode halt") {
        return false;
    }
    msg.contains("HTTP error")
        || msg.contains("timeout")
        || msg.contains("rate")
        || msg.contains("status code: 429")
        || msg.contains("status code: 5")
}

// ---------------------------------------------------------------------------
// Per-event decoders (dispatch by topic0)
// ---------------------------------------------------------------------------

/// Outcome of dispatching a single log. `Decoded` has a row to insert; `DecodeFailed`
/// is routed by `is_strict` (halt vs dead-letter); `UnknownTopic0` is defensive (our
/// eth_getLogs filter shouldn't return unknown topics, but if it does, dead-letter).
enum DispatchOutcome {
    Decoded(PreparedRow),
    DecodeFailed {
        event_name: &'static str,
        topic0: B256,
        is_strict: bool,
        error: String,
    },
    UnknownTopic0 {
        topic0: B256,
    },
}

fn decode_one(
    contract: ContractKind,
    raw: &RawLog,
    block_ts: &HashMap<u64, i64>,
) -> DispatchOutcome {
    let topic0 = match raw
        .topics
        .first()
        .and_then(|t| FixedBytes::<32>::from_str(t.trim_start_matches("0x")).ok())
    {
        Some(t) => t,
        None => {
            return DispatchOutcome::DecodeFailed {
                event_name: "(none)",
                topic0: B256::ZERO,
                is_strict: false,
                error: "log missing or malformed topic0".into(),
            };
        }
    };

    let log_data = match build_log_data(raw) {
        Ok(d) => d,
        Err(e) => {
            return DispatchOutcome::DecodeFailed {
                event_name: "(unknown)",
                topic0,
                is_strict: is_strict_event(contract, topic0),
                error: e.to_string(),
            };
        }
    };

    let block_number = match u64_from_hex(&raw.block_number) {
        Ok(n) => n,
        Err(e) => {
            return DispatchOutcome::DecodeFailed {
                event_name: "(envelope)",
                topic0,
                is_strict: false,
                error: format!("bad block_number: {e}"),
            };
        }
    };
    let log_index = match u32_from_hex(&raw.log_index) {
        Ok(n) => n,
        Err(e) => {
            return DispatchOutcome::DecodeFailed {
                event_name: "(envelope)",
                topic0,
                is_strict: false,
                error: format!("bad log_index: {e}"),
            };
        }
    };
    let ts_secs = match block_ts.get(&block_number) {
        Some(n) => *n,
        None => {
            return DispatchOutcome::DecodeFailed {
                event_name: "(envelope)",
                topic0,
                is_strict: false,
                error: format!("missing block timestamp for block {block_number}"),
            };
        }
    };
    let block_timestamp = match DateTime::from_timestamp(ts_secs, 0) {
        Some(t) => t,
        None => {
            return DispatchOutcome::DecodeFailed {
                event_name: "(envelope)",
                topic0,
                is_strict: false,
                error: format!("invalid timestamp: {ts_secs}"),
            };
        }
    };
    let raw_event = match serde_json::to_value(raw) {
        Ok(v) => v,
        Err(e) => {
            return DispatchOutcome::DecodeFailed {
                event_name: "(envelope)",
                topic0,
                is_strict: false,
                error: e.to_string(),
            };
        }
    };
    let contract_address = raw.address.to_lowercase();
    let block_hash = raw.block_hash.to_lowercase();
    let tx_hash = raw.transaction_hash.to_lowercase();

    // Common envelope used by all branches.
    let mut row = PreparedRow {
        tx_hash,
        log_index: log_index as i32,
        block_number: block_number as i64,
        block_hash,
        block_timestamp,
        contract_address,
        contract_name: contract.name(),
        event_name: "",
        event_signature: format!("0x{:x}", topic0),
        asset: None,
        amount_raw: None,
        amount_normalized: None,
        is_valuable: false,
        from_address: None,
        to_address: None,
        raw_event,
    };

    // Each branch is: try-decode → on Ok fill row + return Decoded; on Err return DecodeFailed.
    // Routed through one helper closure to keep the body readable.
    macro_rules! decoded {
        ($name:expr, $body:block) => {{
            row.event_name = $name;
            $body
            return DispatchOutcome::Decoded(row);
        }};
    }
    match contract {
        ContractKind::BondingManager => {
            if topic0 == Reward::SIGNATURE_HASH {
                match Reward::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("Reward", {
                        row.asset = Some("LPT");
                        row.is_valuable = true;
                        set_amount(&mut row, d.amount, LPT_DECIMALS);
                        row.to_address = Some(addr_lower(&d.transcoder));
                    }),
                    Err(e) => return decode_failed(contract, "Reward", topic0, e),
                }
            } else if topic0 == Bond::SIGNATURE_HASH {
                match Bond::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("Bond", {
                        row.asset = Some("LPT");
                        row.is_valuable = true;
                        // SPEC §6.3 — additionalAmount, NOT bondedAmount.
                        set_amount(&mut row, d.additionalAmount, LPT_DECIMALS);
                        row.from_address = Some(addr_lower(&d.delegator));
                        row.to_address = Some(addr_lower(&d.newDelegate));
                    }),
                    Err(e) => return decode_failed(contract, "Bond", topic0, e),
                }
            } else if topic0 == Unbond::SIGNATURE_HASH {
                match Unbond::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("Unbond", {
                        row.asset = Some("LPT");
                        row.is_valuable = true;
                        set_amount(&mut row, d.amount, LPT_DECIMALS);
                        row.from_address = Some(addr_lower(&d.delegator));
                        row.to_address = Some(addr_lower(&d.delegate));
                    }),
                    Err(e) => return decode_failed(contract, "Unbond", topic0, e),
                }
            } else if topic0 == Rebond::SIGNATURE_HASH {
                match Rebond::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("Rebond", {
                        row.asset = Some("LPT");
                        row.is_valuable = true;
                        set_amount(&mut row, d.amount, LPT_DECIMALS);
                        row.from_address = Some(addr_lower(&d.delegator));
                        row.to_address = Some(addr_lower(&d.delegate));
                    }),
                    Err(e) => return decode_failed(contract, "Rebond", topic0, e),
                }
            } else if topic0 == WithdrawStake::SIGNATURE_HASH {
                match WithdrawStake::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("WithdrawStake", {
                        row.asset = Some("LPT");
                        row.is_valuable = true;
                        set_amount(&mut row, d.amount, LPT_DECIMALS);
                        row.from_address = Some(addr_lower(&d.delegator));
                    }),
                    Err(e) => return decode_failed(contract, "WithdrawStake", topic0, e),
                }
            } else if topic0 == TransferBond::SIGNATURE_HASH {
                match TransferBond::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("TransferBond", {
                        row.asset = Some("LPT");
                        row.is_valuable = true;
                        set_amount(&mut row, d.amount, LPT_DECIMALS);
                        row.from_address = Some(addr_lower(&d.oldDelegator));
                        row.to_address = Some(addr_lower(&d.newDelegator));
                    }),
                    Err(e) => return decode_failed(contract, "TransferBond", topic0, e),
                }
            } else if topic0 == EarningsClaimed::SIGNATURE_HASH {
                match EarningsClaimed::decode_log_data(&log_data, true) {
                    Ok(d) => {
                        // Multi-asset per SPEC §6.8.
                        row.event_name = "EarningsClaimed";
                        row.asset = None;
                        row.is_valuable = true;
                        row.amount_raw = None;
                        row.amount_normalized = None;
                        row.from_address = Some(addr_lower(&d.delegator));
                        row.to_address = Some(addr_lower(&d.delegate));
                        if let Some(obj) = row.raw_event.as_object_mut() {
                            obj.insert(
                                "decoded".to_string(),
                                serde_json::json!({
                                    "rewards":    d.rewards.to_string(),
                                    "fees":       d.fees.to_string(),
                                    "startRound": d.startRound.to_string(),
                                    "endRound":   d.endRound.to_string(),
                                }),
                            );
                        }
                        return DispatchOutcome::Decoded(row);
                    }
                    Err(e) => return decode_failed(contract, "EarningsClaimed", topic0, e),
                }
            } else if topic0 == WithdrawFees::SIGNATURE_HASH {
                match WithdrawFees::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("WithdrawFees", {
                        row.asset = Some("ETH");
                        row.is_valuable = true;
                        set_amount(&mut row, d.amount, ETH_DECIMALS);
                        row.from_address = Some(addr_lower(&d.delegator));
                        row.to_address = Some(addr_lower(&d.recipient));
                    }),
                    Err(e) => return decode_failed(contract, "WithdrawFees", topic0, e),
                }
            } else if topic0 == BondingManager::TranscoderActivated::SIGNATURE_HASH {
                match BondingManager::TranscoderActivated::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("TranscoderActivated", {
                        row.to_address = Some(addr_lower(&d.transcoder));
                        if let Some(obj) = row.raw_event.as_object_mut() {
                            obj.insert(
                                "decoded".to_string(),
                                serde_json::json!({
                                    "transcoder": addr_lower(&d.transcoder),
                                    "activationRound": d.activationRound.to_string(),
                                }),
                            );
                        }
                    }),
                    Err(e) => return decode_failed(contract, "TranscoderActivated", topic0, e),
                }
            } else if topic0 == BondingManager::TranscoderDeactivated::SIGNATURE_HASH {
                match BondingManager::TranscoderDeactivated::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("TranscoderDeactivated", {
                        row.to_address = Some(addr_lower(&d.transcoder));
                        if let Some(obj) = row.raw_event.as_object_mut() {
                            obj.insert(
                                "decoded".to_string(),
                                serde_json::json!({
                                    "transcoder": addr_lower(&d.transcoder),
                                    "deactivationRound": d.deactivationRound.to_string(),
                                }),
                            );
                        }
                    }),
                    Err(e) => return decode_failed(contract, "TranscoderDeactivated", topic0, e),
                }
            } else if topic0 == BondingManager::TranscoderUpdate::SIGNATURE_HASH {
                match BondingManager::TranscoderUpdate::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("TranscoderUpdate", {
                        row.to_address = Some(addr_lower(&d.transcoder));
                        if let Some(obj) = row.raw_event.as_object_mut() {
                            obj.insert(
                                "decoded".to_string(),
                                serde_json::json!({
                                    "transcoder": addr_lower(&d.transcoder),
                                    "rewardCut": d.rewardCut.to_string(),
                                    "feeShare": d.feeShare.to_string(),
                                }),
                            );
                        }
                    }),
                    Err(e) => return decode_failed(contract, "TranscoderUpdate", topic0, e),
                }
            } else if topic0 == TranscoderSlashed::SIGNATURE_HASH {
                match TranscoderSlashed::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("TranscoderSlashed", {
                        // Slashing penalty is LPT removed from the transcoder's bonded stake.
                        // Strict-decode: slashing is monetary even if currently inactive
                        // (slashRate=0 on Livepeer today).
                        row.asset = Some("LPT");
                        row.is_valuable = true;
                        set_amount(&mut row, d.penalty, LPT_DECIMALS);
                        row.from_address = Some(addr_lower(&d.transcoder));
                        row.to_address = Some(addr_lower(&d.finder));
                        if let Some(obj) = row.raw_event.as_object_mut() {
                            obj.insert(
                                "decoded".to_string(),
                                serde_json::json!({
                                    "transcoder":    addr_lower(&d.transcoder),
                                    "finder":        addr_lower(&d.finder),
                                    "penalty":       d.penalty.to_string(),
                                    "finderReward":  d.finderReward.to_string(),
                                }),
                            );
                        }
                    }),
                    Err(e) => return decode_failed(contract, "TranscoderSlashed", topic0, e),
                }
            } else if topic0 == BondingManager::ParameterUpdate::SIGNATURE_HASH {
                match BondingManager::ParameterUpdate::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("ParameterUpdate", {
                        if let Some(obj) = row.raw_event.as_object_mut() {
                            obj.insert(
                                "decoded".to_string(),
                                serde_json::json!({ "param": d.param }),
                            );
                        }
                    }),
                    Err(e) => return decode_failed(contract, "ParameterUpdate", topic0, e),
                }
            } else if topic0 == BondingManager::SetController::SIGNATURE_HASH {
                match BondingManager::SetController::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("SetController", {
                        if let Some(obj) = row.raw_event.as_object_mut() {
                            obj.insert(
                                "decoded".to_string(),
                                serde_json::json!({ "controller": addr_lower(&d.controller) }),
                            );
                        }
                    }),
                    Err(e) => return decode_failed(contract, "SetController", topic0, e),
                }
            } else {
                return DispatchOutcome::UnknownTopic0 { topic0 };
            }
        }
        ContractKind::TicketBroker => {
            if topic0 == WinningTicketRedeemed::SIGNATURE_HASH {
                match WinningTicketRedeemed::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("WinningTicketRedeemed", {
                        row.asset = Some("ETH");
                        row.is_valuable = true;
                        set_amount(&mut row, d.faceValue, ETH_DECIMALS);
                        row.from_address = Some(addr_lower(&d.sender));
                        row.to_address = Some(addr_lower(&d.recipient));
                    }),
                    Err(e) => return decode_failed(contract, "WinningTicketRedeemed", topic0, e),
                }
            } else if topic0 == WinningTicketTransfer::SIGNATURE_HASH {
                match WinningTicketTransfer::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("WinningTicketTransfer", {
                        row.asset = Some("ETH");
                        row.is_valuable = true;
                        set_amount(&mut row, d.amount, ETH_DECIMALS);
                        row.from_address = Some(addr_lower(&d.sender));
                        row.to_address = Some(addr_lower(&d.recipient));
                    }),
                    Err(e) => return decode_failed(contract, "WinningTicketTransfer", topic0, e),
                }
            } else if topic0 == DepositFunded::SIGNATURE_HASH {
                match DepositFunded::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("DepositFunded", {
                        row.asset = Some("ETH");
                        row.is_valuable = true;
                        set_amount(&mut row, d.amount, ETH_DECIMALS);
                        row.from_address = Some(addr_lower(&d.sender));
                    }),
                    Err(e) => return decode_failed(contract, "DepositFunded", topic0, e),
                }
            } else if topic0 == ReserveFunded::SIGNATURE_HASH {
                match ReserveFunded::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("ReserveFunded", {
                        row.asset = Some("ETH");
                        row.is_valuable = true;
                        set_amount(&mut row, d.amount, ETH_DECIMALS);
                        row.from_address = Some(addr_lower(&d.reserveHolder));
                    }),
                    Err(e) => return decode_failed(contract, "ReserveFunded", topic0, e),
                }
            } else if topic0 == Withdrawal::SIGNATURE_HASH {
                match Withdrawal::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("Withdrawal", {
                        row.asset = Some("ETH");
                        row.is_valuable = true;
                        let total = d.deposit.saturating_add(d.reserve);
                        set_amount(&mut row, total, ETH_DECIMALS);
                        row.from_address = Some(addr_lower(&d.sender));
                    }),
                    Err(e) => return decode_failed(contract, "Withdrawal", topic0, e),
                }
            } else if topic0 == Unlock::SIGNATURE_HASH {
                // TD-031: previously decoded into an empty block, leaving
                // from_address NULL. The gateway-balance-backfill candidate
                // finder filters on `from_address IS NOT NULL`, so every
                // Unlock event was silently skipped and gateways stuck with
                // pre-unlock matview state.
                match Unlock::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("Unlock", {
                        row.from_address = Some(addr_lower(&d.sender));
                    }),
                    Err(e) => return decode_failed(contract, "Unlock", topic0, e),
                }
            } else if topic0 == ReserveClaimed::SIGNATURE_HASH {
                match ReserveClaimed::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("ReserveClaimed", {
                        // ETH from a broadcaster's reserve paid out to a claimant.
                        // Marked is_valuable=FALSE initially: we suspect WinningTicketRedeemed.faceValue
                        // already includes the reserve-drawn portion (so summing both would
                        // double-count). Capture amount for forward compatibility — flip
                        // is_valuable to TRUE post-backfill once the co-occurrence question
                        // is resolved.
                        row.asset = Some("ETH");
                        row.is_valuable = false;
                        set_amount(&mut row, d.amount, ETH_DECIMALS);
                        row.from_address = Some(addr_lower(&d.reserveHolder));
                        row.to_address = Some(addr_lower(&d.claimant));
                    }),
                    Err(e) => return decode_failed(contract, "ReserveClaimed", topic0, e),
                }
            } else if topic0 == UnlockCancelled::SIGNATURE_HASH {
                match UnlockCancelled::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("UnlockCancelled", {
                        row.from_address = Some(addr_lower(&d.sender));
                    }),
                    Err(e) => return decode_failed(contract, "UnlockCancelled", topic0, e),
                }
            } else if topic0 == TicketBroker::ParameterUpdate::SIGNATURE_HASH {
                match TicketBroker::ParameterUpdate::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("ParameterUpdate", {
                        if let Some(obj) = row.raw_event.as_object_mut() {
                            obj.insert(
                                "decoded".to_string(),
                                serde_json::json!({ "param": d.param }),
                            );
                        }
                    }),
                    Err(e) => return decode_failed(contract, "ParameterUpdate", topic0, e),
                }
            } else if topic0 == TicketBroker::SetController::SIGNATURE_HASH {
                match TicketBroker::SetController::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("SetController", {
                        if let Some(obj) = row.raw_event.as_object_mut() {
                            obj.insert(
                                "decoded".to_string(),
                                serde_json::json!({ "controller": addr_lower(&d.controller) }),
                            );
                        }
                    }),
                    Err(e) => return decode_failed(contract, "SetController", topic0, e),
                }
            } else {
                return DispatchOutcome::UnknownTopic0 { topic0 };
            }
        }
        ContractKind::LivepeerToken => {
            if topic0 == Transfer::SIGNATURE_HASH {
                match Transfer::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("Transfer", {
                        row.asset = Some("LPT");
                        row.is_valuable = true;
                        set_amount(&mut row, d.value, LPT_DECIMALS);
                        row.from_address = Some(addr_lower(&d.from));
                        row.to_address = Some(addr_lower(&d.to));
                    }),
                    Err(e) => return decode_failed(contract, "Transfer", topic0, e),
                }
            } else if topic0 == Mint::SIGNATURE_HASH {
                match Mint::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("Mint", {
                        row.asset = Some("LPT");
                        row.is_valuable = true;
                        set_amount(&mut row, d.amount, LPT_DECIMALS);
                        row.to_address = Some(addr_lower(&d.to));
                    }),
                    Err(e) => return decode_failed(contract, "Mint", topic0, e),
                }
            } else if topic0 == Burn::SIGNATURE_HASH {
                match Burn::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("Burn", {
                        row.asset = Some("LPT");
                        row.is_valuable = true;
                        set_amount(&mut row, d.value, LPT_DECIMALS);
                        row.from_address = Some(addr_lower(&d.burner));
                    }),
                    Err(e) => return decode_failed(contract, "Burn", topic0, e),
                }
            } else if topic0 == LivepeerToken::Approval::SIGNATURE_HASH {
                decoded!("Approval", {});
            } else {
                return DispatchOutcome::UnknownTopic0 { topic0 };
            }
        }
        ContractKind::RoundsManager => {
            if topic0 == NewRound::SIGNATURE_HASH {
                match NewRound::decode_log_data(&log_data, true) {
                    Ok(d) => {
                        row.event_name = "NewRound";
                        if let Some(obj) = row.raw_event.as_object_mut() {
                            obj.insert(
                                "decoded".to_string(),
                                serde_json::json!({
                                    "round":      d.round.to_string(),
                                    "blockHash":  format!("0x{:x}", d.blockHash),
                                }),
                            );
                        }
                        return DispatchOutcome::Decoded(row);
                    }
                    Err(e) => return decode_failed(contract, "NewRound", topic0, e),
                }
            } else if topic0 == RoundsManager::ParameterUpdate::SIGNATURE_HASH {
                match RoundsManager::ParameterUpdate::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("ParameterUpdate", {
                        if let Some(obj) = row.raw_event.as_object_mut() {
                            obj.insert(
                                "decoded".to_string(),
                                serde_json::json!({ "param": d.param }),
                            );
                        }
                    }),
                    Err(e) => return decode_failed(contract, "ParameterUpdate", topic0, e),
                }
            } else if topic0 == RoundsManager::SetController::SIGNATURE_HASH {
                match RoundsManager::SetController::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("SetController", {
                        if let Some(obj) = row.raw_event.as_object_mut() {
                            obj.insert(
                                "decoded".to_string(),
                                serde_json::json!({ "controller": addr_lower(&d.controller) }),
                            );
                        }
                    }),
                    Err(e) => return decode_failed(contract, "SetController", topic0, e),
                }
            } else {
                return DispatchOutcome::UnknownTopic0 { topic0 };
            }
        }
        ContractKind::Governor => {
            if topic0 == ProposalCreated::SIGNATURE_HASH {
                match ProposalCreated::decode_log_data(&log_data, true) {
                    Ok(d) => {
                        row.event_name = "ProposalCreated";
                        if let Some(obj) = row.raw_event.as_object_mut() {
                            obj.insert(
                                "decoded".to_string(),
                                serde_json::json!({
                                    "proposalId": d.proposalId.to_string(),
                                    "proposer":   addr_lower(&d.proposer),
                                    "voteStart":  d.voteStart.to_string(),
                                    "voteEnd":    d.voteEnd.to_string(),
                                    "description": d.description,
                                }),
                            );
                        }
                        return DispatchOutcome::Decoded(row);
                    }
                    Err(e) => return decode_failed(contract, "ProposalCreated", topic0, e),
                }
            } else if topic0 == ProposalExecuted::SIGNATURE_HASH {
                match ProposalExecuted::decode_log_data(&log_data, true) {
                    Ok(d) => {
                        row.event_name = "ProposalExecuted";
                        if let Some(obj) = row.raw_event.as_object_mut() {
                            obj.insert(
                                "decoded".to_string(),
                                serde_json::json!({ "proposalId": d.proposalId.to_string() }),
                            );
                        }
                        return DispatchOutcome::Decoded(row);
                    }
                    Err(e) => return decode_failed(contract, "ProposalExecuted", topic0, e),
                }
            } else if topic0 == VoteCast::SIGNATURE_HASH {
                match VoteCast::decode_log_data(&log_data, true) {
                    Ok(d) => {
                        row.event_name = "VoteCast";
                        if let Some(obj) = row.raw_event.as_object_mut() {
                            obj.insert(
                                "decoded".to_string(),
                                serde_json::json!({
                                    "voter":      addr_lower(&d.voter),
                                    "proposalId": d.proposalId.to_string(),
                                    "support":    d.support,
                                    "weight":     d.weight.to_string(),
                                    "reason":     d.reason,
                                }),
                            );
                        }
                        row.from_address = Some(addr_lower(&d.voter));
                        return DispatchOutcome::Decoded(row);
                    }
                    Err(e) => return decode_failed(contract, "VoteCast", topic0, e),
                }
            } else if topic0 == VoteCastWithParams::SIGNATURE_HASH {
                match VoteCastWithParams::decode_log_data(&log_data, true) {
                    Ok(d) => {
                        // OZ Governor's extended vote path (castVoteWithReasonAndParams).
                        // Same monetary semantics as VoteCast (none) but carries an extra
                        // params blob that we preserve in raw_event.decoded.
                        row.event_name = "VoteCastWithParams";
                        row.from_address = Some(addr_lower(&d.voter));
                        if let Some(obj) = row.raw_event.as_object_mut() {
                            obj.insert(
                                "decoded".to_string(),
                                serde_json::json!({
                                    "voter":      addr_lower(&d.voter),
                                    "proposalId": d.proposalId.to_string(),
                                    "support":    d.support,
                                    "weight":     d.weight.to_string(),
                                    "reason":     d.reason,
                                    "params":     format!("0x{}", alloy::hex::encode(&d.params)),
                                }),
                            );
                        }
                        return DispatchOutcome::Decoded(row);
                    }
                    Err(e) => return decode_failed(contract, "VoteCastWithParams", topic0, e),
                }
            } else if topic0 == ProposalCanceled::SIGNATURE_HASH {
                match ProposalCanceled::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("ProposalCanceled", {
                        if let Some(obj) = row.raw_event.as_object_mut() {
                            obj.insert(
                                "decoded".to_string(),
                                serde_json::json!({ "proposalId": d.proposalId.to_string() }),
                            );
                        }
                    }),
                    Err(e) => return decode_failed(contract, "ProposalCanceled", topic0, e),
                }
            } else if topic0 == ProposalQueued::SIGNATURE_HASH {
                match ProposalQueued::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("ProposalQueued", {
                        if let Some(obj) = row.raw_event.as_object_mut() {
                            obj.insert(
                                "decoded".to_string(),
                                serde_json::json!({
                                    "proposalId": d.proposalId.to_string(),
                                    "eta":        d.eta.to_string(),
                                }),
                            );
                        }
                    }),
                    Err(e) => return decode_failed(contract, "ProposalQueued", topic0, e),
                }
            } else if topic0 == Governor::ParameterUpdate::SIGNATURE_HASH {
                match Governor::ParameterUpdate::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("ParameterUpdate", {
                        if let Some(obj) = row.raw_event.as_object_mut() {
                            obj.insert(
                                "decoded".to_string(),
                                serde_json::json!({ "param": d.param }),
                            );
                        }
                    }),
                    Err(e) => return decode_failed(contract, "ParameterUpdate", topic0, e),
                }
            } else if topic0 == Governor::SetController::SIGNATURE_HASH {
                match Governor::SetController::decode_log_data(&log_data, true) {
                    Ok(d) => decoded!("SetController", {
                        if let Some(obj) = row.raw_event.as_object_mut() {
                            obj.insert(
                                "decoded".to_string(),
                                serde_json::json!({ "controller": addr_lower(&d.controller) }),
                            );
                        }
                    }),
                    Err(e) => return decode_failed(contract, "SetController", topic0, e),
                }
            } else {
                return DispatchOutcome::UnknownTopic0 { topic0 };
            }
        }
    }
}

fn decode_failed(
    contract: ContractKind,
    event_name: &'static str,
    topic0: B256,
    e: alloy::sol_types::Error,
) -> DispatchOutcome {
    DispatchOutcome::DecodeFailed {
        event_name,
        topic0,
        is_strict: is_strict_event(contract, topic0),
        error: e.to_string(),
    }
}

fn set_amount(row: &mut PreparedRow, amount: U256, decimals: u32) {
    let s = amount.to_string();
    let raw = BigDecimal::from_str(&s).unwrap_or_default();
    let normalized = raw.clone() / BigDecimal::from(10u128.pow(decimals));
    row.amount_raw = Some(raw);
    row.amount_normalized = Some(normalized);
}

fn addr_lower(a: &Address) -> String {
    format!("0x{:040x}", a)
}

fn topic0s_for(c: ContractKind) -> Vec<String> {
    let to_hex = |b: B256| format!("0x{:x}", b);
    match c {
        ContractKind::BondingManager => vec![
            to_hex(Reward::SIGNATURE_HASH),
            to_hex(Bond::SIGNATURE_HASH),
            to_hex(Unbond::SIGNATURE_HASH),
            to_hex(Rebond::SIGNATURE_HASH),
            to_hex(WithdrawStake::SIGNATURE_HASH),
            to_hex(TransferBond::SIGNATURE_HASH),
            to_hex(EarningsClaimed::SIGNATURE_HASH),
            to_hex(WithdrawFees::SIGNATURE_HASH),
            to_hex(BondingManager::TranscoderActivated::SIGNATURE_HASH),
            to_hex(BondingManager::TranscoderDeactivated::SIGNATURE_HASH),
            to_hex(BondingManager::TranscoderUpdate::SIGNATURE_HASH),
            to_hex(TranscoderSlashed::SIGNATURE_HASH),
            to_hex(BondingManager::ParameterUpdate::SIGNATURE_HASH),
            to_hex(BondingManager::SetController::SIGNATURE_HASH),
        ],
        ContractKind::TicketBroker => vec![
            to_hex(WinningTicketRedeemed::SIGNATURE_HASH),
            to_hex(WinningTicketTransfer::SIGNATURE_HASH),
            to_hex(DepositFunded::SIGNATURE_HASH),
            to_hex(ReserveFunded::SIGNATURE_HASH),
            to_hex(Withdrawal::SIGNATURE_HASH),
            to_hex(TicketBroker::Unlock::SIGNATURE_HASH),
            to_hex(ReserveClaimed::SIGNATURE_HASH),
            to_hex(UnlockCancelled::SIGNATURE_HASH),
            to_hex(TicketBroker::ParameterUpdate::SIGNATURE_HASH),
            to_hex(TicketBroker::SetController::SIGNATURE_HASH),
        ],
        ContractKind::LivepeerToken => vec![
            to_hex(Transfer::SIGNATURE_HASH),
            to_hex(LivepeerToken::Approval::SIGNATURE_HASH),
            to_hex(Mint::SIGNATURE_HASH),
            to_hex(Burn::SIGNATURE_HASH),
        ],
        ContractKind::RoundsManager => vec![
            to_hex(NewRound::SIGNATURE_HASH),
            to_hex(RoundsManager::ParameterUpdate::SIGNATURE_HASH),
            to_hex(RoundsManager::SetController::SIGNATURE_HASH),
        ],
        ContractKind::Governor => vec![
            to_hex(ProposalCreated::SIGNATURE_HASH),
            to_hex(ProposalExecuted::SIGNATURE_HASH),
            to_hex(VoteCast::SIGNATURE_HASH),
            to_hex(VoteCastWithParams::SIGNATURE_HASH),
            to_hex(ProposalCanceled::SIGNATURE_HASH),
            to_hex(ProposalQueued::SIGNATURE_HASH),
            to_hex(Governor::ParameterUpdate::SIGNATURE_HASH),
            to_hex(Governor::SetController::SIGNATURE_HASH),
        ],
    }
}

async fn eth_get_logs_multi_topic(
    pg: &PgPool,
    p: &Provider,
    contract: &str,
    topic0s: &[String],
    from_block: u64,
    to_block: u64,
) -> Result<serde_json::Value> {
    let params = serde_json::json!([{
        "address": contract,
        "topics": [topic0s],
        "fromBlock": format!("0x{:x}", from_block),
        "toBlock":   format!("0x{:x}", to_block),
    }]);
    let outcome = cross_check::single_call_cached(pg, p, "eth_getLogs", &params, None).await?;
    Ok(serde_json::from_slice(&outcome.response_bytes)?)
}

async fn advance_checkpoint(
    tx: &mut Transaction<'_, Postgres>,
    contract: ContractKind,
    suffix: &str,
    to_block: u64,
) -> Result<()> {
    let name = checkpoint_name(contract, suffix);
    sqlx::query(
        r#"INSERT INTO indexer_checkpoints
                (name, chain_id, last_processed_block, updated_at)
           VALUES ($1, $2, $3, now())
           ON CONFLICT (name) DO UPDATE
              SET last_processed_block = GREATEST(indexer_checkpoints.last_processed_block, EXCLUDED.last_processed_block),
                  updated_at = now()"#,
    )
    .bind(&name)
    .bind(ARBITRUM_CHAIN_ID)
    .bind(to_block as i64)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn build_log_data(raw: &RawLog) -> Result<LogData> {
    let topics_b256: Vec<FixedBytes<32>> = raw
        .topics
        .iter()
        .map(|t| FixedBytes::<32>::from_str(t.trim_start_matches("0x")))
        .collect::<std::result::Result<_, _>>()
        .context("decoding topic bytes")?;
    let data_bytes =
        alloy::hex::decode(raw.data.trim_start_matches("0x")).context("decoding data hex")?;
    LogData::new(topics_b256, data_bytes.into()).context("malformed LogData (topics/data shape)")
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RawLog {
    address: String,
    topics: Vec<String>,
    data: String,
    block_number: String,
    block_hash: String,
    transaction_hash: String,
    log_index: String,
}

#[derive(Debug)]
struct PreparedRow {
    tx_hash: String,
    log_index: i32,
    block_number: i64,
    block_hash: String,
    block_timestamp: DateTime<chrono::Utc>,
    contract_address: String,
    contract_name: &'static str,
    event_name: &'static str,
    event_signature: String,
    asset: Option<&'static str>,
    amount_raw: Option<BigDecimal>,
    amount_normalized: Option<BigDecimal>,
    is_valuable: bool,
    from_address: Option<String>,
    to_address: Option<String>,
    raw_event: serde_json::Value,
}

fn u64_from_hex(s: &str) -> Result<u64> {
    Ok(u64::from_str_radix(s.trim_start_matches("0x"), 16)?)
}
fn u32_from_hex(s: &str) -> Result<u32> {
    Ok(u32::from_str_radix(s.trim_start_matches("0x"), 16)?)
}
fn i64_from_hex(s: &str) -> Result<i64> {
    Ok(i64::from_str_radix(s.trim_start_matches("0x"), 16)?)
}

#[derive(Debug, Clone)]
struct DeadLetterRow {
    block_number: i64,
    block_hash: String,
    tx_hash: String,
    log_index: i32,
    contract_address: String,
    topics: Vec<String>,
    data: Vec<u8>,
    attempted_abi_hash: String,
    error_message: String,
}

impl DeadLetterRow {
    fn from_raw(raw: &RawLog, abi_hash: &str, error_message: &str) -> Result<Self> {
        let block_number = u64_from_hex(&raw.block_number)? as i64;
        let log_index = u32_from_hex(&raw.log_index)? as i32;
        let data = alloy::hex::decode(raw.data.trim_start_matches("0x"))
            .context("decoding data hex for dead-letter")?;
        Ok(DeadLetterRow {
            block_number,
            block_hash: raw.block_hash.to_lowercase(),
            tx_hash: raw.transaction_hash.to_lowercase(),
            log_index,
            contract_address: raw.address.to_lowercase(),
            topics: raw.topics.iter().map(|t| t.to_lowercase()).collect(),
            data,
            attempted_abi_hash: abi_hash.to_string(),
            error_message: error_message.to_string(),
        })
    }
}

// Suppress an unused-import warning; alloy::primitives::Address is referenced by addr_lower
// and by some sol!-generated structs but rustc occasionally flags it under cfg flags.
#[allow(dead_code)]
fn _addr_assert(a: Address) -> Address {
    a
}
