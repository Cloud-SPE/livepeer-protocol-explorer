//! Round-anchored current-stake refresh.
//!
//! `stake_balances_by_block` rows are event-triggered: a delegator's bonded
//! amount is only re-observed when *they* act (bond / unbond / claim). Between
//! claims the protocol compounds rewards into their stake every round, so a
//! passive delegator's latest row understates their true stake — by years'
//! worth of compounding in the worst case.
//!
//! This worker closes that gap. Once per protocol round, anchored at the
//! latest finalized `NewRound` event block (a deterministic, cacheable block —
//! the same anchoring the profile worker uses for `orch_stake_by_round`), it
//! reads contract truth for every delegator whose latest row claims a
//! positive bonded principal:
//!
//!   - `BondingManager.getDelegator(d)`        → bondedAmount, delegateAddress
//!   - `BondingManager.pendingStake(d, round)` → stake incl. compounded rewards
//!
//! and upserts a fresh row at the anchor block (`source = 'round_refresh'`).
//! Because `getDelegator` is authoritative, this also self-heals any residual
//! stale state (a delegator who unbonded or moved gets a truthful row), and
//! `delegator_registry.is_active` is updated to match.
//!
//! All reads go through `rpc_call_cache` at a pinned block, so the
//! deterministic-replay invariant holds. Like `tx-receipts-backfill`, this
//! stage is not part of `livepeer-orchestrator replay`.

use alloy::primitives::{Address, U256};
use alloy::sol;
use alloy::sol_types::SolCall;
use anyhow::{bail, Context, Result};
use bigdecimal::BigDecimal;
use livepeer_core::rpc::{cross_check, Provider};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgPool, Row};
use std::str::FromStr;
use tracing::{info, warn};

const ARBITRUM_CHAIN_ID: i64 = 42161;
const LPT_DECIMALS: u32 = 18;
const SOURCE_ROUND_REFRESH: &str = "round_refresh";
const ROUND_REFRESH_CHECKPOINT: &str = "staker_round_stake_refresh";
const RPC_BATCH_SIZE: usize = 250;
const LAYER_STALE_RETRIES: usize = 3;
const UPSERT_CHUNK: usize = 1_000;

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
    }
}

#[derive(Debug, Default, Serialize)]
pub struct RoundRefreshSummary {
    pub round: Option<i64>,
    pub anchor_block: Option<i64>,
    pub candidates: u64,
    pub rows_refreshed: u64,
    pub zeroed: u64,
}

#[derive(Debug)]
struct NewRoundAnchor {
    event_id: i64,
    round: i64,
    block_number: i64,
    block_timestamp: chrono::DateTime<chrono::Utc>,
    block_hash: String,
}

#[derive(Debug)]
struct RefreshedState {
    delegator: String,
    bonded_principal: BigDecimal,
    delegate_address: String,
    pending_stake: BigDecimal,
    raw_call: Value,
}

pub async fn refresh_current_stake(
    pg: &PgPool,
    archive: &Provider,
    bonding_manager: &str,
    include_tentative: bool,
) -> Result<RoundRefreshSummary> {
    let mut summary = RoundRefreshSummary::default();

    let Some(anchor) = latest_new_round(pg, include_tentative).await? else {
        info!("round refresh: no NewRound event indexed yet");
        return Ok(summary);
    };
    summary.round = Some(anchor.round);
    summary.anchor_block = Some(anchor.block_number);

    let checkpoint = load_checkpoint(pg).await?;
    if checkpoint.is_some_and(|done| done >= anchor.round) {
        // Already refreshed at this round; tick updated_at for liveness.
        advance_checkpoint(pg, anchor.round.min(checkpoint.unwrap_or(0))).await?;
        return Ok(summary);
    }

    let candidates = load_candidates(pg, anchor.block_number).await?;
    summary.candidates = candidates.len() as u64;
    info!(
        round = anchor.round,
        anchor_block = anchor.block_number,
        candidates = candidates.len(),
        "round refresh starting"
    );

    let states =
        read_states_batched(pg, archive, bonding_manager, &anchor, &candidates).await?;

    let zero = BigDecimal::from(0u64);
    summary.zeroed = states
        .iter()
        .filter(|s| s.bonded_principal == zero)
        .count() as u64;

    for chunk in states.chunks(UPSERT_CHUNK) {
        upsert_rows(pg, &anchor, chunk).await?;
        update_registry_activity(pg, chunk).await?;
        summary.rows_refreshed += chunk.len() as u64;
    }

    advance_checkpoint(pg, anchor.round).await?;
    info!(?summary, "round refresh complete");
    Ok(summary)
}

