//! Flow-derived `bonded_principal` per delegator + delegator_registry. SPEC §11.10, §11.11.
//!
//! v2 — delegation-state correctness rework:
//!
//! - **Checkpoint by event id, not block number.** The previous walker
//!   checkpointed on `block_number` with a `GREATEST`-only advance, so any
//!   stake-flow event ingested into `raw_protocol_events` for a block the
//!   walker had already passed (gap repair, out-of-order initial indexing,
//!   late finality) was skipped permanently. Event ids are assigned in
//!   ingestion order, so `id > checkpoint` always picks up late arrivals —
//!   the same scheme `orch_rewards` uses, which is why reward rollups never
//!   exhibited the holes the stake table did.
//!
//! - **Per-delegator full-history replay.** Each batch collects the set of
//!   delegators touched by new events, then re-derives every touched
//!   delegator's complete row history from their full event stream in
//!   `(block_number, log_index)` order. Replay is a pure function of the
//!   delegator's events, so writes are idempotent (core belief #8) and a
//!   late-arriving event simply causes that delegator's history to be
//!   rebuilt correctly. The first run after deploying v2 starts from a fresh
//!   checkpoint name and therefore re-derives the whole table — that is the
//!   intended one-time repair of historical holes.
//!
//! - **`delegator_registry.is_active` is maintained.** Previously set TRUE at
//!   first bond and never touched again; now updated on every replay to
//!   `final balance > 0`.
//!
//! Per SPEC §11.11 the registry is derived from `Bond` events. Non-Bond
//! events on delegators we've never seen a Bond for are skipped with a
//! warning — their pre-window state is unknown. The full-genesis backfill
//! would not have this issue.

use anyhow::{Context, Result};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgPool, Row};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::str::FromStr;
use tracing::{info, warn};

const ARBITRUM_CHAIN_ID: i64 = 42161;
const SOURCE_FLOW: &str = "flow_derived";
const SOURCE_EARNINGS_BOOTSTRAP: &str = "earnings_claimed_bootstrap";
/// v2 checkpoint stores the last processed `raw_protocol_events.id` (the
/// `last_processed_block` column is reused for the id, as `orch_rewards`
/// does). The v1 block-number checkpoint (`staker_flow_backfill`) is
/// intentionally abandoned so the first v2 run re-derives all history.
const FLOW_CHECKPOINT: &str = "staker_flow_backfill_v2";
const FLOW_BATCH_SIZE: i64 = 5_000;
const ZERO_ADDRESS: &str = "0x0000000000000000000000000000000000000000";

#[derive(Debug, Default, Serialize)]
pub struct FlowSummary {
    pub events_seen: u64,
    pub delegators_replayed: u64,
    pub stake_rows_written: u64,
    pub delegators_registered: u64,
    pub skipped_unregistered: u64,
    pub checkpoint_event_id: Option<i64>,
}

#[derive(Debug, Clone)]
struct StakeEvent {
    event_id: i64,
    event_name: String,
    block_number: i64,
    block_timestamp: DateTime<Utc>,
    block_hash: String,
    from_address: Option<String>,
    to_address: Option<String>,
    amount_normalized: Option<BigDecimal>,
    raw_event: serde_json::Value,
}

/// One stake row to persist: the delegator's state at the end of a block.
#[derive(Debug, Clone, PartialEq)]
struct RowWrite {
    block_number: i64,
    block_timestamp: DateTime<Utc>,
    block_hash: String,
    delegate_address: String,
    bonded_principal: BigDecimal,
    source: &'static str,
    triggering_event_id: i64,
}

/// Registry state derived from a full replay.
#[derive(Debug, Clone, PartialEq)]
struct RegistryWrite {
    first_bond_block: i64,
    first_bond_event_id: i64,
    last_seen_block: i64,
    last_seen_event_id: i64,
    is_active: bool,
}

