use alloy::primitives::{Address, FixedBytes, LogData, U256};
use alloy::sol;
use alloy::sol_types::{SolCall, SolEvent};
use anyhow::{Context, Result};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use futures::stream::{self, StreamExt, TryStreamExt};
use livepeer_core::{
    rpc::{cross_check, BlockTag, Provider},
    Config,
};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;
use tracing::info;

const ARBITRUM_CHAIN_ID: i64 = 42161;
const LPT_DECIMALS: u32 = 18;
const PERCENT_DENOMINATOR: i64 = 10_000;
const FULL_PERCENT_RAW: i64 = 1_000_000;
const ORCH_PROFILE_BATCH_LIMIT: i64 = 500;
/// Bounded concurrency for the per-NewRound orchestrator fanout.
/// Each known orch makes ~3-4 `eth_call`s inside `read_orchestrator_snapshot`,
/// so the effective in-flight RPC count is `concurrency × ~4`. Empirical
/// findings (2026-05-08):
/// - `concurrency = 32` and `= 24` both immediately tripped HTTP 429 from
///   Chainstack on cold-cache bursts at the first NewRound, even though
///   the post-cache `livepeer_rpc_calls_total` sustained rate was <2/s.
/// - `concurrency = 12` runs cleanly and sustained ~0.4 cached-miss
///   calls/sec measured over multiple iterations.
///   The bottleneck isn't the configured rate ceiling — it's Chainstack's
///   burst tolerance for the uncached calls that actually leave the host.
///   `livepeer_rpc_calls_total` only counts those, so the metric understates
///   the brief in-flight peak that triggers 429.
///
/// TD-022 attempted to replace this with JSON-RPC batching for a projected
/// ~100× RPC count reduction. Empirically the actual win was only ~1.5×
/// because: (a) Multicall3's universal CREATE2 deployment is NOT on
/// Arbitrum One, forcing a pivot to JSON-RPC batching; (b) Chainstack
/// rate-limits per constituent inside a batch, not per HTTP envelope, so
/// batch=100 still tripped the RPS limit and we had to back off to
/// batch=25; (c) the per-orch DB queries (`cache::get`, transcoder cuts,
/// lifecycle state) are still sequential and bound the iteration. The
/// 1.5× win didn't justify the complexity overhead — see TD-022 closure
/// for the full investigation. The `livepeer_core::rpc::multicall::
/// batch_call_cached` helper introduced for that work is still present
/// (no callers) and remains useful for future targeted batching.
const NEW_ROUND_FANOUT_CONCURRENCY: usize = 12;
const ORCH_PROFILE_CHECKPOINT: &str = "staker_orch_profile";

sol!(
    #[allow(missing_docs)]
    BondingManager,
    "../../abi/BondingManager.json"
);

sol!(
    #[allow(missing_docs)]
    Controller,
    "../../abi/Controller.json"
);

sol!(
    #[allow(missing_docs)]
    ServiceRegistry,
    "../../abi/ServiceRegistry.json"
);

#[derive(Debug, Default, Serialize)]
pub struct ProfileBackfillSummary {
    pub orch_events_seen: u64,
    pub orch_rows_written: u64,
    pub orchestrators_touched: u64,
    pub orch_checkpoint_block: Option<i64>,
}

#[derive(Debug, Clone)]
struct OrchCandidate {
    event_id: i64,
    event_name: String,
    block_number: i64,
    block_timestamp: DateTime<Utc>,
    block_hash: String,
    orchestrator_address: Option<String>,
    raw_event: Value,
}

#[derive(Debug)]
struct TranscoderCuts {
    reward_cut_percent: BigDecimal,
    fee_share_percent: BigDecimal,
    fee_cut_percent: BigDecimal,
}

/// Public result type for `fetch_active_transcoder_state`. Includes the
/// `last_reward_round` field so callers (e.g. the cuts backfill subcommand)
/// can detect the never-registered zero state without misinterpreting a
/// legitimate 0% cut.
#[derive(Debug, Clone)]
pub struct ActiveTranscoderState {
    /// `getTranscoder` slot 0. Zero when the address was never registered.
    pub last_reward_round: u64,
    pub reward_cut_percent: BigDecimal,
    pub fee_share_percent: BigDecimal,
    pub fee_cut_percent: BigDecimal,
}