async fn latest_new_round(pg: &PgPool, include_tentative: bool) -> Result<Option<NewRoundAnchor>> {
    let finality_filter = if include_tentative {
        ""
    } else {
        "AND finality = 'finalized'"
    };
    let sql = format!(
        r#"SELECT id, block_number, block_timestamp, block_hash,
                  raw_event -> 'decoded' ->> 'round' AS round
             FROM raw_protocol_events
            WHERE chain_id = $1
              AND is_canonical = TRUE
              AND event_name = 'NewRound'
              {finality_filter}
            ORDER BY block_number DESC, log_index DESC
            LIMIT 1"#
    );
    let row = sqlx::query(&sql)
        .bind(ARBITRUM_CHAIN_ID)
        .fetch_optional(pg)
        .await?;
    Ok(row.and_then(|r| {
        let round: Option<String> = r.try_get("round").ok().flatten();
        let round = round.and_then(|v| v.parse::<i64>().ok())?;
        Some(NewRoundAnchor {
            event_id: r.get("id"),
            round,
            block_number: r.get("block_number"),
            block_timestamp: r.get("block_timestamp"),
            block_hash: r.get("block_hash"),
        })
    }))
}

/// Delegators whose latest row claims positive bonded principal and predates
/// the anchor block. Rows at or after the anchor are already fresher than
/// this refresh would make them.
async fn load_candidates(pg: &PgPool, anchor_block: i64) -> Result<Vec<String>> {
    let rows = sqlx::query(
        r#"
        WITH latest AS (
            SELECT DISTINCT ON (delegator_address)
                   delegator_address, bonded_principal, block_number
              FROM stake_balances_by_block
             WHERE chain_id = $1
             ORDER BY delegator_address, block_number DESC
        )
        SELECT delegator_address
          FROM latest
         WHERE bonded_principal > 0 AND block_number < $2
         ORDER BY delegator_address"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(anchor_block)
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(|r| r.get::<String, _>(0)).collect())
}

async fn read_states_batched(
    pg: &PgPool,
    archive: &Provider,
    bonding_manager: &str,
    anchor: &NewRoundAnchor,
    candidates: &[String],
) -> Result<Vec<RefreshedState>> {
    let mut states = Vec::with_capacity(candidates.len());

    for chunk in candidates.chunks(RPC_BATCH_SIZE / 2) {
        // Two calls per delegator, interleaved: [getDelegator, pendingStake, ...]
        let mut requests: Vec<(String, Value, Option<i64>)> = Vec::with_capacity(chunk.len() * 2);
        let mut addrs: Vec<(&String, Address)> = Vec::with_capacity(chunk.len());
        for delegator in chunk {
            let addr = Address::from_str(delegator)
                .with_context(|| format!("invalid delegator address {delegator}"))?;
            addrs.push((delegator, addr));
            requests.push((
                "eth_call".to_string(),
                get_delegator_params(bonding_manager, addr, anchor.block_number),
                Some(anchor.block_number),
            ));
            requests.push((
                "eth_call".to_string(),
                pending_stake_params(bonding_manager, addr, anchor.round, anchor.block_number),
                Some(anchor.block_number),
            ));
        }

        let outcomes = batch_with_retries(pg, archive, &requests).await?;
        for (i, (delegator, _)) in addrs.iter().enumerate() {
            let state_bytes = &outcomes[i * 2];
            let pending_bytes = &outcomes[i * 2 + 1];
            let state = decode_get_delegator(state_bytes)?;
            let pending = decode_pending_stake(pending_bytes)?;
            states.push(RefreshedState {
                delegator: (*delegator).clone(),
                bonded_principal: u256_to_decimal(&state.bonded_amount, LPT_DECIMALS),
                delegate_address: format!("{:#x}", state.delegate_address).to_lowercase(),
                pending_stake: u256_to_decimal(&pending, LPT_DECIMALS),
                raw_call: serde_json::json!({
                    "bondedAmount_wei": state.bonded_amount.to_string(),
                    "pendingStake_wei": pending.to_string(),
                    "round": anchor.round.to_string(),
                }),
            });
        }
    }
    Ok(states)
}

