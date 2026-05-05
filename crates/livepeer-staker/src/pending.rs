//! Exact stake-state + pending-stake / pending-fees refresh via BondingManager RPC
//! calls. SPEC §11.10
//! Scope 2 enhancement.
//!
//! Stage 1:
//!   - Read `getDelegator(delegator)` at every existing stake-row block.
//!   - Overwrite flow-derived `bonded_principal` / `delegate_address` with
//!     contract truth.
//!
//! Stage 2, for every EarningsClaimed event:
//!   - Read delegator from from_address
//!   - Read endRound from raw_event.decoded.endRound
//!   - eth_call BondingManager.pendingStake(delegator, endRound) at event block
//!   - eth_call BondingManager.pendingFees(delegator, endRound) at event block
//!   - UPDATE stake_balances_by_block.pending_{stake,fees,round}; bump source to 'both'
//!
//! Reads go through `rpc_call_cache` so the deterministic-replay invariant holds.
//!
//! ## Bulk implementation (TD-009)
//!
//! Per-event N+1 (4-5 round-trips × 56K events = ~3-4h walltime) replaced with:
//!   1. Compute deterministic `call_hash` for each event's pendingStake/pendingFees call.
//!   2. Bulk-SELECT all matching `rpc_call_cache` rows in ONE query → in-memory HashMap.
//!   3. Loop events in-memory, decoding cached responses; only fall through to
//!      `cross_check::single_call_cached` on cache miss (rare on a warm DB).
//!   4. Bulk-UPDATE `stake_balances_by_block` via `UPDATE … FROM unnest(arrays)`
//!      in chunks of 5000 events.
//!
//! Determinism preserved: cache lookups produce byte-identical responses to the
//! per-event path; the UPDATE applies the same values to the same rows. Only
//! difference vs the per-event version: ordering of independent UPDATEs (which
//! can't observe each other anyway since each touches a distinct PK).

use alloy::primitives::{Address, U256};
use alloy::sol;
use alloy::sol_types::SolCall;
use anyhow::{Context, Result};
use bigdecimal::BigDecimal;
use livepeer_core::rpc::{cache, cross_check, Provider};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::str::FromStr;
use tracing::{debug, info, warn};

const ARBITRUM_CHAIN_ID: i64 = 42161;
const LPT_DECIMALS: u32 = 18;
const ETH_DECIMALS: u32 = 18;
const UPDATE_CHUNK: usize = 5_000;
const EXACT_BATCH_SIZE: usize = 250;
const EXACT_LAYER_STALE_RETRIES: usize = 3;

sol! {
    #[allow(missing_docs)]
    interface BondingManager {
        function getDelegator(address _delegator) external view returns (
            uint256 bondedAmount,
            uint256 fees,
            address delegateAddress,
            uint256 delegatedAmount,
            uint256 startRound,
            uint256 lastClaimRound,
            uint256 nextUnbondingLockId
        );
        function pendingStake(address _delegator, uint256 _endRound) external view returns (uint256);
        function pendingFees(address _delegator, uint256 _endRound) external view returns (uint256);
    }
}

#[derive(Debug, Default, Serialize)]
pub struct PendingSummary {
    pub reconciled_rows: u64,
    pub events_considered: u64,
    pub refreshed: u64,
    pub failed_decode: u64,
    pub no_stake_row: u64,
}

#[derive(Debug)]
struct PreparedUpdate {
    delegator: String,
    block_number: i64,
    pending_stake: BigDecimal,
    pending_fees: BigDecimal,
    pending_round: i64,
    raw_call: Value,
}

#[derive(Clone, Debug)]
struct ExactStakeRpcCall {
    delegator_lower: String,
    block_number: i64,
    state_hash: String,
    state_params: Value,
}

