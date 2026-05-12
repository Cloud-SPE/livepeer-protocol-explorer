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
const GATEWAY_FLOW_BATCH_LIMIT: i64 = 2_000;
const GATEWAY_CLAIMANT_BATCH_LIMIT: i64 = 1_000;
const GATEWAY_BALANCE_BATCH_LIMIT: i64 = 250;
const GATEWAY_FLOW_CHECKPOINT: &str = "gateway_flow_backfill";
const GATEWAY_CLAIMANT_CHECKPOINT: &str = "gateway_claimant_backfill";
const GATEWAY_BALANCE_CHECKPOINT: &str = "gateway_balance_backfill";

#[derive(Debug, Default, Serialize)]
pub struct GatewayBackfillSummary {
    pub balance_candidates_seen: u64,
    pub balance_rows_written: u64,
    pub flow_candidates_seen: u64,
    pub flow_rows_written: u64,
    pub claimant_candidates_seen: u64,
    pub claimant_rows_written: u64,
    pub gateways_touched: u64,
    pub claimants_touched: u64,
    pub flow_checkpoint_block: Option<i64>,
    pub claimant_checkpoint_block: Option<i64>,
    pub balance_checkpoint_block: Option<i64>,
}

#[derive(Debug)]
struct GatewayCandidate {
    event_id: i64,
    gateway_address: String,
    block_number: i64,
    block_timestamp: DateTime<Utc>,
    block_hash: String,
}

#[derive(Debug)]
struct GatewayFlowCandidate {
    event_id: i64,
    gateway_address: String,
    claimant_address: Option<String>,
    counterparty_address: Option<String>,
    event_name: String,
    flow_kind: String,
    block_number: i64,
    block_timestamp: DateTime<Utc>,
    tx_hash: String,
    log_index: i32,
    asset: Option<String>,
    amount_native: Option<BigDecimal>,
    amount_usd: Option<BigDecimal>,
    valuation_version: Option<String>,
    block_hash: String,
}

