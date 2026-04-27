//! Pending-stake / pending-fees refresh via BondingManager RPC calls. SPEC §11.10
//! Scope 2 enhancement.
//!
//! For every EarningsClaimed event:
//!   - Read delegator from from_address
//!   - Read endRound from raw_event.decoded.endRound
//!   - eth_call BondingManager.pendingStake(delegator, endRound) at event block
//!   - eth_call BondingManager.pendingFees(delegator, endRound) at event block
//!   - UPDATE stake_balances_by_block.pending_{stake,fees,round}; bump source to 'both'
//!
//! Reads go through `cross_check::single_call_cached` so the deterministic-replay
//! invariant holds — second run reads from `rpc_call_cache`.

use alloy::primitives::{Address, U256};
use alloy::sol;
use alloy::sol_types::SolCall;
use anyhow::{Context, Result};
use bigdecimal::BigDecimal;
use livepeer_core::rpc::{cross_check, Provider};
use serde::Serialize;
use sqlx::{PgPool, Row};
use std::str::FromStr;
use tracing::{debug, info, warn};

const ARBITRUM_CHAIN_ID: i64 = 42161;
const LPT_DECIMALS: u32 = 18;
const ETH_DECIMALS: u32 = 18;

sol! {
    #[allow(missing_docs)]
    interface BondingManager {
        function pendingStake(address _delegator, uint256 _endRound) external view returns (uint256);
        function pendingFees(address _delegator, uint256 _endRound) external view returns (uint256);
    }
}

#[derive(Debug, Default, Serialize)]
pub struct PendingSummary {
    pub events_considered: u64,
    pub refreshed: u64,
    pub failed_decode: u64,
    pub no_stake_row: u64,
}

pub async fn refresh_pending(
    pg: &PgPool,
    archive: &Provider,
    bonding_manager: &str,
    include_tentative: bool,
) -> Result<PendingSummary> {
    let finality = if include_tentative { "" } else { "AND finality = 'finalized'" };
    let sql = format!(
        r#"SELECT id, block_number, block_timestamp, from_address,
                  raw_event -> 'decoded' ->> 'endRound' AS end_round
             FROM raw_protocol_events
            WHERE chain_id = $1
              AND event_name = 'EarningsClaimed'
              AND is_canonical = TRUE
              {finality}
            ORDER BY block_number, log_index"#
    );
    let rows = sqlx::query(&sql)
        .bind(ARBITRUM_CHAIN_ID)
        .fetch_all(pg)
        .await?;
    info!(events = rows.len(), "pending refresh starting");

    let mut summary = PendingSummary {
        events_considered: rows.len() as u64,
        ..Default::default()
    };

    for r in &rows {
        let event_id: i64 = r.get(0);
        let block_number: i64 = r.get(1);
        let from: Option<String> = r.try_get(3).ok();
        let end_round_str: Option<String> = r.try_get(4).ok();
        let (Some(delegator), Some(end_round_str)) = (from, end_round_str) else {
            summary.failed_decode += 1;
            continue;
        };
        let delegator_lower = delegator.to_lowercase();
        let delegator_addr = match Address::from_str(&delegator_lower) {
            Ok(a) => a,
            Err(_) => { summary.failed_decode += 1; continue; }
        };
        let end_round = match U256::from_str(&end_round_str) {
            Ok(n) => n,
            Err(_) => { summary.failed_decode += 1; continue; }
        };

        let pending_stake = match read_pending(pg, archive, bonding_manager, "pendingStake", delegator_addr, end_round, block_number).await {
            Ok(v) => v,
            Err(e) => { warn!(event_id, error = %e, "pendingStake read failed"); summary.failed_decode += 1; continue; }
        };
        let pending_fees = match read_pending(pg, archive, bonding_manager, "pendingFees", delegator_addr, end_round, block_number).await {
            Ok(v) => v,
            Err(e) => { warn!(event_id, error = %e, "pendingFees read failed"); summary.failed_decode += 1; continue; }
        };

        let pstake_lpt = u256_to_decimal(&pending_stake, LPT_DECIMALS);
        let pfees_eth = u256_to_decimal(&pending_fees, ETH_DECIMALS);
        let end_round_i64: i64 = end_round_str.parse().unwrap_or(0);

        // Update the existing stake row at this delegator+block. If it doesn't exist
        // (delegator wasn't previously seen — partial-window backfill), record nothing.
        let updated = sqlx::query(
            r#"UPDATE stake_balances_by_block
                  SET pending_stake = $4,
                      pending_fees  = $5,
                      pending_round = $6,
                      source        = 'both',
                      raw_call      = $7
                WHERE chain_id = $1 AND delegator_address = $2 AND block_number = $3"#,
        )
        .bind(ARBITRUM_CHAIN_ID)
        .bind(&delegator_lower)
        .bind(block_number)
        .bind(&pstake_lpt)
        .bind(&pfees_eth)
        .bind(end_round_i64)
        .bind(serde_json::json!({
            "pendingStake_wei": pending_stake.to_string(),
            "pendingFees_wei":  pending_fees.to_string(),
            "endRound":         end_round.to_string(),
        }))
        .execute(pg)
        .await?;
        if updated.rows_affected() == 0 {
            summary.no_stake_row += 1;
            continue;
        }
        summary.refreshed += 1;
        debug!(event_id, %delegator_lower, %pstake_lpt, %pfees_eth, end_round = end_round_i64, "pending refreshed");
    }

    info!(?summary, "pending refresh complete");
    Ok(summary)
}

async fn read_pending(
    pg: &PgPool,
    archive: &Provider,
    bonding_manager: &str,
    method: &'static str,
    delegator: Address,
    end_round: U256,
    block_number: i64,
) -> Result<U256> {
    let calldata = if method == "pendingStake" {
        BondingManager::pendingStakeCall { _delegator: delegator, _endRound: end_round }.abi_encode()
    } else {
        BondingManager::pendingFeesCall { _delegator: delegator, _endRound: end_round }.abi_encode()
    };
    let data = format!("0x{}", alloy::hex::encode(calldata));
    let outcome = cross_check::single_call_cached(
        pg,
        archive,
        "eth_call",
        &serde_json::json!([{ "to": bonding_manager, "data": data }, format!("0x{:x}", block_number as u64)]),
        Some(block_number),
    )
    .await?;
    let s = std::str::from_utf8(&outcome.response_bytes).unwrap_or_default();
    let hex_str = s.trim_matches('"').trim_start_matches("0x");
    let raw = alloy::hex::decode(hex_str).context("decoding eth_call return hex")?;
    if method == "pendingStake" {
        let v = BondingManager::pendingStakeCall::abi_decode_returns(&raw, true)?;
        Ok(v._0)
    } else {
        let v = BondingManager::pendingFeesCall::abi_decode_returns(&raw, true)?;
        Ok(v._0)
    }
}

fn u256_to_decimal(u: &U256, decimals: u32) -> BigDecimal {
    let raw = BigDecimal::from_str(&u.to_string()).unwrap_or_default();
    raw / BigDecimal::from(10u128.pow(decimals))
}