pub async fn refresh_pending(
    pg: &PgPool,
    archive: &Provider,
    bonding_manager: &str,
    include_tentative: bool,
) -> Result<PendingSummary> {
    let reconciled_rows = reconcile_exact_stake_rows(pg, archive, bonding_manager).await?;

    let finality = if include_tentative {
        ""
    } else {
        "AND finality = 'finalized'"
    };
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
    info!(events = rows.len(), "pending refresh starting (bulk)");

    let mut summary = PendingSummary {
        reconciled_rows,
        events_considered: rows.len() as u64,
        ..Default::default()
    };

    // Stage 1: parse + compute call_hashes for every (event, method) pair.
    struct Event {
        event_id: i64,
        block_number: i64,
        delegator_lower: String,
        delegator_addr: Address,
        end_round: U256,
        end_round_str: String,
        stake_hash: String,
        stake_params: Value,
        fees_hash: String,
        fees_params: Value,
    }
    let mut events: Vec<Event> = Vec::with_capacity(rows.len());
    let mut all_hashes: Vec<String> = Vec::with_capacity(rows.len() * 2);
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
            Err(_) => {
                summary.failed_decode += 1;
                continue;
            }
        };
        let end_round = match U256::from_str(&end_round_str) {
            Ok(n) => n,
            Err(_) => {
                summary.failed_decode += 1;
                continue;
            }
        };
        let (stake_params, stake_hash) = call_for(
            bonding_manager,
            "pendingStake",
            delegator_addr,
            end_round,
            block_number,
        );
        let (fees_params, fees_hash) = call_for(
            bonding_manager,
            "pendingFees",
            delegator_addr,
            end_round,
            block_number,
        );
        all_hashes.push(stake_hash.clone());
        all_hashes.push(fees_hash.clone());
        events.push(Event {
            event_id,
            block_number,
            delegator_lower,
            delegator_addr,
            end_round,
            end_round_str,
            stake_hash,
            stake_params,
            fees_hash,
            fees_params,
        });
    }

    // Stage 2: bulk-fetch all cached responses in one round-trip.
    let cache_map = prefetch_cache(pg, &all_hashes).await?;
    info!(
        unique_hashes = all_hashes.len(),
        cache_hits = cache_map.len(),
        "bulk-prefetched rpc_call_cache"
    );

    // Stage 3: decode in-memory, fall through to single_call_cached on miss.
    let mut updates: Vec<PreparedUpdate> = Vec::with_capacity(events.len());
    for ev in &events {
        let pending_stake = match resolve_call(
            &cache_map,
            pg,
            archive,
            "pendingStake",
            &ev.stake_hash,
            &ev.stake_params,
            ev.block_number,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                warn!(event_id = ev.event_id, error = %e, "pendingStake read failed");
                summary.failed_decode += 1;
                continue;
            }
        };
        let pending_fees = match resolve_call(
            &cache_map,
            pg,
            archive,
            "pendingFees",
            &ev.fees_hash,
            &ev.fees_params,
            ev.block_number,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                warn!(event_id = ev.event_id, error = %e, "pendingFees read failed");
                summary.failed_decode += 1;
                continue;
            }
        };
        let pstake_lpt = u256_to_decimal(&pending_stake, LPT_DECIMALS);
        let pfees_eth = u256_to_decimal(&pending_fees, ETH_DECIMALS);
        let end_round_i64: i64 = ev.end_round_str.parse().unwrap_or(0);
        updates.push(PreparedUpdate {
            delegator: ev.delegator_lower.clone(),
            block_number: ev.block_number,
            pending_stake: pstake_lpt.clone(),
            pending_fees: pfees_eth.clone(),
            pending_round: end_round_i64,
            raw_call: serde_json::json!({
                "pendingStake_wei": pending_stake.to_string(),
                "pendingFees_wei":  pending_fees.to_string(),
                "endRound":         ev.end_round.to_string(),
            }),
        });
        debug!(event_id = ev.event_id, %pstake_lpt, %pfees_eth, "pending decoded");
        let _ = ev.delegator_addr; // silence unused (kept on Event for future)
    }

    // Stage 4: bulk UPDATE in chunks. UPDATE … FROM unnest(arrays).
    let mut refreshed_total: u64 = 0;
    let mut no_stake_total: u64 = 0;
    for chunk in updates.chunks(UPDATE_CHUNK) {
        let mut delegators: Vec<&str> = Vec::with_capacity(chunk.len());
        let mut blocks: Vec<i64> = Vec::with_capacity(chunk.len());
        let mut pstakes: Vec<BigDecimal> = Vec::with_capacity(chunk.len());
        let mut pfeess: Vec<BigDecimal> = Vec::with_capacity(chunk.len());
        let mut prounds: Vec<i64> = Vec::with_capacity(chunk.len());
        let mut raws: Vec<Value> = Vec::with_capacity(chunk.len());
        for u in chunk {
            delegators.push(&u.delegator);
            blocks.push(u.block_number);
            pstakes.push(u.pending_stake.clone());
            pfeess.push(u.pending_fees.clone());
            prounds.push(u.pending_round);
            raws.push(u.raw_call.clone());
        }
        let result = sqlx::query(
            r#"UPDATE stake_balances_by_block s
                  SET pending_stake = v.pstake,
                      pending_fees  = v.pfees,
                      pending_round = v.pround,
                      source        = 'both',
                      raw_call      = v.raw_call
                 FROM unnest($2::text[], $3::bigint[],
                             $4::numeric[], $5::numeric[],
                             $6::bigint[], $7::jsonb[])
                      AS v(delegator, block_number, pstake, pfees, pround, raw_call)
                WHERE s.chain_id          = $1
                  AND s.delegator_address = v.delegator
                  AND s.block_number      = v.block_number"#,
        )
        .bind(ARBITRUM_CHAIN_ID)
        .bind(&delegators)
        .bind(&blocks)
        .bind(&pstakes)
        .bind(&pfeess)
        .bind(&prounds)
        .bind(&raws)
        .execute(pg)
        .await?;
        let chunk_refreshed = result.rows_affected();
        refreshed_total += chunk_refreshed;
        no_stake_total += chunk.len() as u64 - chunk_refreshed;
    }
    summary.refreshed = refreshed_total;
    summary.no_stake_row = no_stake_total;

    info!(?summary, "pending refresh complete (bulk)");
    Ok(summary)
}

