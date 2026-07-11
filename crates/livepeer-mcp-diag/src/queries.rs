//! Centralized read-only SQL. All statements are `SELECT`s. Identifiers used
//! in string-built queries (rollup table names) come only from the fixed
//! allowlist below — never from tool input.

/// The five per-contract indexer checkpoints the daemon advances. Lag is
/// measured against `MIN(last_processed_block)` of these (see
/// `livepeer-daemon/src/supervisor.rs::current_indexer_lag`).
pub const INDEXER_CHECKPOINT_NAMES: [&str; 5] = [
    "indexer_BondingManager",
    "indexer_TicketBroker",
    "indexer_LivepeerToken",
    "indexer_RoundsManager",
    "indexer_Governor",
];

/// Daily rollups: (checkpoint name in `indexer_checkpoints`, rollup table).
/// The checkpoint's `last_processed_block` column holds the max source
/// `event_id` folded into that rollup; `updated_at` is the liveness heartbeat.
pub const ROLLUPS: [(&str, &str); 4] = [
    ("rollup_orch_payouts_daily", "orch_payouts_daily"),
    ("rollup_orch_rewards_daily", "orch_rewards_daily"),
    ("rollup_tickets_daily", "tickets_daily"),
    ("rollup_event_metrics_daily", "event_metrics_daily"),
];

/// All checkpoints with age in seconds. Tools filter by name.
pub const CHECKPOINTS: &str = r#"
SELECT name,
       last_processed_block,
       updated_at,
       EXTRACT(EPOCH FROM (now() - updated_at))::bigint AS age_secs
FROM indexer_checkpoints
ORDER BY name
"#;

/// Finality frontier — the finality watcher stamps rows directly and keeps no
/// block cursor, so the frontier lives in `raw_protocol_events`.
pub const FINALITY_FRONTIER: &str = r#"
SELECT MAX(block_number)                                              AS frontier_block,
       MAX(finalized_at)                                             AS finalized_at,
       EXTRACT(EPOCH FROM (now() - MAX(finalized_at)))::bigint       AS age_secs
FROM raw_protocol_events
WHERE finality = 'finalized' AND is_canonical
"#;

/// The degraded-spot sibling version the valuator writes for early events from
/// before the Uniswap pool had enough observation cardinality for a 30-min
/// TWAP. Hardcoded in the valuator as `format!("v1{DEGRADED_VERSION_SUFFIX}")`
/// (livepeer-valuator/src/onchain.rs:53). An event priced under EITHER version
/// is "done" — the backlog must not count degraded-priced events as unpriced.
pub const DEGRADED_VALUATION_VERSION: &str = "v1_degraded_spot_pre_cardinality";

/// Not-yet-priced backlog, mirroring the valuator's real candidate predicate
/// (onchain.rs:1372-1379): a valuable/canonical/finalized event is still work
/// only if it has NO `event_valuations` row under the main OR degraded version
/// AND no terminal (`failed_%`) attempt under either. Counting only the main
/// version — or keying on `amount_usd IS NULL` — massively over-counts, because
/// early history is legitimately priced under the degraded version.
/// $1 = main version, $2 = degraded version.
pub const PRICING_BACKLOG: &str = r#"
SELECT count(*)                                                       AS backlog,
       MIN(e.block_timestamp)                                        AS oldest_ts,
       EXTRACT(EPOCH FROM (now() - MIN(e.block_timestamp)))::bigint  AS oldest_age_secs
FROM raw_protocol_events e
WHERE e.is_valuable AND e.is_canonical AND e.finality = 'finalized'
  AND NOT EXISTS (
        SELECT 1 FROM event_valuations v
        WHERE v.event_id = e.id AND v.valuation_version IN ($1, $2)
  )
  AND NOT EXISTS (
        SELECT 1 FROM valuation_attempts a
        WHERE a.event_id = e.id AND a.valuation_version IN ($1, $2)
          AND a.result_status LIKE 'failed_%'
  )
"#;

/// Terminal-failure breakdown by status for the active version. $1 = version.
pub const TERMINAL_FAILURES: &str = r#"
SELECT status, count(*) AS n
FROM event_valuations
WHERE valuation_version = $1 AND status LIKE 'failed_%'
GROUP BY status
ORDER BY n DESC
"#;

/// Pricing retries that are due (backpressure / stuck transient failures).
pub const PENDING_RETRIES: &str = r#"
SELECT count(*) AS n
FROM valuation_attempts
WHERE next_retry_at IS NOT NULL AND next_retry_at <= now()
"#;

/// Highest event_id priced with a usable USD value under the main OR degraded
/// version — the frontier a rollup can advance to. $1 = main, $2 = degraded.
pub const MAX_PRICED_EVENT_ID: &str = r#"
SELECT MAX(v.event_id) AS id
FROM event_valuations v
WHERE v.valuation_version IN ($1, $2) AND v.amount_usd IS NOT NULL
"#;

/// Latest materialized day across all four rollups, in one round trip. Uses
/// only the fixed table allowlist in `ROLLUPS`.
pub const ROLLUP_MAX_DAYS: &str = r#"
SELECT 'orch_payouts_daily'  AS rollup, MAX(day_utc) AS max_day FROM orch_payouts_daily
UNION ALL
SELECT 'orch_rewards_daily',  MAX(day_utc) FROM orch_rewards_daily
UNION ALL
SELECT 'tickets_daily',       MAX(day_utc) FROM tickets_daily
UNION ALL
SELECT 'event_metrics_daily', MAX(day_utc) FROM event_metrics_daily
"#;

/// Grouped error counts for `recent_errors` — counts, never row dumps.
pub const ERROR_COUNTS: &str = r#"
SELECT 'decode_failures_unresolved' AS kind, count(*) AS n
  FROM decode_failures WHERE resolved_at IS NULL
UNION ALL
SELECT 'reorg_events_total', count(*) FROM reorg_events
UNION ALL
SELECT 'rpc_divergence_failures_total', count(*) FROM rpc_divergence_failures
UNION ALL
SELECT 'valuation_attempts_failed', count(*)
  FROM valuation_attempts WHERE result_status LIKE 'failed_%'
"#;

/// Most recent unresolved decode failures, capped. Returns a compact shape.
/// (decode_failures stores block/tx/topics, not decoded names — the point is
/// the dead-letter volume and recency, not per-event semantics.)
pub const RECENT_DECODE_FAILURES: &str = r#"
SELECT block_number, tx_hash, topics, created_at
FROM decode_failures
WHERE resolved_at IS NULL
ORDER BY created_at DESC
LIMIT 20
"#;