#[derive(Debug)]
struct LifecycleState {
    is_active: bool,
    last_lifecycle_event_at: Option<DateTime<Utc>>,
}

#[derive(Debug)]
struct OrchestratorSnapshot {
    total_stake: BigDecimal,
    cuts: TranscoderCuts,
    lifecycle: LifecycleState,
    service_uri: Option<String>,
}

pub async fn run_profile_backfill(
    pg: &PgPool,
    archive: &Provider,
    cfg: &Config,
    include_tentative: bool,
) -> Result<ProfileBackfillSummary> {
    let orch_checkpoint = load_checkpoint(pg, ORCH_PROFILE_CHECKPOINT).await?;

    let orch_candidates = fetch_orch_candidates_after(
        pg,
        include_tentative,
        orch_checkpoint,
        ORCH_PROFILE_BATCH_LIMIT,
    )
    .await?;

    let mut summary = ProfileBackfillSummary {
        orch_events_seen: orch_candidates.len() as u64,
        orch_checkpoint_block: orch_checkpoint,
        ..Default::default()
    };
    info!(
        orch_checkpoint,
        orch_candidates = orch_candidates.len(),
        "profile backfill starting"
    );

    let mut known_orchs = load_known_orchestrators_before(pg, orch_checkpoint).await?;
    let mut current_round = load_current_round_before(pg, orch_checkpoint).await?;
    let bonding_manager = cfg.static_.contracts.bonding_manager.to_lowercase();
    let controller = cfg.static_.contracts.controller.to_lowercase();

    for candidate in &orch_candidates {
        if candidate.event_name != "TransferBond" {
            if let Some(addr) = candidate.orchestrator_address.as_ref() {
                known_orchs.insert(addr.clone());
            }
        }
    }

    let mut orch_touched = HashSet::new();
    let mut max_orch_block = orch_checkpoint;
    for candidate in orch_candidates {
        max_orch_block = Some(candidate.block_number);
        if candidate.event_name == "NewRound" {
            current_round = extract_round(&candidate.raw_event).or(current_round);

            // Walk the on-chain active pool ONCE per NewRound (~100 sequential
            // calls via the linked list). HashSet membership lookup then
            // gives `is_active` per orch in O(1) without any per-orch RPC.
            // This replaces the old event-history-based logic which missed
            // protocol-side auto-deactivations (orchs that fall out of the
            // pool from insufficient stake never emit TranscoderDeactivated).
            let block = candidate.block_number;
            let bonding_manager_ref = bonding_manager.as_str();
            let controller_ref = controller.as_str();
            let active_pool =
                Arc::new(fetch_active_pool(pg, archive, bonding_manager_ref, block).await?);

            // Bounded concurrent fanout: read all known-orch snapshots in
            // parallel (up to NEW_ROUND_FANOUT_CONCURRENCY in flight at
            // once), then upsert serially. RPC reads were the documented
            // bottleneck — DB writes are cheap once the data is in hand.
            let snapshots: Vec<(String, OrchestratorSnapshot)> =
                stream::iter(known_orchs.iter().cloned())
                    .map(|orch| {
                        let active_pool = active_pool.clone();
                        async move {
                            let snapshot = read_orchestrator_snapshot(
                                pg,
                                archive,
                                bonding_manager_ref,
                                controller_ref,
                                &orch,
                                block,
                                active_pool.as_ref(),
                            )
                            .await?;
                            Ok::<_, anyhow::Error>((orch, snapshot))
                        }
                    })
                    .buffer_unordered(NEW_ROUND_FANOUT_CONCURRENCY)
                    .try_collect()
                    .await?;

            // TD-026: bulk INSERT into orch_stake_by_round (per-round
            // historical table). The matview `orchestrator_profile` derives
            // from this. PK on (chain_id, address, round) + ON CONFLICT
            // DO NOTHING means re-runs at the same round are idempotent.
            let written =
                insert_orch_stake_by_round_batch(pg, &candidate, current_round, &snapshots).await?;
            summary.orch_rows_written += written;
            for (orch, _) in &snapshots {
                orch_touched.insert(orch.clone());
            }
            continue;
        }

        // TD-026: non-NewRound branches no longer write to a profile table.
        // Lifecycle (`is_active`, `last_lifecycle_event_at`) and cuts
        // (`latest_*_percent`) are derivable from `raw_protocol_events`
        // directly via SQL — the matview pulls them via a join. The only
        // useful side effect of this branch is updating `known_orchs` so
        // the next NewRound's fanout includes newly-bonded orchestrators.
        if candidate.event_name == "TransferBond" {
            let orch = resolve_transfer_bond_delegate(
                pg,
                archive,
                &bonding_manager,
                &candidate.raw_event,
                candidate.block_number,
            )
            .await?;
            known_orchs.insert(orch);
        } else if let Some(orch) = candidate.orchestrator_address.as_ref() {
            known_orchs.insert(orch.clone());
        }
    }

    if let Some(block) = max_orch_block {
        advance_checkpoint(pg, ORCH_PROFILE_CHECKPOINT, block).await?;
        summary.orch_checkpoint_block = Some(block);
    }
    summary.orchestrators_touched = orch_touched.len() as u64;

    info!(?summary, "profile backfill complete");
    Ok(summary)
}