#[derive(Debug)]
struct ExactStakeUpdate {
    delegator: String,
    block_number: i64,
    bonded_principal: BigDecimal,
    delegate_address: String,
}

async fn reconcile_exact_stake_rows(
    pg: &PgPool,
    archive: &Provider,
    bonding_manager: &str,
) -> Result<u64> {
    let rows = sqlx::query(
        r#"SELECT delegator_address, block_number
             FROM stake_balances_by_block
            WHERE chain_id = $1
            ORDER BY block_number, delegator_address"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .fetch_all(pg)
    .await?;
    info!(
        rows = rows.len(),
        "exact stake reconciliation starting (bulk)"
    );

    let mut calls: Vec<ExactStakeRpcCall> = Vec::with_capacity(rows.len());
    let mut all_hashes: Vec<String> = Vec::with_capacity(rows.len());
    for r in &rows {
        let delegator_lower: String = r.get(0);
        let block_number: i64 = r.get(1);
        let delegator_addr = match Address::from_str(&delegator_lower) {
            Ok(a) => a,
            Err(_) => continue,
        };
        let (state_params, state_hash) =
            call_for_get_delegator(bonding_manager, delegator_addr, block_number);
        all_hashes.push(state_hash.clone());
        calls.push(ExactStakeRpcCall {
            delegator_lower,
            block_number,
            state_hash,
            state_params,
        });
    }

    let cache_map = prefetch_cache(pg, &all_hashes).await?;
    info!(
        unique_hashes = all_hashes.len(),
        cache_hits = cache_map.len(),
        "bulk-prefetched rpc_call_cache for exact stake reconciliation"
    );

    let mut updates: Vec<ExactStakeUpdate> = Vec::with_capacity(calls.len());
    let mut misses: Vec<ExactStakeRpcCall> = Vec::new();
    for call in &calls {
        if let Some(bytes) = cache_map.get(&call.state_hash) {
            let state = decode_get_delegator(bytes)?;
            updates.push(ExactStakeUpdate {
                delegator: call.delegator_lower.clone(),
                block_number: call.block_number,
                bonded_principal: u256_to_decimal(&state.bonded_amount, LPT_DECIMALS),
                delegate_address: format!("{:#x}", state.delegate_address).to_lowercase(),
            });
        } else {
            misses.push(ExactStakeRpcCall {
                delegator_lower: call.delegator_lower.clone(),
                block_number: call.block_number,
                state_hash: call.state_hash.clone(),
                state_params: call.state_params.clone(),
            });
        }
    }

    if !misses.is_empty() {
        info!(
            misses = misses.len(),
            batch_size = EXACT_BATCH_SIZE,
            "resolving exact stake cache misses"
        );
        let missed_updates =
            resolve_exact_misses_batched(pg, archive, &misses, EXACT_LAYER_STALE_RETRIES).await?;
        updates.extend(missed_updates);
    }

    let mut reconciled_total: u64 = 0;
    for chunk in updates.chunks(UPDATE_CHUNK) {
        let mut delegators: Vec<&str> = Vec::with_capacity(chunk.len());
        let mut blocks: Vec<i64> = Vec::with_capacity(chunk.len());
        let mut principals: Vec<BigDecimal> = Vec::with_capacity(chunk.len());
        let mut delegates: Vec<&str> = Vec::with_capacity(chunk.len());
        for u in chunk {
            delegators.push(&u.delegator);
            blocks.push(u.block_number);
            principals.push(u.bonded_principal.clone());
            delegates.push(&u.delegate_address);
        }
        let result = sqlx::query(
            r#"UPDATE stake_balances_by_block s
                  SET bonded_principal = v.bonded_principal,
                      delegate_address = v.delegate_address
                 FROM unnest($2::text[], $3::bigint[],
                             $4::numeric[], $5::text[])
                      AS v(delegator, block_number, bonded_principal, delegate_address)
                WHERE s.chain_id          = $1
                  AND s.delegator_address = v.delegator
                  AND s.block_number      = v.block_number"#,
        )
        .bind(ARBITRUM_CHAIN_ID)
        .bind(&delegators)
        .bind(&blocks)
        .bind(&principals)
        .bind(&delegates)
        .execute(pg)
        .await?;
        reconciled_total += result.rows_affected();
    }

    info!(
        reconciled_rows = reconciled_total,
        "exact stake reconciliation complete (bulk)"
    );
    Ok(reconciled_total)
}

/// Compute the (params Value, call_hash) for a pending* call. The hash matches
/// what `cross_check::single_call_cached` writes via `cache::compute_call_hash`,
/// so a cache hit here means the per-event path would also have hit.
fn call_for(
    bonding_manager: &str,
    method: &'static str,
    delegator: Address,
    end_round: U256,
    block_number: i64,
) -> (Value, String) {
    let calldata = if method == "pendingStake" {
        BondingManager::pendingStakeCall {
            _delegator: delegator,
            _endRound: end_round,
        }
        .abi_encode()
    } else {
        BondingManager::pendingFeesCall {
            _delegator: delegator,
            _endRound: end_round,
        }
        .abi_encode()
    };
    let data = format!("0x{}", alloy::hex::encode(calldata));
    let params = serde_json::json!([{ "to": bonding_manager, "data": data }, format!("0x{:x}", block_number as u64)]);
    let hash = cache::compute_call_hash("eth_call", &params, Some(block_number));
    (params, hash)
}

fn call_for_get_delegator(
    bonding_manager: &str,
    delegator: Address,
    block_number: i64,
) -> (Value, String) {
    let calldata = BondingManager::getDelegatorCall {
        _delegator: delegator,
    }
    .abi_encode();
    let data = format!("0x{}", alloy::hex::encode(calldata));
    let params = serde_json::json!([{ "to": bonding_manager, "data": data }, format!("0x{:x}", block_number as u64)]);
    let hash = cache::compute_call_hash("eth_call", &params, Some(block_number));
    (params, hash)
}

async fn prefetch_cache(pg: &PgPool, hashes: &[String]) -> Result<HashMap<String, Vec<u8>>> {
    if hashes.is_empty() {
        return Ok(HashMap::new());
    }
    // Dedup before SELECTing.
    let mut unique: Vec<&String> = hashes.iter().collect();
    unique.sort();
    unique.dedup();
    let unique_owned: Vec<String> = unique.into_iter().cloned().collect();
    let rows = sqlx::query(
        "SELECT call_hash, response_bytes FROM rpc_call_cache WHERE call_hash = ANY($1)",
    )
    .bind(&unique_owned)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get::<String, _>(0), r.get::<Vec<u8>, _>(1)))
        .collect())
}

