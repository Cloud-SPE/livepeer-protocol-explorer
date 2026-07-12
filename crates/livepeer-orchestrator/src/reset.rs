//! Truncate primitives for the deterministic-replay path.
//!
//! Two functions: one for `bootstrap` (which currently does NOT call this
//! — see RUNBOOK §recovery), one for `replay` (which always does).
//!
//! Tables intentionally **never** truncated by anything in this module:
//!   - `rpc_call_cache` — the determinism source of truth (SPEC §11.12)
//!   - `seeded_event_prices` — imported from SQLite once, irreplaceable
//!   - `replay_finality_latest_l1_ts` / `replay_finality_finalized_l1_ts`
//!     checkpoint rows — recorded finality inputs that drive replay
//!
//! Tables NEVER referenced as TRUNCATE targets even though they're listed
//! in `docs/DETERMINISM.md` as replay-reconstructed:
//!   - `orchestrator_profile`, `broadcaster_profile` — these are
//!     materialized views since TD-025 / TD-026 (migrations 042 / 044).
//!     Postgres errors with "is not a table" when you try to TRUNCATE a
//!     matview. The matviews are derived from `orch_stake_by_round` and
//!     `gateway_balances_by_block` (which DO get truncated here); use
//!     `refresh_derived_matviews` after replay to repopulate them.

use anyhow::Result;
use sqlx::PgPool;

/// Tables truncated by both bootstrap and full replay.
///
/// Excludes raw_protocol_events + decode_failures (handled separately
/// based on whether raw events are being replayed) and the matviews
/// (`orchestrator_profile`, `broadcaster_profile` — see module doc).
const REBUILDABLE_DERIVED_TABLES: &[&str] = &[
    // Valuator output
    "event_valuations",
    "valuation_attempts",
    "token_prices_by_block",
    // Valuator incremental-scan cursors (migration 047). MUST be reset on
    // replay: it's a per-pass finalized_at high-water mark. A stale watermark
    // would make the ETH/LPT/MULTI passes scan only the recent tail and skip
    // rebuilding historical valuations. The seed pass runs first and repopulates
    // event_valuations, so scan_floor's version-wide cold-start guard can't
    // catch this on its own — the cursor row itself has to go.
    "valuator_cursors",
    // Staker output (per-delegator)
    "stake_balances_by_block",
    "delegator_registry",
    // Gateway worker output
    "gateway_balances_by_block",
    "gateway_flows",
    "gateway_claimants_by_block",
    // Profile worker output (TD-026: orch_stake_by_round; broadcaster
    // and orchestrator profile MATVIEWS are NOT truncated — they're
    // refreshed via REFRESH MATERIALIZED VIEW from the source tables.)
    "orch_stake_by_round",
    // Receipts archive (TD-020). Replay-reconstructed via cached
    // eth_getTransactionReceipt responses in rpc_call_cache.
    "tx_receipts",
    // Daily rollups
    "orch_payouts_daily",
    "orch_rewards_daily",
    "tickets_daily",
    // Event metrics rollup (TD-018)
    "event_metrics_daily",
    // Reorg + divergence audit logs
    "reorg_events",
    "reorg_mutations",
    "rpc_divergence_failures",
];

/// Truncate every replay-rebuildable table plus raw_protocol_events and
/// indexer_checkpoints. Intentionally NOT wired into `bootstrap::run` —
/// `bootstrap` assumes a clean DB and is idempotent on top of existing
/// state. Operators who genuinely want a from-scratch rebuild should
/// invoke this explicitly (e.g. via a one-shot binary or `psql`-driven
/// procedure) — see RUNBOOK §recovery for the supported recovery paths.
///
/// Marked `dead_code` because we keep the function for the `replay`
/// path's sister-truncate pattern and for operator scripts; its absence
/// from `bootstrap::run` is the documented design.
#[allow(dead_code)]
pub async fn truncate_for_bootstrap(pg: &PgPool) -> Result<()> {
    let mut tables: Vec<&str> = vec!["raw_protocol_events", "decode_failures"];
    tables.extend(REBUILDABLE_DERIVED_TABLES);
    tables.push("indexer_checkpoints");
    let sql = format!(
        "TRUNCATE TABLE {} RESTART IDENTITY CASCADE",
        tables.join(", ")
    );
    sqlx::query(&sql).execute(pg).await?;
    Ok(())
}

pub async fn truncate_for_replay(pg: &PgPool, keep_raw_events: bool) -> Result<()> {
    let mut tables: Vec<&str> = Vec::new();
    if !keep_raw_events {
        tables.push("raw_protocol_events");
        tables.push("decode_failures");
    }
    tables.extend(REBUILDABLE_DERIVED_TABLES);
    let sql = format!(
        "TRUNCATE TABLE {} RESTART IDENTITY CASCADE",
        tables.join(", ")
    );
    sqlx::query(&sql).execute(pg).await?;
    if !keep_raw_events {
        sqlx::query(
            r#"DELETE FROM indexer_checkpoints
                 WHERE name NOT IN (
                   'replay_finality_latest_l1_ts',
                   'replay_finality_finalized_l1_ts'
                 )"#,
        )
        .execute(pg)
        .await?;
    }
    Ok(())
}

/// Refresh `orchestrator_profile` and `broadcaster_profile` materialized
/// views. Called after replay so the matviews reflect the rebuilt source
/// tables. In live mode the daemon's `matview_refresh_loop` (TD-025)
/// handles this every 30 s; replay has no daemon, so we refresh
/// explicitly.
///
/// CONCURRENTLY isn't strictly required here (replay is single-threaded
/// and there are no readers) but keeping the same shape as the live
/// refresh path means the unique-index requirement (which we already
/// have) keeps working uniformly.
pub async fn refresh_derived_matviews(pg: &PgPool) -> Result<()> {
    sqlx::query("REFRESH MATERIALIZED VIEW broadcaster_profile")
        .execute(pg)
        .await?;
    sqlx::query("REFRESH MATERIALIZED VIEW orchestrator_profile")
        .execute(pg)
        .await?;
    Ok(())
}
