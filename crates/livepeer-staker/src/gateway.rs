use alloy::primitives::{Address, U256};
use alloy::sol;
use alloy::sol_types::SolCall;
use anyhow::{Context, Result};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use livepeer_core::{
    rpc::{cross_check, BlockTag, Provider},
    Config,
};
use serde::Serialize;
use serde_json::json;
use sqlx::{PgPool, Row};
use std::collections::HashSet;
use std::str::FromStr;
use tracing::info;

sol!(
    #[allow(missing_docs)]
    TicketBroker,
    "../../abi/TicketBroker.json"
);

const ARBITRUM_CHAIN_ID: i64 = 42161;
const ETH_DECIMALS: u32 = 18;
const SOURCE_RPC_RECONCILED: &str = "rpc_reconciled";

#[derive(Debug, Default, Serialize)]
pub struct GatewayBackfillSummary {
    pub candidates_seen: u64,
    pub rows_written: u64,
    pub gateways_touched: u64,
}

#[derive(Debug)]
struct GatewayCandidate {
    event_id: i64,
    gateway_address: String,
    block_number: i64,
    block_timestamp: DateTime<Utc>,
    block_hash: String,
}

pub async fn run_gateway_backfill(
    pg: &PgPool,
    archive: &Provider,
    cfg: &Config,
    include_tentative: bool,
) -> Result<GatewayBackfillSummary> {
    let candidates = fetch_candidates(pg, include_tentative).await?;
    info!(candidates = candidates.len(), "gateway backfill starting");

    let mut summary = GatewayBackfillSummary {
        candidates_seen: candidates.len() as u64,
        ..Default::default()
    };
    let mut gateways = HashSet::new();
    for candidate in candidates {
        let snapshot = read_gateway_state(
            pg,
            archive,
            &cfg.static_.contracts.ticket_broker.to_lowercase(),
            &candidate.gateway_address,
            candidate.block_number,
        )
        .await?;
        upsert_gateway_row(pg, &candidate, &snapshot).await?;
        gateways.insert(candidate.gateway_address);
        summary.rows_written += 1;
    }
    summary.gateways_touched = gateways.len() as u64;
    info!(?summary, "gateway backfill complete");
    Ok(summary)
}

#[derive(Debug)]
struct GatewaySnapshot {
    deposit: BigDecimal,
    reserve_funds_remaining: BigDecimal,
    reserve_claimed_in_current_round: BigDecimal,
    withdraw_round: i64,
    unlock_in_progress: bool,
    raw_call: serde_json::Value,
}

async fn fetch_candidates(pg: &PgPool, include_tentative: bool) -> Result<Vec<GatewayCandidate>> {
    let finality_filter = if include_tentative {
        ""
    } else {
        "AND r.finality = 'finalized'"
    };
    let sql = format!(
        r#"WITH latest_touch AS (
               SELECT DISTINCT ON (r.from_address, r.block_number)
                      r.id AS event_id,
                      r.from_address AS gateway_address,
                      r.block_number,
                      r.block_timestamp,
                      r.block_hash
                 FROM raw_protocol_events r
                WHERE r.chain_id = $1
                  AND r.is_canonical = TRUE
                  AND r.contract_name = 'TicketBroker'
                  AND r.from_address IS NOT NULL
                  AND r.event_name IN (
                        'DepositFunded',
                        'ReserveFunded',
                        'WinningTicketTransfer',
                        'WinningTicketRedeemed',
                        'ReserveClaimed',
                        'Withdrawal',
                        'Unlock',
                        'UnlockCancelled'
                  )
                  {finality_filter}
                ORDER BY r.from_address, r.block_number, r.log_index DESC
           )
           SELECT l.event_id, l.gateway_address, l.block_number, l.block_timestamp, l.block_hash
             FROM latest_touch l
             LEFT JOIN gateway_balances_by_block g
               ON g.chain_id = $1
              AND g.gateway_address = l.gateway_address
              AND g.block_number = l.block_number
            WHERE g.gateway_address IS NULL
            ORDER BY l.block_number ASC, l.gateway_address ASC"#,
    );
    let rows = sqlx::query(&sql)
        .bind(ARBITRUM_CHAIN_ID)
        .fetch_all(pg)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| GatewayCandidate {
            event_id: r.get("event_id"),
            gateway_address: r.get("gateway_address"),
            block_number: r.get("block_number"),
            block_timestamp: r.get("block_timestamp"),
            block_hash: r.get("block_hash"),
        })
        .collect())
}