async fn resolve_call(
    cache_map: &HashMap<String, Vec<u8>>,
    pg: &PgPool,
    archive: &Provider,
    method_kind: &'static str,
    call_hash: &str,
    params: &Value,
    block_number: i64,
) -> Result<U256> {
    // Cache hit: decode in-memory.
    if let Some(bytes) = cache_map.get(call_hash) {
        return decode_pending(method_kind, bytes);
    }
    // Cache miss: defer to the existing cross-check flow (RPC + cache write).
    let outcome =
        cross_check::single_call_cached(pg, archive, "eth_call", params, Some(block_number))
            .await?;
    decode_pending(method_kind, &outcome.response_bytes)
}

fn decode_pending(method_kind: &str, bytes: &[u8]) -> Result<U256> {
    let s = std::str::from_utf8(bytes).unwrap_or_default();
    let hex_str = s.trim_matches('"').trim_start_matches("0x");
    let raw = alloy::hex::decode(hex_str).context("decoding eth_call return hex")?;
    if method_kind == "pendingStake" {
        let v = BondingManager::pendingStakeCall::abi_decode_returns(&raw, true)?;
        Ok(v._0)
    } else {
        let v = BondingManager::pendingFeesCall::abi_decode_returns(&raw, true)?;
        Ok(v._0)
    }
}