#[derive(Debug, Default)]
struct ReplayOutcome {
    rows: Vec<RowWrite>,
    registry: Option<RegistryWrite>,
    skipped_unregistered: u64,
}

pub async fn run_flow_backfill(pg: &PgPool, include_tentative: bool) -> Result<FlowSummary> {
    let checkpoint = load_flow_checkpoint(pg).await?;
    let new_events = fetch_new_event_heads(pg, include_tentative, checkpoint, FLOW_BATCH_SIZE).await?;
    info!(
        checkpoint,
        events = new_events.len(),
        "flow backfill starting (v2 event-id checkpoint)"
    );

    let mut summary = FlowSummary {
        events_seen: new_events.len() as u64,
        checkpoint_event_id: checkpoint,
        ..Default::default()
    };
    if new_events.is_empty() {
        // Tick updated_at so dashboards distinguish "caught up" from "stalled".
        advance_flow_checkpoint(pg, checkpoint.unwrap_or(0)).await?;
        return Ok(summary);
    }

    let affected = affected_delegators(&new_events);
    let max_event_id = new_events
        .iter()
        .map(|e| e.event_id)
        .max()
        .expect("non-empty");

    let history = fetch_full_history(pg, include_tentative, &affected).await?;
    let previously_registered = load_registered(pg, &affected).await?;

    // Group each delegator's events; TransferBond belongs to both sides.
    let mut per_delegator: HashMap<String, Vec<&StakeEvent>> = HashMap::new();
    for ev in &history {
        for d in delegators_of(ev) {
            per_delegator.entry(d).or_default().push(ev);
        }
    }

    for (delegator, events) in &per_delegator {
        let outcome = replay_delegator(delegator, events);
        summary.skipped_unregistered += outcome.skipped_unregistered;
        for row in &outcome.rows {
            upsert_stake_row(pg, delegator, row).await?;
            summary.stake_rows_written += 1;
        }
        if let Some(reg) = &outcome.registry {
            upsert_registry(pg, delegator, reg).await?;
            if !previously_registered.contains(delegator) {
                summary.delegators_registered += 1;
            }
        }
        summary.delegators_replayed += 1;
    }

    if summary.skipped_unregistered > 0 {
        warn!(
            skipped = summary.skipped_unregistered,
            "stake-flow events on delegators with no Bond seen in window — skipped (full-genesis backfill would not skip)"
        );
    }

    advance_flow_checkpoint(pg, max_event_id).await?;
    summary.checkpoint_event_id = Some(max_event_id);

    info!(?summary, "flow backfill complete");
    Ok(summary)
}