async fn fetch_orch_candidates_after(
    pg: &PgPool,
    include_tentative: bool,
    resume_from_block: Option<i64>,
    limit: i64,
) -> Result<Vec<OrchCandidate>> {
    let finality_filter = if include_tentative {
        ""
    } else {
        "AND finality = 'finalized'"
    };
    let sql = format!(
        r#"SELECT id, event_name, block_number, block_timestamp, block_hash, to_address, raw_event
             FROM raw_protocol_events
            WHERE chain_id = $1
              AND is_canonical = TRUE
              {finality_filter}
              AND event_name IN (
                    'Bond',
                    'Unbond',
                    'Rebond',
                    'EarningsClaimed',
                    'TransferBond',
                    'TranscoderUpdate',
                    'TranscoderActivated',
                    'TranscoderDeactivated',
                    'NewRound'
              )
              AND ($2::bigint IS NULL OR block_number >= $2)
            ORDER BY block_number ASC, log_index ASC
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
        .map(|r| OrchCandidate {
            event_id: r.get("id"),
            event_name: r.get("event_name"),
            block_number: r.get("block_number"),
            block_timestamp: r.get("block_timestamp"),
            block_hash: r.get("block_hash"),
            orchestrator_address: r.try_get("to_address").ok(),
            raw_event: r.get("raw_event"),
        })
        .collect())
}

// TD-025: gateway candidate fetching, `read_gateway_snapshot`, and
// `upsert_broadcaster_profile` removed. `broadcaster_profile` is now a
// materialized view over `gateway_balances_by_block`. The original
// per-event walk (~600K events to materialize 13-50 rows) was fully
// redundant; `gateway_balance_backfill` produces the same snapshots into
// a strictly larger table.

async fn load_known_orchestrators_before(
    pg: &PgPool,
    resume_from_block: Option<i64>,
) -> Result<HashSet<String>> {
    let mut known: HashSet<String> = HashSet::new();

    if let Some(block) = resume_from_block {
        // Profile-derived addresses include TransferBond-resolved delegates,
        // whose discovery cost an RPC call the event scan below can't repeat.
        let rows = sqlx::query_scalar::<_, String>(
            r#"SELECT address
                 FROM orchestrator_profile
                WHERE chain_id = $1
                  AND as_of_block < $2"#,
        )
        .bind(ARBITRUM_CHAIN_ID)
        .bind(block)
        .fetch_all(pg)
        .await?;
        known.extend(rows);
    }

    // The lifecycle-event scan must ALWAYS run, not just on a cold start:
    // discovery is otherwise in-memory only, so an orchestrator whose
    // activation event was consumed in an earlier iteration that ended before
    // the next NewRound is in neither `orch_stake_by_round` nor the matview,
    // and the profile-derived set alone would exclude it from every future
    // NewRound fanout permanently (API 404 for a live orch).
    let sql = r#"SELECT DISTINCT to_address
                   FROM raw_protocol_events
                  WHERE chain_id = $1
                    AND is_canonical = TRUE
                    AND to_address IS NOT NULL
                    AND contract_name = 'BondingManager'
                    AND event_name IN (
                        'Bond',
                        'Unbond',
                        'Rebond',
                        'EarningsClaimed',
                        'TransferBond',
                        'TranscoderUpdate',
                        'TranscoderActivated',
                        'TranscoderDeactivated'
                    )
                    AND ($2::bigint IS NULL OR block_number < $2)"#;
    let rows = sqlx::query_scalar::<_, String>(sql)
        .bind(ARBITRUM_CHAIN_ID)
        .bind(resume_from_block)
        .fetch_all(pg)
        .await?;
    known.extend(rows);
    Ok(known)
}

