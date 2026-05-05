//! Flow-derived `bonded_principal` per delegator + delegator_registry. SPEC §11.10, §11.11.
//!
//! Walks every stake-flow event from `raw_protocol_events` in `(block_number, log_index)`
//! order, maintains an in-memory per-delegator balance, and writes a
//! `stake_balances_by_block` row per affected delegator after each event.
//!
//! Per SPEC §11.11 the registry is derived from `Bond` events. Non-Bond events on
//! delegators we've never seen a Bond for are skipped with a warning — their pre-window
//! state is unknown. The full-genesis backfill would not have this issue.
//!
//! Idempotency: ON CONFLICT (chain_id, delegator_address, block_number) DO UPDATE.
//! Re-runs replay events in the same order → same balance evolution → same row writes.

use anyhow::{Context, Result};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgPool, Row};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use tracing::{info, warn};

const ARBITRUM_CHAIN_ID: i64 = 42161;
const SOURCE_FLOW: &str = "flow_derived";
const SOURCE_EARNINGS_BOOTSTRAP: &str = "earnings_claimed_bootstrap";
const FLOW_CHECKPOINT: &str = "staker_flow_backfill";
const FLOW_BATCH_SIZE: i64 = 5_000;

#[derive(Debug, Default, Serialize)]
pub struct FlowSummary {
    pub events_seen: u64,
    pub bond_events: u64,
    pub stake_rows_written: u64,
    pub delegators_registered: u64,
    pub skipped_unregistered: u64,
    pub checkpoint_block: Option<i64>,
}

#[derive(Debug)]
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

#[derive(Debug)]
struct StakeSeedState {
    delegate_address: String,
    bonded_principal: BigDecimal,
}