/// Pure replay of one delegator's complete event stream (already ordered by
/// `(block_number, log_index)`). Returns the per-block rows and the registry
/// state. Multiple events in one block collapse to a single row reflecting
/// the state after the last of them.
fn replay_delegator(delegator: &str, events: &[&StakeEvent]) -> ReplayOutcome {
    let zero = BigDecimal::from(0u64);
    let mut balance = zero.clone();
    let mut delegate: Option<String> = None;
    let mut registered = false;
    let mut first_bond: Option<(i64, i64)> = None;
    let mut last_seen: Option<(i64, i64)> = None;
    // block_number → row; later events in the same block overwrite.
    let mut rows: BTreeMap<i64, RowWrite> = BTreeMap::new();
    let mut skipped_unregistered = 0u64;

    let write_row = |rows: &mut BTreeMap<i64, RowWrite>,
                         ev: &StakeEvent,
                         delegate: &Option<String>,
                         balance: &BigDecimal,
                         source: &'static str| {
        rows.insert(
            ev.block_number,
            RowWrite {
                block_number: ev.block_number,
                block_timestamp: ev.block_timestamp,
                block_hash: ev.block_hash.clone(),
                delegate_address: delegate
                    .clone()
                    .unwrap_or_else(|| ZERO_ADDRESS.to_string()),
                bonded_principal: balance.clone(),
                source,
                triggering_event_id: ev.event_id,
            },
        );
    };

    for ev in events {
        let is_from = ev.from_address.as_deref() == Some(delegator);
        let is_to = ev.to_address.as_deref() == Some(delegator);

        match ev.event_name.as_str() {
            "Bond" if is_from => {
                if !registered {
                    registered = true;
                    first_bond.get_or_insert((ev.block_number, ev.event_id));
                }
                balance += ev.amount_normalized.clone().unwrap_or_else(|| zero.clone());
                if let Some(d) = ev.to_address.as_ref() {
                    delegate = Some(d.clone());
                }
                write_row(&mut rows, ev, &delegate, &balance, SOURCE_FLOW);
            }
            "Unbond" if is_from => {
                if !registered {
                    skipped_unregistered += 1;
                    continue;
                }
                balance -= ev.amount_normalized.clone().unwrap_or_else(|| zero.clone());
                if let Some(d) = ev.to_address.as_ref() {
                    delegate = Some(d.clone());
                }
                write_row(&mut rows, ev, &delegate, &balance, SOURCE_FLOW);
            }
            "WithdrawStake" if is_from => {
                if !registered {
                    skipped_unregistered += 1;
                    continue;
                }
                // Stake already left bonded_principal at the Unbond.
                write_row(&mut rows, ev, &delegate, &balance, SOURCE_FLOW);
            }
            "Rebond" if is_from => {
                if !registered {
                    skipped_unregistered += 1;
                    continue;
                }
                balance += ev.amount_normalized.clone().unwrap_or_else(|| zero.clone());
                if let Some(d) = ev.to_address.as_ref() {
                    delegate = Some(d.clone());
                }
                write_row(&mut rows, ev, &delegate, &balance, SOURCE_FLOW);
            }
            "EarningsClaimed" if is_from => {
                // Claimed rewards (LPT) compound into bonded principal.
                balance += earnings_rewards_lpt(&ev.raw_event);
                if let Some(d) = ev.to_address.as_ref() {
                    delegate = Some(d.clone());
                }
                // Pre-first-bond claims are observable on-chain and need a
                // base row so pending refresh has something to update later.
                // Keep them auditable via a distinct source; do not register
                // the delegator before the first real Bond arrives.
                let source = if registered {
                    SOURCE_FLOW
                } else {
                    SOURCE_EARNINGS_BOOTSTRAP
                };
                write_row(&mut rows, ev, &delegate, &balance, source);
            }
            "TransferBond" => {
                // Moves ownership of an unbonding lock; the underlying stake
                // already left bonded_principal at the preceding Unbond, so
                // neither side's balance changes here.
                if is_from && !is_to {
                    if !registered {
                        skipped_unregistered += 1;
                        continue;
                    }
                    write_row(&mut rows, ev, &delegate, &balance, SOURCE_FLOW);
                } else if is_to {
                    // Receiving a lock makes this address a delegator.
                    if !registered {
                        registered = true;
                        first_bond.get_or_insert((ev.block_number, ev.event_id));
                    }
                    write_row(&mut rows, ev, &delegate, &balance, SOURCE_FLOW);
                }
            }
            _ => continue,
        }
        last_seen = Some((ev.block_number, ev.event_id));
    }

    let registry = match (registered, first_bond, last_seen) {
        (true, Some((fb_block, fb_id)), Some((ls_block, ls_id))) => Some(RegistryWrite {
            first_bond_block: fb_block,
            first_bond_event_id: fb_id,
            last_seen_block: ls_block,
            last_seen_event_id: ls_id,
            is_active: balance > BigDecimal::from(0u64),
        }),
        _ => None,
    };

    ReplayOutcome {
        rows: rows.into_values().collect(),
        registry,
        skipped_unregistered,
    }
}

fn earnings_rewards_lpt(raw_event: &serde_json::Value) -> BigDecimal {
    raw_event
        .get("decoded")
        .and_then(|d| d.get("rewards"))
        .and_then(|v| v.as_str())
        .and_then(|s| BigDecimal::from_str(s).ok())
        .map(|wei| wei / BigDecimal::from(10u128.pow(18)))
        .unwrap_or_else(|| BigDecimal::from(0u64))
}