async fn load_current_round_before(
    pg: &PgPool,
    resume_from_block: Option<i64>,
) -> Result<Option<i64>> {
    let row = sqlx::query(
        r#"SELECT raw_event -> 'decoded' ->> 'round' AS round
             FROM raw_protocol_events
            WHERE chain_id = $1
              AND is_canonical = TRUE
              AND event_name = 'NewRound'
              AND ($2::bigint IS NULL OR block_number < $2)
            ORDER BY block_number DESC, log_index DESC
            LIMIT 1"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(resume_from_block)
    .fetch_optional(pg)
    .await?;
    Ok(row
        .and_then(|r| r.try_get::<Option<String>, _>("round").ok().flatten())
        .and_then(|v| v.parse::<i64>().ok()))
}

fn extract_round(raw_event: &Value) -> Option<i64> {
    raw_event
        .get("decoded")
        .and_then(|d| d.get("round"))
        .and_then(Value::as_str)
        .and_then(|v| v.parse::<i64>().ok())
}

async fn read_orchestrator_snapshot(
    pg: &PgPool,
    archive: &Provider,
    bonding_manager: &str,
    controller: &str,
    orchestrator: &str,
    block_number: i64,
    active_pool: &HashSet<String>,
) -> Result<OrchestratorSnapshot> {
    let orch_addr = Address::from_str(orchestrator).context("parsing orchestrator address")?;
    let stake_data = format!(
        "0x{}",
        alloy::hex::encode(
            BondingManager::transcoderTotalStakeCall {
                _transcoder: orch_addr,
            }
            .abi_encode()
        )
    );
    let stake_params = json!([
        { "to": bonding_manager, "data": stake_data },
        BlockTag::Number(block_number as u64).to_param()
    ]);
    let stake_outcome =
        cross_check::single_call_cached(pg, archive, "eth_call", &stake_params, Some(block_number))
            .await?;
    let stake_raw = decode_hex_result(&stake_outcome.response_bytes)?;
    let stake = BondingManager::transcoderTotalStakeCall::abi_decode_returns(&stake_raw, true)?;

    let cuts =
        read_active_transcoder_cuts(pg, archive, bonding_manager, orchestrator, block_number)
            .await?;
    // is_active = membership in the on-chain active pool at this block.
    // Falls back to event-history-based last_lifecycle_event_at for the
    // timestamp only (the bool was unreliable because the protocol can
    // auto-deactivate without emitting TranscoderDeactivated).
    let last_lifecycle_event_at =
        load_last_lifecycle_event_at(pg, orchestrator, block_number).await?;
    let lifecycle = LifecycleState {
        is_active: active_pool.contains(&orchestrator.to_lowercase()),
        last_lifecycle_event_at,
    };
    let service_registry =
        resolve_service_registry_address(pg, archive, controller, block_number).await?;
    let service_uri = match service_registry.as_deref() {
        Some(addr) => read_service_uri(pg, archive, addr, orchestrator, block_number).await?,
        None => None,
    };
    Ok(OrchestratorSnapshot {
        total_stake: u256_to_decimal(&stake._0, LPT_DECIMALS),
        cuts,
        lifecycle,
        service_uri,
    })
}

/// Walk the on-chain active orchestrator pool linked list once and return
/// the membership set. Each address is lowercased for case-insensitive
/// comparison against the snake-case stored orch addresses.
///
/// The pool changes only at NewRound boundaries, so this is exactly one
/// walk per profile-backfill iteration (which itself runs per NewRound).
/// Walks via `getFirstTranscoderInPool` then `getNextTranscoderInPool`
/// repeatedly until the linked list returns the zero address. Each call
/// goes through `single_call_cached` so the walk is deterministic and
/// replay-friendly.
async fn fetch_active_pool(
    pg: &PgPool,
    archive: &Provider,
    bonding_manager: &str,
    block_number: i64,
) -> Result<HashSet<String>> {
    let mut pool = HashSet::new();
    let first_data = format!(
        "0x{}",
        alloy::hex::encode(BondingManager::getFirstTranscoderInPoolCall {}.abi_encode())
    );
    let first_params = json!([
        { "to": bonding_manager, "data": first_data },
        BlockTag::Number(block_number as u64).to_param()
    ]);
    let first_outcome =
        cross_check::single_call_cached(pg, archive, "eth_call", &first_params, Some(block_number))
            .await?;
    let first_raw = decode_hex_result(&first_outcome.response_bytes)?;
    let mut current =
        BondingManager::getFirstTranscoderInPoolCall::abi_decode_returns(&first_raw, true)?._0;

    while current != Address::ZERO {
        pool.insert(format!("{current:#x}").to_lowercase());
        let next_data = format!(
            "0x{}",
            alloy::hex::encode(
                BondingManager::getNextTranscoderInPoolCall {
                    _transcoder: current,
                }
                .abi_encode()
            )
        );
        let next_params = json!([
            { "to": bonding_manager, "data": next_data },
            BlockTag::Number(block_number as u64).to_param()
        ]);
        let next_outcome = cross_check::single_call_cached(
            pg,
            archive,
            "eth_call",
            &next_params,
            Some(block_number),
        )
        .await?;
        let next_raw = decode_hex_result(&next_outcome.response_bytes)?;
        current =
            BondingManager::getNextTranscoderInPoolCall::abi_decode_returns(&next_raw, true)?._0;
    }
    Ok(pool)
}

/// Reads the orch's **active** `rewardCut` / `feeShare` from chain storage
/// at `block_number` via `BondingManager.getTranscoder`.
///
/// The previous implementation read these from the most recent
/// `TranscoderUpdate` event in `raw_protocol_events`. That event carries the
/// `pending` values set by `transcoder(rewardCut, feeShare)` — the protocol
/// only copies pending → active when the orch calls `reward()` in a
/// subsequent round, and that copy emits no event. Orchs that requested a
/// change but haven't earned since had stale-but-divergent cuts in the API.
async fn read_active_transcoder_cuts(
    pg: &PgPool,
    archive: &Provider,
    bonding_manager: &str,
    orchestrator: &str,
    block_number: i64,
) -> Result<TranscoderCuts> {
    let state =
        fetch_active_transcoder_state(pg, archive, bonding_manager, orchestrator, block_number)
            .await?;
    Ok(TranscoderCuts {
        reward_cut_percent: state.reward_cut_percent,
        fee_share_percent: state.fee_share_percent,
        fee_cut_percent: state.fee_cut_percent,
    })
}

/// Reads the full active `getTranscoder` view (3 fields exposed): the
/// `lastRewardRound` slot plus the three percentage values. Public so the
/// historical-cuts backfill subcommand can use `last_reward_round == 0` as
/// the never-registered discriminator (avoids overwriting a legitimate 0%
/// reward_cut with the chain's all-zero unregistered state).
pub async fn fetch_active_transcoder_state(
    pg: &PgPool,
    archive: &Provider,
    bonding_manager: &str,
    orchestrator: &str,
    block_number: i64,
) -> Result<ActiveTranscoderState> {
    let orch_addr = Address::from_str(orchestrator).context("parsing orchestrator address")?;
    let data = format!(
        "0x{}",
        alloy::hex::encode(
            BondingManager::getTranscoderCall {
                _transcoder: orch_addr,
            }
            .abi_encode()
        )
    );
    let params = json!([
        { "to": bonding_manager, "data": data },
        BlockTag::Number(block_number as u64).to_param()
    ]);
    let outcome =
        cross_check::single_call_cached(pg, archive, "eth_call", &params, Some(block_number))
            .await?;
    let raw = decode_hex_result(&outcome.response_bytes)?;
    let decoded = BondingManager::getTranscoderCall::abi_decode_returns(&raw, true)?;

    let reward_cut_raw = decoded.rewardCut.to_string();
    let fee_share_raw = decoded.feeShare.to_string();

    let reward_cut_percent = raw_percent_to_decimal(&reward_cut_raw)?;
    let fee_share_percent = raw_percent_to_decimal(&fee_share_raw)?;
    let fee_cut_percent = inverse_raw_percent_to_decimal(&fee_share_raw)?;

    // U256 → u64 truncation. Round numbers are tiny (~4k today, ~1M in a
    // century), so this fits trivially.
    let last_reward_round = decoded.lastRewardRound.try_into().unwrap_or(u64::MAX);

    Ok(ActiveTranscoderState {
        last_reward_round,
        reward_cut_percent,
        fee_share_percent,
        fee_cut_percent,
    })
}

/// Returns the most recent TranscoderActivated/Deactivated timestamp for
/// the orch, used purely for `last_lifecycle_event_at` display.
///
/// `is_active` is NOT derived from this — see `fetch_active_pool` instead.
/// The protocol can auto-deactivate orchs (when stake falls below the
/// lowest-active threshold) without emitting TranscoderDeactivated, so
/// event-history would mislabel them as active forever.
async fn load_last_lifecycle_event_at(
    pg: &PgPool,
    orchestrator: &str,
    block_number: i64,
) -> Result<Option<DateTime<Utc>>> {
    let row = sqlx::query(
        r#"SELECT block_timestamp
             FROM raw_protocol_events
            WHERE chain_id = $1
              AND is_canonical = TRUE
              AND event_name IN ('TranscoderActivated', 'TranscoderDeactivated')
              AND to_address = $2
              AND block_number <= $3
            ORDER BY block_number DESC, log_index DESC
            LIMIT 1"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(orchestrator)
    .bind(block_number)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(|r| r.get("block_timestamp")))
}

/// TD-026: bulk INSERT one (orch, round) row per snapshot from the
/// NewRound fanout. Returns the count of rows actually inserted (after
/// ON CONFLICT — re-runs at the same round are no-ops). The matview
/// `orchestrator_profile` derives from this table on REFRESH.
async fn insert_orch_stake_by_round_batch(
    pg: &PgPool,
    candidate: &OrchCandidate,
    current_round: Option<i64>,
    snapshots: &[(String, OrchestratorSnapshot)],
) -> Result<u64> {
    if snapshots.is_empty() {
        return Ok(0);
    }
    // We must have a round id to write — the table's PK is keyed on it.
    // If the indexer hasn't seen a NewRound yet we silently skip; the
    // next NewRound iter will write properly.
    let Some(round) = current_round else {
        return Ok(0);
    };

    let mut qb: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
        "INSERT INTO orch_stake_by_round (\
            chain_id, address, round, block_number, block_timestamp, block_hash, \
            total_stake, service_uri, latest_fee_cut_percent, latest_reward_cut_percent, \
            latest_fee_share_percent, is_active, last_lifecycle_event_at, \
            triggering_event_id\
         ) ",
    );
    qb.push_values(snapshots.iter(), |mut b, (orch, snap)| {
        b.push_bind(ARBITRUM_CHAIN_ID)
            .push_bind(orch)
            .push_bind(round)
            .push_bind(candidate.block_number)
            .push_bind(candidate.block_timestamp)
            .push_bind(&candidate.block_hash)
            .push_bind(&snap.total_stake)
            .push_bind(&snap.service_uri)
            .push_bind(&snap.cuts.fee_cut_percent)
            .push_bind(&snap.cuts.reward_cut_percent)
            .push_bind(&snap.cuts.fee_share_percent)
            .push_bind(snap.lifecycle.is_active)
            .push_bind(snap.lifecycle.last_lifecycle_event_at)
            .push_bind(candidate.event_id);
    });
    qb.push(" ON CONFLICT (chain_id, address, round) DO NOTHING");
    let result = qb
        .build()
        .execute(pg)
        .await
        .context("bulk-inserting orch_stake_by_round")?;
    Ok(result.rows_affected())
}

