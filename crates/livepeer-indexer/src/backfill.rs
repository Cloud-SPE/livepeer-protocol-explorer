//! Generalized log backfill — fetch by (contract, all topic0s of interest), dispatch
//! by topic0 to typed decoders, atomic batch insert + checkpoint advance.
//!
//! S6.2 covers BondingManager, TicketBroker, LivepeerToken (full v1 valued catalog +
//! TransferBond/WithdrawFees added in SPEC v1.1). Non-monetary events under each
//! contract land here too — they get is_valuable=false and amount=NULL.
//!
//! S6.3 will add strict-decode allowlist halt vs dead-letter. S6.4 will add the
//! dynamic-batch backfill driver that walks [from, to] in chunks.

use crate::events::BondingManager::{
    self, Bond, EarningsClaimed, Rebond, Reward, TransferBond, Unbond,
    WithdrawStake,
};
// BondingManager has two WithdrawFees overloads. _0 carries (delegator, recipient, amount);
// _1 is the legacy form with just (delegator) and no amount. We bind _0 explicitly.
use crate::events::BondingManager::WithdrawFees_0 as WithdrawFees;
use crate::events::LivepeerToken::{self, Transfer};
// TicketBroker emits "Withdrawal" (full deposit + reserve drain) — not "Withdraw".
use crate::events::TicketBroker::{
    self, DepositFunded, ReserveFunded, Withdrawal, WinningTicketRedeemed,
    WinningTicketTransfer,
};
use alloy::primitives::{Address, FixedBytes, LogData, B256, U256};
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
const BATCH_INSERT_SIZE: usize = 500;
const LPT_DECIMALS: u32 = 18;
const ETH_DECIMALS: u32 = 18;

/// Which contract to back-fill. Maps to the proxy address from config and the abi_hash
/// from the registry. Each contract has a fixed list of topic0s of interest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractKind {
    BondingManager,
    TicketBroker,
    LivepeerToken,
}

impl ContractKind {
    pub fn name(self) -> &'static str {
        match self {
            ContractKind::BondingManager => "BondingManager",
            ContractKind::TicketBroker => "TicketBroker",
            ContractKind::LivepeerToken => "LivepeerToken",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "BondingManager" => Some(Self::BondingManager),
            "TicketBroker" => Some(Self::TicketBroker),
            "LivepeerToken" => Some(Self::LivepeerToken),
            _ => None,
        }
    }
}

/// Backfill all known events from `contract` in `[from_block, to_block]` (inclusive).
pub async fn backfill_contract(
    pg: &PgPool,
    archive: &Provider,
    contract: ContractKind,
    proxy_address: &str,
    abi_hash: &str,
    from_block: u64,
    to_block: u64,
) -> Result<u64> {
    let topic0s = topic0s_for(contract);
    info!(
        contract = contract.name(),
        proxy = proxy_address,
        from_block,
        to_block,
        topics_of_interest = topic0s.len(),
        "fetching logs"
    );

    let logs_value = eth_get_logs_multi_topic(archive, proxy_address, &topic0s, from_block, to_block)
        .await?;
    let raw_logs: Vec<RawLog> =
        serde_json::from_value(logs_value).context("decoding eth_getLogs response")?;
    info!(count = raw_logs.len(), "logs fetched");

    if raw_logs.is_empty() {
        advance_checkpoint(pg, to_block).await?;
        return Ok(0);
    }

    // Resolve unique block timestamps via the cross_check cache.
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
    info!(blocks_resolved = block_ts.len(), "block timestamps cached");

    // Decode + transform each log into a row.
    let mut prepared: Vec<PreparedRow> = Vec::with_capacity(raw_logs.len());
    let mut undecoded = 0u64;
    for raw in &raw_logs {
        match decode_one(contract, raw, &block_ts, abi_hash)? {
            Some(row) => prepared.push(row),
            None => undecoded += 1,
        }
    }
    if undecoded > 0 {
        warn!(undecoded, "logs with no matching event decoder — skipped (S6.3 will dead-letter these)");
    }

    // Atomic batch insert + checkpoint advance.
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
    Ok(inserted)
}

// ---------------------------------------------------------------------------
// Per-event decoders (dispatch by topic0)
// ---------------------------------------------------------------------------