pub async fn run_gateway_backfill(
    pg: &PgPool,
    archive: &Provider,
    cfg: &Config,
    include_tentative: bool,
) -> Result<GatewayBackfillSummary> {
    let started = std::time::Instant::now();
    let flow_checkpoint = load_gateway_checkpoint(pg, GATEWAY_FLOW_CHECKPOINT).await?;
    let claimant_checkpoint = load_gateway_checkpoint(pg, GATEWAY_CLAIMANT_CHECKPOINT).await?;
    let balance_checkpoint = load_gateway_checkpoint(pg, GATEWAY_BALANCE_CHECKPOINT).await?;

    let flow_candidates = fetch_flow_candidates_after(
        pg,
        include_tentative,
        flow_checkpoint,
        GATEWAY_FLOW_BATCH_LIMIT,
    )
    .await?;
    let claimant_candidates = fetch_claimant_candidates_after(
        pg,
        include_tentative,
        claimant_checkpoint,
        GATEWAY_CLAIMANT_BATCH_LIMIT,
    )
    .await?;
    let balance_candidates = fetch_balance_candidates_after(
        pg,
        include_tentative,
        balance_checkpoint,
        GATEWAY_BALANCE_BATCH_LIMIT,
    )
    .await?;
    let initial_balance_candidates = balance_candidates.len() as i64;
    let initial_flow_candidates = flow_candidates.len() as i64;
    let initial_claimant_candidates = claimant_candidates.len() as i64;
    info!(
        flow_checkpoint,
        claimant_checkpoint,
        balance_checkpoint,
        balance_candidates = initial_balance_candidates,
        flow_candidates = initial_flow_candidates,
        claimant_candidates = initial_claimant_candidates,
        "gateway backfill starting"
    );

    let mut summary = GatewayBackfillSummary {
        balance_candidates_seen: balance_candidates.len() as u64,
        flow_candidates_seen: flow_candidates.len() as u64,
        claimant_candidates_seen: claimant_candidates.len() as u64,
        flow_checkpoint_block: flow_checkpoint,
        claimant_checkpoint_block: claimant_checkpoint,
        balance_checkpoint_block: balance_checkpoint,
        ..Default::default()
    };
    let mut flow_touched = None;
    let mut claimant_touched = None;
    let mut gateways = HashSet::new();
    let mut claimants = HashSet::new();

    for candidate in flow_candidates {
        upsert_gateway_flow(pg, &candidate).await?;
        summary.flow_rows_written += 1;
        flow_touched = Some(candidate.block_number);
        gateways.insert(candidate.gateway_address.clone());
    }

    if let Some(block) = flow_touched {
        advance_gateway_checkpoint(pg, GATEWAY_FLOW_CHECKPOINT, block).await?;
        summary.flow_checkpoint_block = Some(block);
    }

    for candidate in claimant_candidates {
        let claimant = match candidate.claimant_address.as_ref() {
            Some(v) => v,
            None => continue,
        };
        let claimant_snapshot = read_claimant_state(
            pg,
            archive,
            &cfg.static_.contracts.ticket_broker.to_lowercase(),
            &candidate.gateway_address,
            claimant,
            candidate.block_number,
        )
        .await?;
        upsert_gateway_claimant_row(pg, &candidate, &claimant_snapshot).await?;
        summary.claimant_rows_written += 1;
        claimant_touched = Some(candidate.block_number);
        claimants.insert((candidate.gateway_address.clone(), claimant.to_string()));
    }

    if let Some(block) = claimant_touched {
        advance_gateway_checkpoint(pg, GATEWAY_CLAIMANT_CHECKPOINT, block).await?;
        summary.claimant_checkpoint_block = Some(block);
    }

    for candidate in balance_candidates {
        let snapshot = read_gateway_state(
            pg,
            archive,
            &cfg.static_.contracts.ticket_broker.to_lowercase(),
            &candidate.gateway_address,
            candidate.block_number,
        )
        .await?;
        upsert_gateway_row(pg, &candidate, &snapshot).await?;
        summary.balance_rows_written += 1;
        summary.balance_checkpoint_block = Some(candidate.block_number);
        gateways.insert(candidate.gateway_address);
    }

    if let Some(block) = summary.balance_checkpoint_block {
        advance_gateway_checkpoint(pg, GATEWAY_BALANCE_CHECKPOINT, block).await?;
    }

    summary.gateways_touched = gateways.len() as u64;
    summary.claimants_touched = claimants.len() as u64;

    let elapsed = started.elapsed();
    info!(
        ?summary,
        elapsed_ms = elapsed.as_millis() as u64,
        "gateway backfill complete"
    );

    crate::metrics::record_gateway_iteration(crate::metrics::GatewayIterationRecord {
        balance: crate::metrics::GatewayAxisRecord {
            candidates: initial_balance_candidates,
            rows_written: summary.balance_rows_written,
            checkpoint_block: summary.balance_checkpoint_block,
        },
        flow: crate::metrics::GatewayAxisRecord {
            candidates: initial_flow_candidates,
            rows_written: summary.flow_rows_written,
            checkpoint_block: summary.flow_checkpoint_block,
        },
        claimant: crate::metrics::GatewayAxisRecord {
            candidates: initial_claimant_candidates,
            rows_written: summary.claimant_rows_written,
            checkpoint_block: summary.claimant_checkpoint_block,
        },
        elapsed_seconds: elapsed.as_secs() as i64,
        succeeded: true,
    });
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

#[derive(Debug)]
struct ClaimantSnapshot {
    claimable_reserve: BigDecimal,
    claimed_reserve: BigDecimal,
    raw_call: serde_json::Value,
}

async fn fetch_balance_candidates_after(
    pg: &PgPool,
    include_tentative: bool,
    resume_from_block: Option<i64>,
    limit: i64,
) -> Result<Vec<GatewayCandidate>> {
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
              AND ($2::bigint IS NULL OR l.block_number >= $2)
            ORDER BY l.block_number ASC, l.gateway_address ASC
            LIMIT $3"#,
    );
    let rows = sqlx::query(&sql)
        .bind(ARBITRUM_CHAIN_ID)
        .bind(resume_from_block)
        .bind(limit)
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

async fn fetch_flow_candidates_after(
    pg: &PgPool,
    include_tentative: bool,
    resume_from_block: Option<i64>,
    limit: i64,
) -> Result<Vec<GatewayFlowCandidate>> {
    let finality_filter = if include_tentative {
        ""
    } else {
        "AND r.finality = 'finalized'"
    };
    let sql = format!(
        r#"SELECT r.id AS event_id,
                  r.from_address AS gateway_address,
                  CASE
                      WHEN r.event_name IN ('WinningTicketRedeemed', 'WinningTicketTransfer', 'ReserveClaimed')
                      THEN r.to_address
                      ELSE NULL
                  END AS claimant_address,
                  r.to_address AS counterparty_address,
                  r.event_name,
                  CASE
                      WHEN r.event_name = 'DepositFunded' THEN 'deposit_in'
                      WHEN r.event_name = 'ReserveFunded' THEN 'reserve_in'
                      WHEN r.event_name = 'WinningTicketTransfer' THEN 'reserve_transfer'
                      WHEN r.event_name = 'WinningTicketRedeemed' THEN 'ticket_redeemed'
                      WHEN r.event_name = 'ReserveClaimed' THEN 'reserve_claimed'
                      WHEN r.event_name = 'Withdrawal' THEN 'withdrawal'
                      ELSE 'other'
                  END AS flow_kind,
                  r.block_number,
                  r.block_timestamp,
                  r.tx_hash,
                  r.log_index,
                  r.asset,
                  r.amount_normalized,
                  v.amount_usd,
                  v.valuation_version,
                  r.block_hash
             FROM raw_protocol_events r
             LEFT JOIN event_valuations v
               ON v.event_id = r.id
              AND v.status = 'priced'
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
                    'Withdrawal'
              )
              {finality_filter}
              AND ($2::bigint IS NULL OR r.block_number >= $2)
              AND NOT EXISTS (
                    SELECT 1
                      FROM gateway_flows gf
                     WHERE gf.event_id = r.id
                       AND gf.flow_kind = CASE
                           WHEN r.event_name = 'DepositFunded' THEN 'deposit_in'
                           WHEN r.event_name = 'ReserveFunded' THEN 'reserve_in'
                           WHEN r.event_name = 'WinningTicketTransfer' THEN 'reserve_transfer'
                           WHEN r.event_name = 'WinningTicketRedeemed' THEN 'ticket_redeemed'
                           WHEN r.event_name = 'ReserveClaimed' THEN 'reserve_claimed'
                           WHEN r.event_name = 'Withdrawal' THEN 'withdrawal'
                           ELSE 'other'
                       END
              )
            ORDER BY r.block_number ASC, r.log_index ASC
            LIMIT $3"#,
    );
    let rows = sqlx::query(&sql)
        .bind(ARBITRUM_CHAIN_ID)
        .bind(resume_from_block)
        .bind(limit)
        .fetch_all(pg)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| GatewayFlowCandidate {
            event_id: r.get("event_id"),
            gateway_address: r.get("gateway_address"),
            claimant_address: r.try_get("claimant_address").ok(),
            counterparty_address: r.try_get("counterparty_address").ok(),
            event_name: r.get("event_name"),
            flow_kind: r.get("flow_kind"),
            block_number: r.get("block_number"),
            block_timestamp: r.get("block_timestamp"),
            tx_hash: r.get("tx_hash"),
            log_index: r.get("log_index"),
            asset: r.try_get("asset").ok(),
            amount_native: r.try_get("amount_normalized").ok(),
            amount_usd: r.try_get("amount_usd").ok(),
            valuation_version: r.try_get("valuation_version").ok(),
            block_hash: r.get("block_hash"),
        })
        .collect())
}