pub async fn run_flow_backfill(pg: &PgPool, include_tentative: bool) -> Result<FlowSummary> {
    let checkpoint = load_flow_checkpoint(pg).await?;
    let events = fetch_stake_events(pg, include_tentative, checkpoint, FLOW_BATCH_SIZE).await?;
    info!(checkpoint, events = events.len(), "flow backfill starting");

    let mut registered: HashSet<String> = load_registered_delegators(pg).await?;
    let seeds = load_latest_stake_state_before_block(pg, checkpoint, &events).await?;
    let mut balances: HashMap<String, BigDecimal> = HashMap::with_capacity(seeds.len());
    let mut delegates: HashMap<String, String> = HashMap::with_capacity(seeds.len());
    for (delegator, state) in seeds {
        balances.insert(delegator.clone(), state.bonded_principal);
        delegates.insert(delegator, state.delegate_address);
    }
    let zero = BigDecimal::from(0u64);

    let mut summary = FlowSummary {
        events_seen: events.len() as u64,
        checkpoint_block: checkpoint,
        ..Default::default()
    };
    let mut max_block_seen = checkpoint;

    for ev in &events {
        match ev.event_name.as_str() {
            "Bond" => {
                let delegator = match ev.from_address.as_ref() {
                    Some(a) => a.to_lowercase(),
                    None => continue,
                };
                let delegate = match ev.to_address.as_ref() {
                    Some(a) => a.to_lowercase(),
                    None => continue,
                };
                let amt = ev.amount_normalized.clone().unwrap_or_else(|| zero.clone());
                let new_balance = balances
                    .entry(delegator.clone())
                    .or_insert_with(|| zero.clone());
                *new_balance += &amt;
                delegates.insert(delegator.clone(), delegate.clone());
                summary.bond_events += 1;

                if registered.insert(delegator.clone()) {
                    summary.delegators_registered += 1;
                    upsert_registry(pg, &delegator, ev.block_number, ev.event_id, true).await?;
                } else {
                    upsert_registry(pg, &delegator, ev.block_number, ev.event_id, false).await?;
                }
                upsert_stake_row(pg, &delegator, &delegate, ev, &balances[&delegator]).await?;
                summary.stake_rows_written += 1;
            }
            "Unbond" | "WithdrawStake" => {
                let delegator = match ev.from_address.as_ref() {
                    Some(a) => a.to_lowercase(),
                    None => continue,
                };
                if !registered.contains(&delegator) {
                    summary.skipped_unregistered += 1;
                    continue;
                }
                if ev.event_name == "Unbond" {
                    let amt = ev.amount_normalized.clone().unwrap_or_else(|| zero.clone());
                    *balances
                        .entry(delegator.clone())
                        .or_insert_with(|| zero.clone()) -= &amt;
                }
                let delegate = ev
                    .to_address
                    .clone()
                    .map(|s| s.to_lowercase())
                    .or_else(|| delegates.get(&delegator).cloned())
                    .unwrap_or_else(|| "0x0000000000000000000000000000000000000000".to_string());
                upsert_registry(pg, &delegator, ev.block_number, ev.event_id, false).await?;
                upsert_stake_row(pg, &delegator, &delegate, ev, &balances[&delegator]).await?;
                summary.stake_rows_written += 1;
            }
            "Rebond" => {
                let delegator = match ev.from_address.as_ref() {
                    Some(a) => a.to_lowercase(),
                    None => continue,
                };
                if !registered.contains(&delegator) {
                    summary.skipped_unregistered += 1;
                    continue;
                }
                let amt = ev.amount_normalized.clone().unwrap_or_else(|| zero.clone());
                *balances
                    .entry(delegator.clone())
                    .or_insert_with(|| zero.clone()) += &amt;
                let delegate = ev
                    .to_address
                    .clone()
                    .map(|s| s.to_lowercase())
                    .or_else(|| delegates.get(&delegator).cloned())
                    .unwrap_or_default();
                if !delegate.is_empty() {
                    delegates.insert(delegator.clone(), delegate.clone());
                }
                upsert_registry(pg, &delegator, ev.block_number, ev.event_id, false).await?;
                upsert_stake_row(pg, &delegator, &delegate, ev, &balances[&delegator]).await?;
                summary.stake_rows_written += 1;
            }
            "EarningsClaimed" => {
                // rewards (LPT) compounds into delegator's bonded principal.
                let delegator = match ev.from_address.as_ref() {
                    Some(a) => a.to_lowercase(),
                    None => continue,
                };
                let rewards_wei: Option<&str> = ev
                    .raw_event
                    .get("decoded")
                    .and_then(|d| d.get("rewards"))
                    .and_then(|v| v.as_str());
                let rewards_lpt = rewards_wei
                    .and_then(|s| BigDecimal::from_str(s).ok())
                    .map(|big| big / BigDecimal::from(10u128.pow(18)))
                    .unwrap_or_else(|| zero.clone());
                *balances
                    .entry(delegator.clone())
                    .or_insert_with(|| zero.clone()) += &rewards_lpt;
                let delegate = ev
                    .to_address
                    .clone()
                    .map(|s| s.to_lowercase())
                    .or_else(|| delegates.get(&delegator).cloned())
                    .unwrap_or_else(|| "0x0000000000000000000000000000000000000000".to_string());
                if registered.contains(&delegator) {
                    upsert_registry(pg, &delegator, ev.block_number, ev.event_id, false).await?;
                    upsert_stake_row(pg, &delegator, &delegate, ev, &balances[&delegator]).await?;
                } else {
                    // Pre-first-bond claims are observable on-chain and need a base
                    // row so pending refresh has something to update later. Keep the
                    // row auditable via a distinct source and do not fabricate a
                    // delegator_registry entry before the first real Bond arrives.
                    upsert_stake_row_source(
                        pg,
                        &delegator,
                        &delegate,
                        ev,
                        &balances[&delegator],
                        SOURCE_EARNINGS_BOOTSTRAP,
                    )
                    .await?;
                }
                summary.stake_rows_written += 1;
            }
            "TransferBond" => {
                // TransferBond moves ownership of an unbonding lock from the old
                // delegator to the new delegator. The underlying stake was already
                // removed from bonded principal by the preceding Unbond event, so
                // this event should not change bonded_principal for either side.
                //
                // We still keep registry/stake rows at this block so callers can see
                // the touched delegators, but the balance is unchanged.
                let old_d = match ev.from_address.as_ref() {
                    Some(a) => a.to_lowercase(),
                    None => continue,
                };
                let new_d = match ev.to_address.as_ref() {
                    Some(a) => a.to_lowercase(),
                    None => continue,
                };
                if registered.contains(&old_d) {
                    let delegate = delegates.get(&old_d).cloned().unwrap_or_default();
                    upsert_registry(pg, &old_d, ev.block_number, ev.event_id, false).await?;
                    upsert_stake_row(pg, &old_d, &delegate, ev, &balances[&old_d]).await?;
                    summary.stake_rows_written += 1;
                } else {
                    summary.skipped_unregistered += 1;
                }
                // The receiving side becomes a delegator on receipt — register them.
                if registered.insert(new_d.clone()) {
                    summary.delegators_registered += 1;
                }
                let delegate_for_new = delegates
                    .get(&new_d)
                    .cloned()
                    .or_else(|| delegates.get(&old_d).cloned())
                    .unwrap_or_default();
                upsert_registry(pg, &new_d, ev.block_number, ev.event_id, true).await?;
                let new_balance = balances
                    .entry(new_d.clone())
                    .or_insert_with(|| zero.clone())
                    .clone();
                upsert_stake_row(pg, &new_d, &delegate_for_new, ev, &new_balance).await?;
                summary.stake_rows_written += 1;
            }
            _ => {}
        }
        max_block_seen = Some(ev.block_number);
    }

    if summary.skipped_unregistered > 0 {
        warn!(
            skipped = summary.skipped_unregistered,
            "stake-flow events on delegators with no Bond seen in window — skipped (full-genesis backfill would not skip)"
        );
    }

    if let Some(block_number) = max_block_seen {
        advance_flow_checkpoint(pg, block_number).await?;
        summary.checkpoint_block = Some(block_number);
    }

    info!(?summary, "flow backfill complete");
    Ok(summary)
}