fn decode_one(
    contract: ContractKind,
    raw: &RawLog,
    block_ts: &HashMap<u64, i64>,
    _abi_hash: &str,
) -> Result<Option<PreparedRow>> {
    let topic0_str = raw
        .topics
        .first()
        .context("log missing topic0")?
        .to_lowercase();
    let topic0 = FixedBytes::<32>::from_str(topic0_str.trim_start_matches("0x"))
        .context("decoding topic0 hex")?;
    let log_data = build_log_data(raw)?;
    let block_number = u64_from_hex(&raw.block_number)?;
    let log_index = u32_from_hex(&raw.log_index)?;
    let ts_secs = *block_ts
        .get(&block_number)
        .context("missing block timestamp")?;
    let block_timestamp = DateTime::from_timestamp(ts_secs, 0).context("invalid timestamp")?;
    let raw_event = serde_json::to_value(raw)?;
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

    match contract {
        ContractKind::BondingManager => {
            if topic0 == Reward::SIGNATURE_HASH {
                let d = Reward::decode_log_data(&log_data, true)?;
                row.event_name = "Reward";
                row.asset = Some("LPT");
                row.is_valuable = true;
                set_amount(&mut row, d.amount, LPT_DECIMALS);
                row.to_address = Some(addr_lower(&d.transcoder));
            } else if topic0 == Bond::SIGNATURE_HASH {
                let d = Bond::decode_log_data(&log_data, true)?;
                row.event_name = "Bond";
                row.asset = Some("LPT");
                row.is_valuable = true;
                // SPEC §6.3 — additionalAmount is the per-event LPT inflow, NOT bondedAmount.
                set_amount(&mut row, d.additionalAmount, LPT_DECIMALS);
                row.from_address = Some(addr_lower(&d.delegator));
                row.to_address = Some(addr_lower(&d.newDelegate));
            } else if topic0 == Unbond::SIGNATURE_HASH {
                let d = Unbond::decode_log_data(&log_data, true)?;
                row.event_name = "Unbond";
                row.asset = Some("LPT");
                row.is_valuable = true;
                set_amount(&mut row, d.amount, LPT_DECIMALS);
                row.from_address = Some(addr_lower(&d.delegator));
                row.to_address = Some(addr_lower(&d.delegate));
            } else if topic0 == Rebond::SIGNATURE_HASH {
                let d = Rebond::decode_log_data(&log_data, true)?;
                row.event_name = "Rebond";
                row.asset = Some("LPT");
                row.is_valuable = true;
                set_amount(&mut row, d.amount, LPT_DECIMALS);
                row.from_address = Some(addr_lower(&d.delegator));
                row.to_address = Some(addr_lower(&d.delegate));
            } else if topic0 == WithdrawStake::SIGNATURE_HASH {
                let d = WithdrawStake::decode_log_data(&log_data, true)?;
                row.event_name = "WithdrawStake";
                row.asset = Some("LPT");
                row.is_valuable = true;
                set_amount(&mut row, d.amount, LPT_DECIMALS);
                row.from_address = Some(addr_lower(&d.delegator));
            } else if topic0 == TransferBond::SIGNATURE_HASH {
                let d = TransferBond::decode_log_data(&log_data, true)?;
                row.event_name = "TransferBond";
                row.asset = Some("LPT");
                row.is_valuable = true;
                set_amount(&mut row, d.amount, LPT_DECIMALS);
                row.from_address = Some(addr_lower(&d.oldDelegator));
                row.to_address = Some(addr_lower(&d.newDelegator));
            } else if topic0 == EarningsClaimed::SIGNATURE_HASH {
                // Multi-asset per SPEC §6.8 — one raw_protocol_events row with asset=NULL.
                // The valuator splits it into two event_valuations rows (LPT + ETH).
                let d = EarningsClaimed::decode_log_data(&log_data, true)?;
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
                            "rewards": d.rewards.to_string(),
                            "fees":    d.fees.to_string(),
                            "startRound": d.startRound.to_string(),
                            "endRound":   d.endRound.to_string(),
                        }),
                    );
                }
            } else if topic0 == WithdrawFees::SIGNATURE_HASH {
                let d = WithdrawFees::decode_log_data(&log_data, true)?;
                row.event_name = "WithdrawFees";
                row.asset = Some("ETH");
                row.is_valuable = true;
                set_amount(&mut row, d.amount, ETH_DECIMALS);
                row.from_address = Some(addr_lower(&d.delegator));
                row.to_address = Some(addr_lower(&d.recipient));
            } else if topic0 == BondingManager::TranscoderActivated::SIGNATURE_HASH {
                row.event_name = "TranscoderActivated";
            } else if topic0 == BondingManager::TranscoderDeactivated::SIGNATURE_HASH {
                row.event_name = "TranscoderDeactivated";
            } else if topic0 == BondingManager::TranscoderUpdate::SIGNATURE_HASH {
                row.event_name = "TranscoderUpdate";
            } else {
                return Ok(None);
            }
        }
        ContractKind::TicketBroker => {
            if topic0 == WinningTicketRedeemed::SIGNATURE_HASH {
                let d = WinningTicketRedeemed::decode_log_data(&log_data, true)?;
                row.event_name = "WinningTicketRedeemed";
                row.asset = Some("ETH");
                row.is_valuable = true;
                set_amount(&mut row, d.faceValue, ETH_DECIMALS);
                row.from_address = Some(addr_lower(&d.sender));
                row.to_address = Some(addr_lower(&d.recipient));
            } else if topic0 == WinningTicketTransfer::SIGNATURE_HASH {
                let d = WinningTicketTransfer::decode_log_data(&log_data, true)?;
                row.event_name = "WinningTicketTransfer";
                row.asset = Some("ETH");
                row.is_valuable = true;
                set_amount(&mut row, d.amount, ETH_DECIMALS);
                row.from_address = Some(addr_lower(&d.sender));
                row.to_address = Some(addr_lower(&d.recipient));
            } else if topic0 == DepositFunded::SIGNATURE_HASH {
                let d = DepositFunded::decode_log_data(&log_data, true)?;
                row.event_name = "DepositFunded";
                row.asset = Some("ETH");
                row.is_valuable = true;
                set_amount(&mut row, d.amount, ETH_DECIMALS);
                row.from_address = Some(addr_lower(&d.sender));
            } else if topic0 == ReserveFunded::SIGNATURE_HASH {
                let d = ReserveFunded::decode_log_data(&log_data, true)?;
                row.event_name = "ReserveFunded";
                row.asset = Some("ETH");
                row.is_valuable = true;
                set_amount(&mut row, d.amount, ETH_DECIMALS);
                row.from_address = Some(addr_lower(&d.reserveHolder));
            } else if topic0 == Withdrawal::SIGNATURE_HASH {
                let d = Withdrawal::decode_log_data(&log_data, true)?;
                row.event_name = "Withdrawal";
                row.asset = Some("ETH");
                row.is_valuable = true;
                // Withdrawal on TicketBroker drains both deposit and reserve; treat the sum as amount.
                let total = d.deposit.saturating_add(d.reserve);
                set_amount(&mut row, total, ETH_DECIMALS);
                row.from_address = Some(addr_lower(&d.sender));
            } else if topic0 == TicketBroker::Unlock::SIGNATURE_HASH {
                row.event_name = "Unlock";
            } else {
                return Ok(None);
            }
        }
        ContractKind::LivepeerToken => {
            if topic0 == Transfer::SIGNATURE_HASH {
                let d = Transfer::decode_log_data(&log_data, true)?;
                row.event_name = "Transfer";
                row.asset = Some("LPT");
                row.is_valuable = true;
                set_amount(&mut row, d.value, LPT_DECIMALS);
                row.from_address = Some(addr_lower(&d.from));
                row.to_address = Some(addr_lower(&d.to));
            } else if topic0 == LivepeerToken::Approval::SIGNATURE_HASH {
                row.event_name = "Approval";
            } else {
                return Ok(None);
            }
        }
    }

    Ok(Some(row))
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
        ],
        ContractKind::TicketBroker => vec![
            to_hex(WinningTicketRedeemed::SIGNATURE_HASH),
            to_hex(WinningTicketTransfer::SIGNATURE_HASH),
            to_hex(DepositFunded::SIGNATURE_HASH),
            to_hex(ReserveFunded::SIGNATURE_HASH),
            to_hex(Withdrawal::SIGNATURE_HASH),
            to_hex(TicketBroker::Unlock::SIGNATURE_HASH),
        ],
        ContractKind::LivepeerToken => vec![
            to_hex(Transfer::SIGNATURE_HASH),
            to_hex(LivepeerToken::Approval::SIGNATURE_HASH),
        ],
    }
}

async fn eth_get_logs_multi_topic(
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
    Ok(p.call("eth_getLogs", &params).await?)
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