async fn fetch_claimant_candidates_after(
    pg: &PgPool,
    include_tentative: bool,
    resume_from_block: Option<i64>,
    limit: i64,
) -> Result<Vec<GatewayFlowCandidate>> {
    let finality_filter = if include_tentative {
        ""
    } else {
        "AND r.finality = 'finalized'"
    };
    let sql = format!(
        r#"SELECT r.id AS event_id,
                  r.from_address AS gateway_address,
                  r.to_address AS claimant_address,
                  r.to_address AS counterparty_address,
                  r.event_name,
                  CASE
                      WHEN r.event_name = 'WinningTicketTransfer' THEN 'reserve_transfer'
                      WHEN r.event_name = 'WinningTicketRedeemed' THEN 'ticket_redeemed'
                      WHEN r.event_name = 'ReserveClaimed' THEN 'reserve_claimed'
                      ELSE 'other'
                  END AS flow_kind,
                  r.block_number,
                  r.block_timestamp,
                  r.tx_hash,
                  r.log_index,
                  r.asset,
                  r.amount_normalized,
                  v.amount_usd,
                  v.valuation_version,
                  r.block_hash
             FROM raw_protocol_events r
             LEFT JOIN event_valuations v
               ON v.event_id = r.id
              AND v.status = 'priced'
             LEFT JOIN gateway_claimants_by_block gc
               ON gc.chain_id = $1
              AND gc.gateway_address = r.from_address
              AND gc.claimant_address = r.to_address
              AND gc.block_number = r.block_number
            WHERE r.chain_id = $1
              AND r.is_canonical = TRUE
              AND r.contract_name = 'TicketBroker'
              AND r.from_address IS NOT NULL
              AND r.to_address IS NOT NULL
              AND r.event_name IN (
                    'WinningTicketTransfer',
                    'WinningTicketRedeemed',
                    'ReserveClaimed'
              )
              {finality_filter}
              AND ($2::bigint IS NULL OR r.block_number >= $2)
              AND gc.gateway_address IS NULL
            ORDER BY r.block_number ASC, r.log_index ASC
            LIMIT $3"#,
    );
    let rows = sqlx::query(&sql)
        .bind(ARBITRUM_CHAIN_ID)
        .bind(resume_from_block)
        .bind(limit)
        .fetch_all(pg)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| GatewayFlowCandidate {
            event_id: r.get("event_id"),
            gateway_address: r.get("gateway_address"),
            claimant_address: r.try_get("claimant_address").ok(),
            counterparty_address: r.try_get("counterparty_address").ok(),
            event_name: r.get("event_name"),
            flow_kind: r.get("flow_kind"),
            block_number: r.get("block_number"),
            block_timestamp: r.get("block_timestamp"),
            tx_hash: r.get("tx_hash"),
            log_index: r.get("log_index"),
            asset: r.try_get("asset").ok(),
            amount_native: r.try_get("amount_normalized").ok(),
            amount_usd: r.try_get("amount_usd").ok(),
            valuation_version: r.try_get("valuation_version").ok(),
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

async fn read_claimant_state(
    pg: &PgPool,
    archive: &Provider,
    ticket_broker: &str,
    gateway: &str,
    claimant: &str,
    block_number: i64,
) -> Result<ClaimantSnapshot> {
    let gateway_addr = Address::from_str(gateway).context("parsing gateway address")?;
    let claimant_addr = Address::from_str(claimant).context("parsing claimant address")?;

    let claimable_data = format!(
        "0x{}",
        alloy::hex::encode(
            TicketBroker::claimableReserveCall {
                _reserveHolder: gateway_addr,
                _claimant: claimant_addr,
            }
            .abi_encode()
        )
    );
    let claimable_params = json!([
        { "to": ticket_broker, "data": claimable_data },
        BlockTag::Number(block_number as u64).to_param()
    ]);
    let claimable_outcome = cross_check::single_call_cached(
        pg,
        archive,
        "eth_call",
        &claimable_params,
        Some(block_number),
    )
    .await?;
    let claimable_raw = decode_hex_result(&claimable_outcome.response_bytes)?;
    let claimable = TicketBroker::claimableReserveCall::abi_decode_returns(&claimable_raw, true)?;

    let claimed_data = format!(
        "0x{}",
        alloy::hex::encode(
            TicketBroker::claimedReserveCall {
                _reserveHolder: gateway_addr,
                _claimant: claimant_addr,
            }
            .abi_encode()
        )
    );
    let claimed_params = json!([
        { "to": ticket_broker, "data": claimed_data },
        BlockTag::Number(block_number as u64).to_param()
    ]);
    let claimed_outcome = cross_check::single_call_cached(
        pg,
        archive,
        "eth_call",
        &claimed_params,
        Some(block_number),
    )
    .await?;
    let claimed_raw = decode_hex_result(&claimed_outcome.response_bytes)?;
    let claimed = TicketBroker::claimedReserveCall::abi_decode_returns(&claimed_raw, true)?;

    Ok(ClaimantSnapshot {
        claimable_reserve: u256_to_decimal(&claimable._0, ETH_DECIMALS),
        claimed_reserve: u256_to_decimal(&claimed._0, ETH_DECIMALS),
        raw_call: json!({
            "claimableReserve_call_hash": claimable_outcome.call_hash,
            "claimedReserve_call_hash": claimed_outcome.call_hash,
        }),
    })
}

async fn upsert_gateway_claimant_row(
    pg: &PgPool,
    candidate: &GatewayFlowCandidate,
    snapshot: &ClaimantSnapshot,
) -> Result<()> {
    let claimant = match candidate.claimant_address.as_ref() {
        Some(v) => v,
        None => return Ok(()),
    };
    sqlx::query(
        r#"INSERT INTO gateway_claimants_by_block (
               chain_id, gateway_address, claimant_address, block_number, block_timestamp, block_hash,
               claimable_reserve, claimed_reserve, source, raw_call, triggering_event_id
           ) VALUES (
               $1, $2, $3, $4, $5, $6,
               $7, $8, $9, $10, $11
           )
           ON CONFLICT (chain_id, gateway_address, claimant_address, block_number) DO UPDATE
               SET block_timestamp = EXCLUDED.block_timestamp,
                   block_hash = EXCLUDED.block_hash,
                   claimable_reserve = EXCLUDED.claimable_reserve,
                   claimed_reserve = EXCLUDED.claimed_reserve,
                   source = EXCLUDED.source,
                   raw_call = EXCLUDED.raw_call,
                   triggering_event_id = EXCLUDED.triggering_event_id"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(&candidate.gateway_address)
    .bind(claimant)
    .bind(candidate.block_number)
    .bind(candidate.block_timestamp)
    .bind(&candidate.block_hash)
    .bind(&snapshot.claimable_reserve)
    .bind(&snapshot.claimed_reserve)
    .bind(SOURCE_RPC_RECONCILED)
    .bind(&snapshot.raw_call)
    .bind(candidate.event_id)
    .execute(pg)
    .await?;
    Ok(())
}

async fn upsert_gateway_flow(pg: &PgPool, candidate: &GatewayFlowCandidate) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO gateway_flows (
               chain_id, event_id, gateway_address, claimant_address, counterparty_address,
               block_number, block_timestamp, tx_hash, log_index, event_name, flow_kind,
               asset, amount_native, amount_usd, valuation_version
           ) VALUES (
               $1, $2, $3, $4, $5,
               $6, $7, $8, $9, $10, $11,
               $12, $13, $14, $15
           )
           ON CONFLICT (event_id, flow_kind) DO UPDATE
               SET gateway_address = EXCLUDED.gateway_address,
                   claimant_address = EXCLUDED.claimant_address,
                   counterparty_address = EXCLUDED.counterparty_address,
                   block_number = EXCLUDED.block_number,
                   block_timestamp = EXCLUDED.block_timestamp,
                   tx_hash = EXCLUDED.tx_hash,
                   log_index = EXCLUDED.log_index,
                   event_name = EXCLUDED.event_name,
                   asset = EXCLUDED.asset,
                   amount_native = EXCLUDED.amount_native,
                   amount_usd = EXCLUDED.amount_usd,
                   valuation_version = EXCLUDED.valuation_version"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(candidate.event_id)
    .bind(&candidate.gateway_address)
    .bind(&candidate.claimant_address)
    .bind(&candidate.counterparty_address)
    .bind(candidate.block_number)
    .bind(candidate.block_timestamp)
    .bind(&candidate.tx_hash)
    .bind(candidate.log_index)
    .bind(&candidate.event_name)
    .bind(&candidate.flow_kind)
    .bind(&candidate.asset)
    .bind(&candidate.amount_native)
    .bind(&candidate.amount_usd)
    .bind(&candidate.valuation_version)
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

async fn load_gateway_checkpoint(pg: &PgPool, name: &str) -> Result<Option<i64>> {
    let block = sqlx::query_scalar::<_, i64>(
        "SELECT last_processed_block FROM indexer_checkpoints WHERE name = $1",
    )
    .bind(name)
    .fetch_optional(pg)
    .await?;
    Ok(block)
}

async fn advance_gateway_checkpoint(pg: &PgPool, name: &str, block: i64) -> Result<()> {
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