async fn resolve_service_registry_address(
    pg: &PgPool,
    archive: &Provider,
    controller: &str,
    block_number: i64,
) -> Result<Option<String>> {
    let contract_id = alloy::primitives::keccak256(b"ServiceRegistry");
    let call_data = format!(
        "0x{}",
        alloy::hex::encode(Controller::getContractCall { _id: contract_id }.abi_encode())
    );
    let params = json!([
        { "to": controller, "data": call_data },
        BlockTag::Number(block_number as u64).to_param()
    ]);
    let outcome =
        cross_check::single_call_cached(pg, archive, "eth_call", &params, Some(block_number))
            .await?;
    let raw = decode_hex_result(&outcome.response_bytes)?;
    let resolved = Controller::getContractCall::abi_decode_returns(&raw, true)?;
    let address = format!("{:#x}", resolved._0).to_lowercase();
    if address == "0x0000000000000000000000000000000000000000" {
        Ok(None)
    } else {
        Ok(Some(address))
    }
}

async fn read_service_uri(
    pg: &PgPool,
    archive: &Provider,
    service_registry: &str,
    orchestrator: &str,
    block_number: i64,
) -> Result<Option<String>> {
    let orch_addr =
        Address::from_str(orchestrator).context("parsing orchestrator address for service uri")?;
    let call_data = format!(
        "0x{}",
        alloy::hex::encode(ServiceRegistry::getServiceURICall { _addr: orch_addr }.abi_encode())
    );
    let params = json!([
        { "to": service_registry, "data": call_data },
        BlockTag::Number(block_number as u64).to_param()
    ]);
    let outcome =
        cross_check::single_call_cached(pg, archive, "eth_call", &params, Some(block_number))
            .await?;
    let raw = decode_hex_result(&outcome.response_bytes)?;
    let uri = ServiceRegistry::getServiceURICall::abi_decode_returns(&raw, true)?._0;
    if uri.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(uri))
    }
}