async fn fetch_stake_events(
    pg: &PgPool,
    include_tentative: bool,
    resume_from_block: Option<i64>,
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
              AND event_name IN ('Bond', 'Unbond', 'Rebond', 'WithdrawStake',
                                 'EarningsClaimed', 'TransferBond')
              AND ($2::bigint IS NULL OR block_number >= $2)
              {finality_filter}
            ORDER BY block_number, log_index
            LIMIT $3"#
    );
    let rows = sqlx::query(&sql)
        .bind(ARBITRUM_CHAIN_ID)
        .bind(resume_from_block)
        .bind(limit)
        .fetch_all(pg)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| StakeEvent {
            event_id: r.get(0),
            event_name: r.get(1),
            block_number: r.get(2),
            block_timestamp: r.get(3),
            block_hash: r.get(4),
            from_address: r.try_get(5).ok(),
            to_address: r.try_get(6).ok(),
            amount_normalized: r.try_get(7).ok(),
            raw_event: r.try_get(8).unwrap_or(serde_json::Value::Null),
        })
        .collect())
}

async fn load_registered_delegators(pg: &PgPool) -> Result<HashSet<String>> {
    let rows = sqlx::query("SELECT delegator_address FROM delegator_registry WHERE chain_id = $1")
        .bind(ARBITRUM_CHAIN_ID)
        .fetch_all(pg)
        .await?;
    Ok(rows.into_iter().map(|r| r.get::<String, _>(0)).collect())
}

async fn load_latest_stake_state_before_block(
    pg: &PgPool,
    resume_from_block: Option<i64>,
    events: &[StakeEvent],
) -> Result<HashMap<String, StakeSeedState>> {
    let Some(resume_from_block) = resume_from_block else {
        return Ok(HashMap::new());
    };
    let delegators = affected_delegators(events);
    if delegators.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"SELECT DISTINCT ON (delegator_address)
               delegator_address, delegate_address, bonded_principal
             FROM stake_balances_by_block
            WHERE chain_id = $1
              AND delegator_address = ANY($2)
              AND block_number < $3
            ORDER BY delegator_address, block_number DESC"#,
    )
    .bind(ARBITRUM_CHAIN_ID)
    .bind(&delegators)
    .bind(resume_from_block)
    .fetch_all(pg)
    .await?;

    let mut states = HashMap::with_capacity(rows.len());
    for row in rows {
        states.insert(
            row.get::<String, _>("delegator_address"),
            StakeSeedState {
                delegate_address: row.get("delegate_address"),
                bonded_principal: row.get("bonded_principal"),
            },
        );
    }
    Ok(states)
}