async fn read_gateway_state(
    pg: &PgPool,
    archive: &Provider,
    ticket_broker: &str,
    gateway: &str,
    block_number: i64,
) -> Result<GatewaySnapshot> {
    let gateway_addr = Address::from_str(gateway).context("parsing gateway address")?;

    let sender_data = format!(
        "0x{}",
        alloy::hex::encode(
            TicketBroker::getSenderInfoCall {
                _sender: gateway_addr
            }
            .abi_encode()
        )
    );
    let sender_params = json!([
        { "to": ticket_broker, "data": sender_data },
        BlockTag::Number(block_number as u64).to_param()
    ]);
    let sender_outcome = cross_check::single_call_cached(
        pg,
        archive,
        "eth_call",
        &sender_params,
        Some(block_number),
    )
    .await?;
    let sender_raw = decode_hex_result(&sender_outcome.response_bytes)?;
    let sender = TicketBroker::getSenderInfoCall::abi_decode_returns(&sender_raw, true)?;

    let unlock_data = format!(
        "0x{}",
        alloy::hex::encode(
            TicketBroker::isUnlockInProgressCall {
                _sender: gateway_addr,
            }
            .abi_encode()
        )
    );
    let unlock_params = json!([
        { "to": ticket_broker, "data": unlock_data },
        BlockTag::Number(block_number as u64).to_param()
    ]);
    let unlock_outcome = cross_check::single_call_cached(
        pg,
        archive,
        "eth_call",
        &unlock_params,
        Some(block_number),
    )
    .await?;
    let unlock_raw = decode_hex_result(&unlock_outcome.response_bytes)?;
    let unlock = TicketBroker::isUnlockInProgressCall::abi_decode_returns(&unlock_raw, true)?;

    Ok(GatewaySnapshot {
        deposit: u256_to_decimal(&sender.sender.deposit, ETH_DECIMALS),
        reserve_funds_remaining: u256_to_decimal(&sender.reserve.fundsRemaining, ETH_DECIMALS),
        reserve_claimed_in_current_round: u256_to_decimal(
            &sender.reserve.claimedInCurrentRound,
            ETH_DECIMALS,
        ),
        withdraw_round: sender.sender.withdrawRound.try_into().unwrap_or(i64::MAX),
        unlock_in_progress: unlock._0,
        raw_call: json!({
            "getSenderInfo_call_hash": sender_outcome.call_hash,
            "isUnlockInProgress_call_hash": unlock_outcome.call_hash,
        }),
    })
}

async fn upsert_gateway_row(
    pg: &PgPool,
    candidate: &GatewayCandidate,
    snapshot: &GatewaySnapshot,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO gateway_balances_by_block (
               chain_id, gateway_address, block_number, block_timestamp, block_hash,
               deposit, reserve_funds_remaining, reserve_claimed_in_current_round,
               withdraw_round, unlock_in_progress, source, raw_call, triggering_event_id
           ) VALUES (
               $1, $2, $3, $4, $5,
               $6, $7, $8,
               $9, $10, $11, $12, $13
           )
           ON CONFLICT (chain_id, gateway_address, block_number) DO UPDATE
               SET block_timestamp = EXCLUDED.block_timestamp,
                   block_hash = EXCLUDED.block_hash,
                   deposit = EXCLUDED.deposit,
                   reserve_funds_remaining = EXCLUDED.reserve_funds_remaining,
                   reserve_claimed_in_current_round = EXCLUDED.reserve_claimed_in_current_round,
                   withdraw_round = EXCLUDED.withdraw_round,
                   unlock_in_progress = EXCLUDED.unlock_in_progress,
                   source = EXCLUDED.source,
                   raw_call = EXCLUDED.raw_call,
                   triggering_event_id = EXCLUDED.triggering_event_id"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(&candidate.gateway_address)
    .bind(candidate.block_number)
    .bind(candidate.block_timestamp)
    .bind(&candidate.block_hash)
    .bind(&snapshot.deposit)
    .bind(&snapshot.reserve_funds_remaining)
    .bind(&snapshot.reserve_claimed_in_current_round)
    .bind(snapshot.withdraw_round)
    .bind(snapshot.unlock_in_progress)
    .bind(SOURCE_RPC_RECONCILED)
    .bind(&snapshot.raw_call)
    .bind(candidate.event_id)
    .execute(pg)
    .await?;
    Ok(())
}

fn decode_hex_result(bytes: &[u8]) -> Result<Vec<u8>> {
    let s = std::str::from_utf8(bytes).unwrap_or_default();
    let hex_str = s.trim_matches('"').trim_start_matches("0x");
    alloy::hex::decode(hex_str).context("decoding eth_call return hex")
}

fn u256_to_decimal(u: &U256, decimals: u32) -> BigDecimal {
    let raw = BigDecimal::from_str(&u.to_string()).unwrap_or_default();
    raw / BigDecimal::from(10u128.pow(decimals))
}