fn delegators_of(ev: &StakeEvent) -> Vec<String> {
    match ev.event_name.as_str() {
        "TransferBond" => {
            let mut v = Vec::with_capacity(2);
            if let Some(from) = ev.from_address.as_ref() {
                v.push(from.clone());
            }
            if let Some(to) = ev.to_address.as_ref() {
                if Some(to) != ev.from_address.as_ref() {
                    v.push(to.clone());
                }
            }
            v
        }
        _ => ev.from_address.iter().cloned().collect(),
    }
}

fn affected_delegators(events: &[StakeEvent]) -> Vec<String> {
    let mut set = HashSet::new();
    for ev in events {
        for d in delegators_of(ev) {
            set.insert(d);
        }
    }
    set.into_iter().collect()
}

const STAKE_EVENT_NAMES: &str =
    "'Bond', 'Unbond', 'Rebond', 'WithdrawStake', 'EarningsClaimed', 'TransferBond'";

/// New (not yet checkpointed) stake-flow events, in ingestion (id) order.
async fn fetch_new_event_heads(
    pg: &PgPool,
    include_tentative: bool,
    resume_from_event_id: Option<i64>,
    limit: i64,
) -> Result<Vec<StakeEvent>> {
    let finality_filter = if include_tentative {
        ""
    } else {
        "AND finality = 'finalized'"
    };
    let sql = format!(
        r#"SELECT id, event_name, block_number, block_timestamp, block_hash,
                  from_address, to_address, amount_normalized, raw_event
             FROM raw_protocol_events
            WHERE chain_id = $1
              AND is_canonical = TRUE
              AND event_name IN ({STAKE_EVENT_NAMES})
              AND ($2::bigint IS NULL OR id > $2)
              {finality_filter}
            ORDER BY id
            LIMIT $3"#
    );
    let rows = sqlx::query(&sql)
        .bind(ARBITRUM_CHAIN_ID)
        .bind(resume_from_event_id)
        .bind(limit)
        .fetch_all(pg)
        .await?;
    Ok(rows.into_iter().map(stake_event_from_row).collect())
}

/// The complete stake-flow event stream for the given delegators, in
/// `(block_number, log_index)` order — the replay input.
async fn fetch_full_history(
    pg: &PgPool,
    include_tentative: bool,
    delegators: &[String],
) -> Result<Vec<StakeEvent>> {
    if delegators.is_empty() {
        return Ok(Vec::new());
    }
    let finality_filter = if include_tentative {
        ""
    } else {
        "AND finality = 'finalized'"
    };
    let sql = format!(
        r#"SELECT id, event_name, block_number, block_timestamp, block_hash,
                  from_address, to_address, amount_normalized, raw_event
             FROM raw_protocol_events
            WHERE chain_id = $1
              AND is_canonical = TRUE
              AND event_name IN ({STAKE_EVENT_NAMES})
              AND (from_address = ANY($2) OR to_address = ANY($2))
              {finality_filter}
            ORDER BY block_number, log_index"#
    );
    let rows = sqlx::query(&sql)
        .bind(ARBITRUM_CHAIN_ID)
        .bind(delegators)
        .fetch_all(pg)
        .await?;
    Ok(rows.into_iter().map(stake_event_from_row).collect())
}

fn stake_event_from_row(r: sqlx::postgres::PgRow) -> StakeEvent {
    StakeEvent {
        event_id: r.get(0),
        event_name: r.get(1),
        block_number: r.get(2),
        block_timestamp: r.get(3),
        block_hash: r.get(4),
        from_address: r.try_get(5).ok(),
        to_address: r.try_get(6).ok(),
        amount_normalized: r.try_get(7).ok(),
        raw_event: r.try_get(8).unwrap_or(serde_json::Value::Null),
    }
}