/// Run one chunk of requests through `batch_call_cached`, retrying
/// layer-stale errors a bounded number of times. Any other error aborts the
/// refresh so the checkpoint does not advance past unread state.
async fn batch_with_retries(
    pg: &PgPool,
    archive: &Provider,
    requests: &[(String, Value, Option<i64>)],
) -> Result<Vec<Vec<u8>>> {
    let mut responses: Vec<Option<Vec<u8>>> = vec![None; requests.len()];
    let mut pending_idx: Vec<usize> = (0..requests.len()).collect();

    for attempt in 0..=LAYER_STALE_RETRIES {
        let batch: Vec<(String, Value, Option<i64>)> = pending_idx
            .iter()
            .map(|&i| requests[i].clone())
            .collect();
        let results = cross_check::batch_call_cached(pg, archive, &batch).await?;
        let mut retry_idx = Vec::new();
        for (slot, result) in pending_idx.iter().zip(results.into_iter()) {
            match result {
                Ok(outcome) => responses[*slot] = Some(outcome.response_bytes),
                Err(e) if is_layer_stale_error(&e.to_string()) => retry_idx.push(*slot),
                Err(e) => return Err(e.into()),
            }
        }
        if retry_idx.is_empty() {
            break;
        }
        if attempt == LAYER_STALE_RETRIES {
            bail!(
                "round refresh: {} calls still layer-stale after {} retries",
                retry_idx.len(),
                LAYER_STALE_RETRIES
            );
        }
        warn!(
            retries = retry_idx.len(),
            attempt, "round refresh: retrying layer-stale calls"
        );
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        pending_idx = retry_idx;
    }

    Ok(responses
        .into_iter()
        .map(|r| r.expect("all slots resolved or bailed"))
        .collect())
}

async fn upsert_rows(pg: &PgPool, anchor: &NewRoundAnchor, states: &[RefreshedState]) -> Result<()> {
    let mut delegators: Vec<&str> = Vec::with_capacity(states.len());
    let mut delegates: Vec<&str> = Vec::with_capacity(states.len());
    let mut bondeds: Vec<BigDecimal> = Vec::with_capacity(states.len());
    let mut pendings: Vec<BigDecimal> = Vec::with_capacity(states.len());
    let mut raws: Vec<Value> = Vec::with_capacity(states.len());
    for s in states {
        delegators.push(&s.delegator);
        delegates.push(&s.delegate_address);
        bondeds.push(s.bonded_principal.clone());
        pendings.push(s.pending_stake.clone());
        raws.push(s.raw_call.clone());
    }
    sqlx::query(
        r#"INSERT INTO stake_balances_by_block
              (chain_id, delegator_address, delegate_address, block_number, block_timestamp,
               block_hash, bonded_principal, pending_stake, pending_round, source,
               raw_call, triggering_event_id)
           SELECT $1, v.delegator, v.delegate, $2, $3, $4, v.bonded, v.pending, $5, $6,
                  v.raw_call, $7
             FROM unnest($8::text[], $9::text[], $10::numeric[], $11::numeric[], $12::jsonb[])
                  AS v(delegator, delegate, bonded, pending, raw_call)
           ON CONFLICT (chain_id, delegator_address, block_number) DO UPDATE
              SET delegate_address = EXCLUDED.delegate_address,
                  bonded_principal = EXCLUDED.bonded_principal,
                  pending_stake    = EXCLUDED.pending_stake,
                  pending_round    = EXCLUDED.pending_round,
                  source           = EXCLUDED.source,
                  raw_call         = EXCLUDED.raw_call"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(anchor.block_number)
    .bind(anchor.block_timestamp)
    .bind(&anchor.block_hash)
    .bind(anchor.round)
    .bind(SOURCE_ROUND_REFRESH)
    .bind(anchor.event_id)
    .bind(&delegators)
    .bind(&delegates)
    .bind(&bondeds)
    .bind(&pendings)
    .bind(&raws)
    .execute(pg)
    .await?;
    Ok(())
}

async fn update_registry_activity(pg: &PgPool, states: &[RefreshedState]) -> Result<()> {
    let mut delegators: Vec<&str> = Vec::with_capacity(states.len());
    let mut actives: Vec<bool> = Vec::with_capacity(states.len());
    let zero = BigDecimal::from(0u64);
    for s in states {
        delegators.push(&s.delegator);
        actives.push(s.bonded_principal > zero);
    }
    sqlx::query(
        r#"UPDATE delegator_registry r
              SET is_active = v.is_active
             FROM unnest($2::text[], $3::bool[]) AS v(delegator, is_active)
            WHERE r.chain_id = $1 AND r.delegator_address = v.delegator"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(&delegators)
    .bind(&actives)
    .execute(pg)
    .await?;
    Ok(())
}

async fn load_checkpoint(pg: &PgPool) -> Result<Option<i64>> {
    let checkpoint = sqlx::query_scalar::<_, i64>(
        "SELECT last_processed_block FROM indexer_checkpoints WHERE name = $1",
    )
    .bind(ROUND_REFRESH_CHECKPOINT)
    .fetch_optional(pg)
    .await?;
    Ok(checkpoint)
}

/// `last_processed_block` stores the last refreshed ROUND for this checkpoint.
async fn advance_checkpoint(pg: &PgPool, round: i64) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO indexer_checkpoints (name, chain_id, last_processed_block, updated_at)
           VALUES ($1, $2, $3, now())
           ON CONFLICT (name) DO UPDATE
              SET last_processed_block = GREATEST(indexer_checkpoints.last_processed_block, EXCLUDED.last_processed_block),
                  updated_at = now()"#,
    )
    .bind(ROUND_REFRESH_CHECKPOINT)
    .bind(ARBITRUM_CHAIN_ID)
    .bind(round)
    .execute(pg)
    .await?;
    Ok(())
}