fn affected_delegators(events: &[StakeEvent]) -> Vec<String> {
    let mut set = HashSet::new();
    for ev in events {
        match ev.event_name.as_str() {
            "TransferBond" => {
                if let Some(from) = ev.from_address.as_ref() {
                    set.insert(from.to_lowercase());
                }
                if let Some(to) = ev.to_address.as_ref() {
                    set.insert(to.to_lowercase());
                }
            }
            "Bond" | "Unbond" | "WithdrawStake" | "Rebond" | "EarningsClaimed" => {
                if let Some(from) = ev.from_address.as_ref() {
                    set.insert(from.to_lowercase());
                }
            }
            _ => {}
        }
    }
    set.into_iter().collect()
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

async fn advance_flow_checkpoint(pg: &PgPool, block_number: i64) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO indexer_checkpoints (name, chain_id, last_processed_block, updated_at)
           VALUES ($1, $2, $3, now())
           ON CONFLICT (name) DO UPDATE
              SET last_processed_block = GREATEST(indexer_checkpoints.last_processed_block, EXCLUDED.last_processed_block),
                  updated_at = now()"#,
    )
    .bind(FLOW_CHECKPOINT)
    .bind(ARBITRUM_CHAIN_ID)
    .bind(block_number)
    .execute(pg)
    .await?;
    Ok(())
}

async fn upsert_registry(
    pg: &PgPool,
    delegator: &str,
    block_number: i64,
    event_id: i64,
    is_first: bool,
) -> Result<()> {
    if is_first {
        sqlx::query(
            r#"INSERT INTO delegator_registry
                  (chain_id, delegator_address, first_bond_block, first_bond_event_id,
                   last_seen_block, last_seen_event_id, is_active)
               VALUES ($1, $2, $3, $4, $3, $4, TRUE)
               ON CONFLICT (chain_id, delegator_address) DO UPDATE
                  SET last_seen_block    = GREATEST(delegator_registry.last_seen_block, EXCLUDED.last_seen_block),
                      last_seen_event_id = CASE
                          WHEN EXCLUDED.last_seen_block > delegator_registry.last_seen_block
                          THEN EXCLUDED.last_seen_event_id
                          ELSE delegator_registry.last_seen_event_id
                      END"#,
        )
        .bind(ARBITRUM_CHAIN_ID)
        .bind(delegator)
        .bind(block_number)
        .bind(event_id)
        .execute(pg)
        .await
        .with_context(|| format!("registering delegator {delegator}"))?;
    } else {
        sqlx::query(
            r#"UPDATE delegator_registry
                  SET last_seen_block    = GREATEST(last_seen_block, $3),
                      last_seen_event_id = CASE
                          WHEN $3 > last_seen_block THEN $4
                          ELSE last_seen_event_id
                      END
                WHERE chain_id = $1 AND delegator_address = $2"#,
        )
        .bind(ARBITRUM_CHAIN_ID)
        .bind(delegator)
        .bind(block_number)
        .bind(event_id)
        .execute(pg)
        .await?;
    }
    Ok(())
}

async fn upsert_stake_row(
    pg: &PgPool,
    delegator: &str,
    delegate: &str,
    ev: &StakeEvent,
    balance: &BigDecimal,
) -> Result<()> {
    upsert_stake_row_source(pg, delegator, delegate, ev, balance, SOURCE_FLOW).await
}

async fn upsert_stake_row_source(
    pg: &PgPool,
    delegator: &str,
    delegate: &str,
    ev: &StakeEvent,
    balance: &BigDecimal,
    source: &str,
) -> Result<()> {
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
    .bind(delegate)
    .bind(ev.block_number)
    .bind(ev.block_timestamp)
    .bind(&ev.block_hash)
    .bind(balance)
    .bind(source)
    .bind(ev.event_id)
    .execute(pg)
    .await?;
    Ok(())
}