fn decode_hex_result(bytes: &[u8]) -> Result<Vec<u8>> {
    let s = std::str::from_utf8(bytes).unwrap_or_default();
    let hex_str = s.trim_matches('"').trim_start_matches("0x");
    alloy::hex::decode(hex_str).context("decoding eth_call return hex")
}

fn decode_transfer_bond_new_delegator(raw_event: &Value) -> Result<String> {
    let raw: RawLog =
        serde_json::from_value(raw_event.clone()).context("decoding raw TransferBond log")?;
    let log_data = build_log_data(&raw)?;
    let decoded = BondingManager::TransferBond::decode_log_data(&log_data, true)
        .context("ABI-decoding TransferBond")?;
    Ok(format!("{:#x}", decoded.newDelegator).to_lowercase())
}

async fn resolve_transfer_bond_delegate(
    pg: &PgPool,
    archive: &Provider,
    bonding_manager: &str,
    raw_event: &Value,
    block_number: i64,
) -> Result<String> {
    let new_delegator = decode_transfer_bond_new_delegator(raw_event)?;
    let delegator =
        Address::from_str(&new_delegator).context("parsing TransferBond newDelegator")?;
    let call_data = format!(
        "0x{}",
        alloy::hex::encode(
            BondingManager::getDelegatorCall {
                _delegator: delegator
            }
            .abi_encode()
        )
    );
    let params = json!([
        { "to": bonding_manager, "data": call_data },
        BlockTag::Number(block_number as u64).to_param()
    ]);
    let outcome =
        cross_check::single_call_cached(pg, archive, "eth_call", &params, Some(block_number))
            .await?;
    let raw = decode_hex_result(&outcome.response_bytes)?;
    let decoded = BondingManager::getDelegatorCall::abi_decode_returns(&raw, true)?;
    Ok(format!("{:#x}", decoded.delegateAddress).to_lowercase())
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

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawLog {
    topics: Vec<String>,
    data: String,
}

fn u256_to_decimal(u: &U256, decimals: u32) -> BigDecimal {
    let raw = BigDecimal::from_str(&u.to_string()).unwrap_or_default();
    raw / BigDecimal::from(10u128.pow(decimals))
}

fn raw_percent_to_decimal(raw: &str) -> Result<BigDecimal> {
    let value = BigDecimal::from_str(raw).context("parsing raw percent")?;
    Ok(value / BigDecimal::from(PERCENT_DENOMINATOR))
}

fn inverse_raw_percent_to_decimal(raw: &str) -> Result<BigDecimal> {
    let value = BigDecimal::from_str(raw).context("parsing raw percent")?;
    Ok((BigDecimal::from(FULL_PERCENT_RAW) - value) / BigDecimal::from(PERCENT_DENOMINATOR))
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