fn get_delegator_params(bonding_manager: &str, delegator: Address, block_number: i64) -> Value {
    let calldata = BondingManager::getDelegatorCall {
        _delegator: delegator,
    }
    .abi_encode();
    eth_call_params(bonding_manager, &calldata, block_number)
}

fn pending_stake_params(
    bonding_manager: &str,
    delegator: Address,
    round: i64,
    block_number: i64,
) -> Value {
    let calldata = BondingManager::pendingStakeCall {
        _delegator: delegator,
        _endRound: U256::from(round as u64),
    }
    .abi_encode();
    eth_call_params(bonding_manager, &calldata, block_number)
}

fn eth_call_params(bonding_manager: &str, calldata: &[u8], block_number: i64) -> Value {
    let data = format!("0x{}", alloy::hex::encode(calldata));
    serde_json::json!([{ "to": bonding_manager, "data": data }, format!("0x{:x}", block_number as u64)])
}

struct DelegatorState {
    bonded_amount: U256,
    delegate_address: Address,
}

fn decode_get_delegator(bytes: &[u8]) -> Result<DelegatorState> {
    let raw = decode_response_hex(bytes)?;
    let v = BondingManager::getDelegatorCall::abi_decode_returns(&raw, true)?;
    Ok(DelegatorState {
        bonded_amount: v.bondedAmount,
        delegate_address: v.delegateAddress,
    })
}

fn decode_pending_stake(bytes: &[u8]) -> Result<U256> {
    let raw = decode_response_hex(bytes)?;
    let v = BondingManager::pendingStakeCall::abi_decode_returns(&raw, true)?;
    Ok(v._0)
}

fn decode_response_hex(bytes: &[u8]) -> Result<Vec<u8>> {
    let s = std::str::from_utf8(bytes).unwrap_or_default();
    let hex_str = s.trim_matches('"').trim_start_matches("0x");
    alloy::hex::decode(hex_str).context("decoding eth_call return hex")
}

fn is_layer_stale_error(s: &str) -> bool {
    s.to_ascii_lowercase().contains("layer stale")
}

fn u256_to_decimal(u: &U256, decimals: u32) -> BigDecimal {
    let raw = BigDecimal::from_str(&u.to_string()).unwrap_or_default();
    raw / BigDecimal::from(10u128.pow(decimals))
}