async fn load_registered(pg: &PgPool, delegators: &[String]) -> Result<HashSet<String>> {
    if delegators.is_empty() {
        return Ok(HashSet::new());
    }
    let rows = sqlx::query(
        "SELECT delegator_address FROM delegator_registry
          WHERE chain_id = $1 AND delegator_address = ANY($2)",
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(delegators)
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(|r| r.get::<String, _>(0)).collect())
}

async fn load_flow_checkpoint(pg: &PgPool) -> Result<Option<i64>> {
    let checkpoint = sqlx::query_scalar::<_, i64>(
        "SELECT last_processed_block FROM indexer_checkpoints WHERE name = $1",
    )
    .bind(FLOW_CHECKPOINT)
    .fetch_optional(pg)
    .await?;
    Ok(checkpoint)
}

async fn advance_flow_checkpoint(pg: &PgPool, event_id: i64) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO indexer_checkpoints (name, chain_id, last_processed_block, updated_at)
           VALUES ($1, $2, $3, now())
           ON CONFLICT (name) DO UPDATE
              SET last_processed_block = GREATEST(indexer_checkpoints.last_processed_block, EXCLUDED.last_processed_block),
                  updated_at = now()"#,
    )
    .bind(FLOW_CHECKPOINT)
    .bind(ARBITRUM_CHAIN_ID)
    .bind(event_id)
    .execute(pg)
    .await?;
    Ok(())
}