struct DelegatorState {
    bonded_amount: U256,
    delegate_address: Address,
}

async fn resolve_exact_misses_batched(
    pg: &PgPool,
    archive: &Provider,
    misses: &[ExactStakeRpcCall],
    retries_left: usize,
) -> Result<Vec<ExactStakeUpdate>> {
    let mut updates = Vec::with_capacity(misses.len());
    let mut pending_misses: Vec<ExactStakeRpcCall> = misses.to_vec();
    let mut retries_remaining = retries_left;

    loop {
        let mut retry_misses: Vec<ExactStakeRpcCall> = Vec::new();

        for chunk in pending_misses.chunks(EXACT_BATCH_SIZE) {
            let requests: Vec<(String, Value, Option<i64>)> = chunk
                .iter()
                .map(|call| {
                    (
                        "eth_call".to_string(),
                        call.state_params.clone(),
                        Some(call.block_number),
                    )
                })
                .collect();
            let results = cross_check::batch_call_cached(pg, archive, &requests).await?;
            for (call, result) in chunk.iter().zip(results.into_iter()) {
                match result {
                    Ok(outcome) => {
                        let state = decode_get_delegator(&outcome.response_bytes)?;
                        updates.push(ExactStakeUpdate {
                            delegator: call.delegator_lower.clone(),
                            block_number: call.block_number,
                            bonded_principal: u256_to_decimal(&state.bonded_amount, LPT_DECIMALS),
                            delegate_address: format!("{:#x}", state.delegate_address)
                                .to_lowercase(),
                        });
                    }
                    Err(e) => {
                        if retries_remaining > 0 && is_layer_stale_error(&e.to_string()) {
                            retry_misses.push(ExactStakeRpcCall {
                                delegator_lower: call.delegator_lower.clone(),
                                block_number: call.block_number,
                                state_hash: call.state_hash.clone(),
                                state_params: call.state_params.clone(),
                            });
                        } else {
                            return Err(e.into());
                        }
                    }
                }
            }
        }

        if retry_misses.is_empty() {
            break;
        }

        warn!(
            retry_misses = retry_misses.len(),
            retries_remaining, "retrying exact stake misses after layer stale"
        );
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        retries_remaining -= 1;
        pending_misses = retry_misses;
    }

    Ok(updates)
}

fn decode_get_delegator(bytes: &[u8]) -> Result<DelegatorState> {
    let s = std::str::from_utf8(bytes).unwrap_or_default();
    let hex_str = s.trim_matches('"').trim_start_matches("0x");
    let raw = alloy::hex::decode(hex_str).context("decoding getDelegator eth_call return hex")?;
    let v = BondingManager::getDelegatorCall::abi_decode_returns(&raw, true)?;
    Ok(DelegatorState {
        bonded_amount: v.bondedAmount,
        delegate_address: v.delegateAddress,
    })
}

fn is_layer_stale_error(s: &str) -> bool {
    s.to_ascii_lowercase().contains("layer stale")
}

fn u256_to_decimal(u: &U256, decimals: u32) -> BigDecimal {
    let raw = BigDecimal::from_str(&u.to_string()).unwrap_or_default();
    raw / BigDecimal::from(10u128.pow(decimals))
}