async fn upsert_registry(pg: &PgPool, delegator: &str, reg: &RegistryWrite) -> Result<()> {
    // Replay covers the delegator's full known history, so every field —
    // including first_bond and is_active — is authoritative and idempotent.
    sqlx::query(
        r#"INSERT INTO delegator_registry
              (chain_id, delegator_address, first_bond_block, first_bond_event_id,
               last_seen_block, last_seen_event_id, is_active)
           VALUES ($1, $2, $3, $4, $5, $6, $7)
           ON CONFLICT (chain_id, delegator_address) DO UPDATE
              SET first_bond_block    = EXCLUDED.first_bond_block,
                  first_bond_event_id = EXCLUDED.first_bond_event_id,
                  last_seen_block     = EXCLUDED.last_seen_block,
                  last_seen_event_id  = EXCLUDED.last_seen_event_id,
                  is_active           = EXCLUDED.is_active"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(delegator)
    .bind(reg.first_bond_block)
    .bind(reg.first_bond_event_id)
    .bind(reg.last_seen_block)
    .bind(reg.last_seen_event_id)
    .bind(reg.is_active)
    .execute(pg)
    .await
    .with_context(|| format!("upserting registry for {delegator}"))?;
    Ok(())
}

async fn upsert_stake_row(pg: &PgPool, delegator: &str, row: &RowWrite) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO stake_balances_by_block
              (chain_id, delegator_address, delegate_address, block_number, block_timestamp,
               block_hash, bonded_principal, source, triggering_event_id)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
           ON CONFLICT (chain_id, delegator_address, block_number) DO UPDATE
              SET delegate_address    = EXCLUDED.delegate_address,
                  block_timestamp     = EXCLUDED.block_timestamp,
                  block_hash          = EXCLUDED.block_hash,
                  bonded_principal    = EXCLUDED.bonded_principal,
                  source              = EXCLUDED.source,
                  triggering_event_id = EXCLUDED.triggering_event_id"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(delegator)
    .bind(&row.delegate_address)
    .bind(row.block_number)
    .bind(row.block_timestamp)
    .bind(&row.block_hash)
    .bind(&row.bonded_principal)
    .bind(row.source)
    .bind(row.triggering_event_id)
    .execute(pg)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const ORCH_A: &str = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const ORCH_B: &str = "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const DELEGATOR: &str = "0xdddddddddddddddddddddddddddddddddddddddd";

    fn ev(
        event_id: i64,
        name: &str,
        block: i64,
        from: Option<&str>,
        to: Option<&str>,
        amount: Option<&str>,
    ) -> StakeEvent {
        StakeEvent {
            event_id,
            event_name: name.to_string(),
            block_number: block,
            block_timestamp: Utc.timestamp_opt(1_700_000_000 + block, 0).unwrap(),
            block_hash: format!("0xhash{block}"),
            from_address: from.map(str::to_string),
            to_address: to.map(str::to_string),
            amount_normalized: amount.map(|a| BigDecimal::from_str(a).unwrap()),
            raw_event: serde_json::Value::Null,
        }
    }

    fn claim(event_id: i64, block: i64, delegate: &str, rewards_wei: &str) -> StakeEvent {
        let mut e = ev(
            event_id,
            "EarningsClaimed",
            block,
            Some(DELEGATOR),
            Some(delegate),
            None,
        );
        e.raw_event = serde_json::json!({ "decoded": { "rewards": rewards_wei } });
        e
    }

    fn replay(events: &[StakeEvent]) -> ReplayOutcome {
        let refs: Vec<&StakeEvent> = events.iter().collect();
        replay_delegator(DELEGATOR, &refs)
    }

    #[test]
    fn bond_then_claim_compounds_into_principal() {
        let events = vec![
            ev(1, "Bond", 100, Some(DELEGATOR), Some(ORCH_A), Some("1000")),
            // 50 LPT in wei
            claim(2, 200, ORCH_A, "50000000000000000000"),
        ];
        let out = replay(&events);
        assert_eq!(out.rows.len(), 2);
        assert_eq!(out.rows[0].bonded_principal, BigDecimal::from(1000u64));
        assert_eq!(out.rows[1].bonded_principal, BigDecimal::from(1050u64));
        let reg = out.registry.expect("registered");
        assert_eq!(reg.first_bond_block, 100);
        assert!(reg.is_active);
    }

    #[test]
    fn move_to_new_delegate_updates_delegate_and_keeps_balance() {
        let events = vec![
            ev(1, "Bond", 100, Some(DELEGATOR), Some(ORCH_A), Some("1000")),
            // Move: bond with additionalAmount 0 toward a new delegate.
            ev(2, "Bond", 200, Some(DELEGATOR), Some(ORCH_B), Some("0")),
        ];
        let out = replay(&events);
        assert_eq!(out.rows.len(), 2);
        assert_eq!(out.rows[1].delegate_address, ORCH_B);
        assert_eq!(out.rows[1].bonded_principal, BigDecimal::from(1000u64));
        // The latest row carries the new delegate, so latest-row-overall
        // queries no longer list this delegator under ORCH_A.
        assert_eq!(out.rows.last().unwrap().delegate_address, ORCH_B);
    }

    #[test]
    fn full_unbond_deactivates_registry() {
        let events = vec![
            ev(1, "Bond", 100, Some(DELEGATOR), Some(ORCH_A), Some("1000")),
            ev(2, "Unbond", 200, Some(DELEGATOR), Some(ORCH_A), Some("1000")),
            ev(3, "WithdrawStake", 300, Some(DELEGATOR), None, Some("1000")),
        ];
        let out = replay(&events);
        assert_eq!(out.rows.len(), 3);
        assert_eq!(out.rows[1].bonded_principal, BigDecimal::from(0u64));
        assert_eq!(out.rows[2].bonded_principal, BigDecimal::from(0u64));
        let reg = out.registry.expect("registered");
        assert!(!reg.is_active, "fully unbonded delegator must be inactive");
        assert_eq!(reg.last_seen_block, 300);
    }

    #[test]
    fn partial_unbond_stays_active() {
        let events = vec![
            ev(1, "Bond", 100, Some(DELEGATOR), Some(ORCH_A), Some("1000")),
            ev(2, "Unbond", 200, Some(DELEGATOR), Some(ORCH_A), Some("400")),
        ];
        let out = replay(&events);
        assert_eq!(
            out.rows[1].bonded_principal,
            BigDecimal::from_str("600").unwrap()
        );
        assert!(out.registry.expect("registered").is_active);
    }

    #[test]
    fn unbond_then_rebond_restores_balance() {
        let events = vec![
            ev(1, "Bond", 100, Some(DELEGATOR), Some(ORCH_A), Some("1000")),
            ev(2, "Unbond", 200, Some(DELEGATOR), Some(ORCH_A), Some("1000")),
            ev(3, "Rebond", 300, Some(DELEGATOR), Some(ORCH_A), Some("1000")),
        ];
        let out = replay(&events);
        assert_eq!(out.rows[2].bonded_principal, BigDecimal::from(1000u64));
        assert!(out.registry.expect("registered").is_active);
    }

    #[test]
    fn pre_bond_events_are_skipped_with_counter() {
        let events = vec![
            ev(1, "Unbond", 100, Some(DELEGATOR), Some(ORCH_A), Some("500")),
            ev(2, "Bond", 200, Some(DELEGATOR), Some(ORCH_A), Some("1000")),
        ];
        let out = replay(&events);
        assert_eq!(out.skipped_unregistered, 1);
        assert_eq!(out.rows.len(), 1);
        assert_eq!(out.rows[0].bonded_principal, BigDecimal::from(1000u64));
    }

    #[test]
    fn pre_bond_claim_writes_bootstrap_row_without_registry() {
        let events = vec![claim(1, 100, ORCH_A, "50000000000000000000")];
        let out = replay(&events);
        assert_eq!(out.rows.len(), 1);
        assert_eq!(out.rows[0].source, SOURCE_EARNINGS_BOOTSTRAP);
        assert!(out.registry.is_none(), "claims alone must not register");
    }

    #[test]
    fn transfer_bond_registers_receiver_without_balance_change() {
        let other = "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
        let events = vec![ev(
            1,
            "TransferBond",
            100,
            Some(other),
            Some(DELEGATOR),
            Some("700"),
        )];
        let out = replay(&events);
        assert_eq!(out.rows.len(), 1);
        assert_eq!(out.rows[0].bonded_principal, BigDecimal::from(0u64));
        let reg = out.registry.expect("receiver becomes a delegator");
        assert!(!reg.is_active, "lock not rebonded yet");
    }

    #[test]
    fn same_block_events_collapse_to_final_state() {
        // bond() auto-claims first: EarningsClaimed then Bond in one block.
        let mut c = claim(2, 200, ORCH_A, "10000000000000000000");
        c.block_number = 200;
        let events = vec![
            ev(1, "Bond", 100, Some(DELEGATOR), Some(ORCH_A), Some("1000")),
            c,
            ev(3, "Bond", 200, Some(DELEGATOR), Some(ORCH_A), Some("90")),
        ];
        let out = replay(&events);
        assert_eq!(out.rows.len(), 2, "one row per block");
        assert_eq!(out.rows[1].bonded_principal, BigDecimal::from(1100u64));
        assert_eq!(out.rows[1].triggering_event_id, 3);
    }

    #[test]
    fn late_arriving_event_changes_replayed_history_deterministically() {
        // Replay with and without a "late" unbond — the rebuilt history is
        // exactly what a from-scratch replay would produce.
        let bond = ev(1, "Bond", 100, Some(DELEGATOR), Some(ORCH_A), Some("1000"));
        let late_unbond = ev(9, "Unbond", 150, Some(DELEGATOR), Some(ORCH_A), Some("1000"));
        let claim_after = claim(3, 200, ORCH_A, "0");

        let without = replay(&[bond.clone(), claim_after.clone()]);
        assert!(without.registry.unwrap().is_active);

        let with = replay(&[bond, late_unbond, claim_after]);
        assert_eq!(with.rows.len(), 3);
        assert_eq!(with.rows[1].bonded_principal, BigDecimal::from(0u64));
        assert!(!with.registry.unwrap().is_active);
    }
}
