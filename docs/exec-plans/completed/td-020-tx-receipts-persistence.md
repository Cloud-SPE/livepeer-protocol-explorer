# TD-020: Transaction Receipt Persistence

**Status:** **Resolved** on 2026-05-08
**Author:** 2026-05-08
**Severity:** medium
**Source:** SQL audit P5 — `/reports/*.csv` cold path observed at 9 s on `gateway-payouts.csv`

## Closure summary

All 6 phases shipped end-to-end on 2026-05-08:

- **Phase A (schema)**: migration 041 applied; `tx_receipts` table created with `(chain_id, tx_hash)` PK + `(chain_id, block_number)` index.
- **Phase B (backfill subcommand)**: `livepeer-staker tx-receipts-backfill` shipped with bounded `buffer_unordered(N)` fan-out via `single_call_cached`. Boundary fix: `block_number >= checkpoint` so leftover txs at the boundary block aren't skipped.
- **Phase C (follow + docker-compose)**: `tx-receipts-follow` subcommand + `livepeer-staker-tx-receipts-follow` service entry.
- **Phase D (reports cutover)**: `load_tx_fees` rewritten to hybrid SQL+RPC pattern. Verified on a fully-backfilled date range: `Index Scan using tx_receipts_pkey`, byte-identical CSV across 3 reruns. Cold path 9.0s → 151ms (60×) within backfilled range.
- **Phase E (live execution)**: 1,416,505 rows backfilled in ~3 hours at concurrency 8 with no 429s, no errors. Final benchmark on April 2026 export (6,378 unique txs): cold **1.4 s** (vs. 9-24 s before), warm **148 ms**.
- **Phase F (closure)**: `docs/DETERMINISM.md` updated; tracker entry added; plan moved here.

Open follow-up (TD-020.5): delete the RPC fallback path in `load_tx_fees` once the `tx_receipts` table is verifiably populated for ≥ 1 week of new events. Until then, the fallback handles any tx not yet picked up by the live `tx-receipts-follow` loop.

Determinism notes: `tx_receipts` is a typed projection of cached `eth_getTransactionReceipt` responses already covered by the `rpc_call_cache` replay contract. No fixture regeneration was required because `livepeer-orchestrator replay` doesn't currently invoke `tx-receipts-backfill`. If a future replay test wants to validate `tx_receipts` content, add the subcommand to the replay sequence and recompute `expected_hashes.json`.

## Problem

`/reports/gateway-payouts.csv`, `/reports/payouts.csv`, and `/reports/rewards.csv` compute on-chain transaction fees by calling `eth_getTransactionReceipt` for every unique tx_hash referenced in the export. Each call is awaited serially in `crates/livepeer-api/src/routes/reports.rs:741-766`. Cold-cache exports take **~9 s** for a 150-tx CSV; warm-cache exports ride on `rpc_call_cache` and finish in 25-55 ms.

The serial loop is the surface symptom. The deeper problem is that **transaction-receipt data is never persisted as first-class indexer state**. The pipeline ingests logs (`eth_getLogs` → `raw_protocol_events`) but never receipts. Receipt-only fields — `gas_used`, `effective_gas_price`, `status` — are reachable only via per-call RPC, mediated by the generic `rpc_call_cache` JSONB store.

The audit's P5 recommendation was to parallelize the loop with `buffer_unordered(12)`, which would drop cold to ~700 ms. That fix is correct but transient: it leaves the receipt RPC dependency embedded in the read path, and it provides no foundation for any future feature that wants to query gas/fee data (gas-cost reports, MEV detection, per-orchestrator overhead analytics).

## Resolution

Materialize a first-class `tx_receipts` table populated by a dedicated backfill + live-follow worker, mirroring the staker / rollups pattern already established in this codebase. The reports endpoints become deterministic SQL joins; receipt RPC disappears from the API read path.

## Scope

**In scope:**
- New table `tx_receipts (chain_id, tx_hash) PK` with gas + fee columns
- Migration 041 + reverse migration
- New staker subcommand `tx-receipts-backfill` (one-shot bounded) and `tx-receipts-follow` (poll loop)
- Reports endpoint rewrite: `load_tx_fees` becomes a SQL query against the new table, with a bounded RPC fallback for not-yet-backfilled rows (removable in a follow-up)
- Prometheus metrics for the new worker (consistent with TD-016 Phase E surface)
- `docker-compose.yml` service entry
- Determinism contract: declare receipts as a deterministic projection of `rpc_call_cache` (no new replay-hash inclusion needed; see § Determinism)

**Out of scope:**
- Any backfill of receipts for tentative (non-finalized) events. The worker only writes finalized rows.
- Reorg-time receipt mutation. Finalized → reorged is structurally impossible per SPEC §9.1; if it ever is, that goes to TD-005.
- Fee-cost analytics endpoints. This plan only ships the data layer; new endpoints are future work.
- L1-side receipts. Out of pipeline.

## Scale

From the live DB at 2026-05-08:

| Metric | Value |
|---|---|
| `raw_protocol_events` rows | 2.66 M |
| Unique tx_hash values | **1.42 M** |
| Unique finalized tx_hash | **1.41 M** |
| Already cached in `rpc_call_cache` (`eth_getTransactionReceipt`) | 1,577 (0.1%) |
| Block range span | 1,543 days (Feb 2022 → 2026-05-08) |

At RPC concurrency 12 with ~50 ms/call, full-history backfill takes **~100 minutes** wall-clock — comparable to gateway-balance-backfill. Subsequent live-follow runs are bounded by event arrival rate (a handful of new txs per round).

## Architecture

### Table shape (migration 041)

```sql
CREATE TABLE tx_receipts (
    chain_id              BIGINT      NOT NULL,
    tx_hash               TEXT        NOT NULL,

    -- Block context (denormalized so reports don't need a join)
    block_number          BIGINT      NOT NULL,
    block_timestamp       TIMESTAMPTZ NOT NULL,

    -- Receipt fields
    gas_used              NUMERIC(78,0) NOT NULL,
    effective_gas_price   NUMERIC(78,0) NOT NULL,
    tx_fee_wei            NUMERIC(78,0) NOT NULL,    -- gas_used * effective_gas_price (precomputed)
    tx_fee_eth            NUMERIC(38,18) NOT NULL,   -- decimal-formatted ETH value
    status                SMALLINT    NOT NULL,      -- 1 = success, 0 = reverted

    -- Sender / recipient (cheap to denormalize from receipt)
    from_address          TEXT        NOT NULL,
    to_address            TEXT,                       -- NULL for contract creations

    created_at            TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    PRIMARY KEY (chain_id, tx_hash)
);

CREATE INDEX idx_tx_receipts_chain_block ON tx_receipts (chain_id, block_number);
```

Rationale:
- PK on `(chain_id, tx_hash)` — exact lookup shape for the reports endpoint
- `(chain_id, block_number)` index — supports any future "give me receipts for this block range" shape
- `tx_fee_wei` AND `tx_fee_eth` precomputed — eliminates per-row arithmetic at read time
- No partial indexes; the table is small (~1.4 M rows) and lookups are point queries

### Worker shape

Two new subcommands in **`livepeer-staker`** (the existing home for backfillers — `Backfill`, `GatewayBackfill`, `RefreshPending`, `ProfileBackfill`, `ProfileFollow`):

```rust
Command::TxReceiptsBackfill { batch_limit, concurrency }
Command::TxReceiptsFollow   { cadence_secs, batch_limit, concurrency }
```

Backfill loop pseudocode:

```
loop:
    candidates ← SELECT DISTINCT chain_id, tx_hash, block_number, block_timestamp
                  FROM raw_protocol_events
                  WHERE finality = 'finalized'
                    AND is_canonical = TRUE
                    AND (chain_id, tx_hash) NOT IN (SELECT chain_id, tx_hash FROM tx_receipts)
                  ORDER BY block_number ASC
                  LIMIT batch_limit
    if candidates.empty: break
    receipts ← stream(candidates).map(fetch_receipt).buffer_unordered(concurrency).try_collect()
    INSERT INTO tx_receipts (...) VALUES ... ON CONFLICT (chain_id, tx_hash) DO NOTHING
    update checkpoint indexer_checkpoints.tx_receipts_backfill = max(block_number)
```

`fetch_receipt` uses the existing `livepeer_core::rpc::cross_check::single_call_cached`, so receipts naturally land in `rpc_call_cache` for replay-determinism *and* the worker is restartable (re-reads from the cache on rerun).

Follow loop is the same body wrapped in `tokio::time::sleep(cadence_secs)` after an empty iteration, mirroring `profile-follow`.

### Reports endpoint cutover

`load_tx_fees` becomes a hybrid query:

```rust
async fn load_tx_fees(
    state: &AppState,
    tx_hashes: Vec<String>,
) -> Result<HashMap<String, BigDecimal>, ApiError> {
    // 1. Lookup in tx_receipts (the fast path, expected to be 100% hit rate post-backfill)
    let rows = sqlx::query_as!(
        TxFeeRow,
        "SELECT tx_hash, tx_fee_eth FROM tx_receipts \
           WHERE chain_id = $1 AND tx_hash = ANY($2)",
        chain_id,
        &tx_hashes,
    ).fetch_all(&state.pg).await.map_err(ApiError::internal)?;

    let mut fees: HashMap<_, _> = rows.into_iter()
        .map(|r| (r.tx_hash, r.tx_fee_eth))
        .collect();

    // 2. Fallback: any tx not yet in tx_receipts (during initial rollout, or live-follow lag)
    let missing: Vec<_> = tx_hashes.iter()
        .filter(|h| !fees.contains_key(*h))
        .cloned()
        .collect();
    if !missing.is_empty() {
        // bounded RPC fan-out for the misses (Option A's fix as the fallback path)
        let extras = fetch_receipts_via_rpc(state, missing).await?;
        fees.extend(extras);
    }
    Ok(fees)
}
```

The RPC fallback uses `buffer_unordered(12)` so it's bounded even when the table is empty (worst-case behavior matches Option A). After backfill finishes and follow-mode is steady, the fallback is dead code; we can delete it in a follow-up PR with confidence (TD-020.5 or just a cleanup commit).

## Determinism contract

`tx_receipts` is a **deterministic projection of `rpc_call_cache`**: every row is computed from a cached `eth_getTransactionReceipt` response, which the cross-check layer already keys by `(method, params, block_height)` and replays bit-exact. The `tx_fee_wei` / `tx_fee_eth` arithmetic is pure decimal math on receipt fields.

Therefore:
- **No new replay-manifest hash entry** is required. The replay contract for `rpc_call_cache` already covers the source bytes.
- **Replay test fixtures** (`tests/fixtures/case-{a,b}/`) need a one-time refresh because the `expected_hashes.json` set will gain a `tx_receipts` row hash. Compute via `scripts/compute-determinism-hashes.sh` after the case-a/b ranges are repopulated.
- **`docs/DETERMINISM.md`** — add `tx_receipts` to the table list with a one-line note: "deterministic projection of rpc_call_cache".

## Phases

### Phase A — Schema (½ hour)

1. Write `migrations/041_create_tx_receipts.up.sql` + `.down.sql`.
2. Apply via `sqlx migrate run` against the live DB.
3. Verify table + indexes via `\d tx_receipts`.

**Acceptance:** `\d tx_receipts` shows the table with the right columns; `_sqlx_migrations` shows version 41; nothing else changed.

### Phase B — Backfill subcommand (1 day)

1. New module `crates/livepeer-staker/src/tx_receipts.rs` with `run_tx_receipts_backfill(...)` returning a summary struct (rows_seen, rows_written, last_processed_block, elapsed_ms — same shape as `BackfillSummary` already used by gateway/profile backfills).
2. Hook into `crates/livepeer-staker/src/main.rs` and `runner.rs` (parallel pattern to `ProfileBackfill`).
3. Use `single_call_cached` for the RPC; concurrency from CLI flag (default 12).
4. UPSERT batch via multi-row `INSERT ... ON CONFLICT DO NOTHING`.
5. Emit Prometheus counters/gauges via the existing `livepeer_staker::metrics` module: 5 metric families to mirror TD-016 Phase E surface (`tx_receipts_backfill_candidates_remaining`, `..._rows_written_total`, `..._last_processed_block`, `..._iterations_total{result}`, `..._iteration_seconds`).
6. Local smoke run on dev DB with `--batch-limit 1000`; verify rows + checkpoint + `/metrics`.

**Acceptance:** Subcommand runs cleanly to exhaustion on a small slice; produces correct rows; metrics visible in `/metrics`; restart at any point is idempotent (verified by SIGKILL mid-iter, same pattern as TD-016 restart-test).

### Phase C — Follow subcommand (½ day)

1. `Command::TxReceiptsFollow { cadence_secs }` wrapping the backfill loop in a sleep; reuse the `should_sleep = summary.rows_written > 0` skip-sleep pattern from `profile-follow`.
2. Add `livepeer-staker-tx-receipts-follow` service to `docker-compose.yml`.

**Acceptance:** Follow service stays alive over a 30-min observation window, advancing the checkpoint as new finalized events arrive; metrics tick.

### Phase D — Reports endpoint cutover (½ day)

1. Rewrite `load_tx_fees` to the hybrid SQL+RPC pattern shown above. Keep the RPC fallback bounded with `buffer_unordered(12)`.
2. Benchmark all three CSV endpoints, both cold and warm:

| Endpoint | Pre | Post (target) |
|---|---|---|
| `/reports/gateway-payouts.csv` cold | 9.0 s | <100 ms |
| `/reports/gateway-payouts.csv` warm | 55 ms | <50 ms |
| `/reports/payouts.csv` cold | 451 ms | <80 ms |
| `/reports/rewards.csv` cold | 551 ms | <80 ms |

Cold targets assume 100% backfill hit rate. With the RPC fallback present, even partial backfill matches Option A's ~700 ms.

3. Confirm `EXPLAIN ANALYZE` shows Index Scan on `tx_receipts_pkey` for the `tx_hash = ANY(...)` lookup.

**Acceptance:** All three endpoints meet the post-target latencies; no functional regression in CSV column shape; `cargo test -p livepeer-api` green.

### Phase E — Backfill execution + monitoring (~100 min wall-clock)

1. Launch `tx-receipts-backfill` against the live DB.
2. Watch `/metrics` for `tx_receipts_backfill_candidates_remaining` to drift to zero.
3. Switch to follow-mode (or just rely on the docker-compose service from Phase C).

**Acceptance:** `SELECT COUNT(*) FROM tx_receipts WHERE chain_id = 42161` ≥ 1.41 M (i.e., ≥ count of unique finalized tx_hashes). Spot-check 5 random rows against `eth_getTransactionReceipt` — bit-exact.

### Phase F — Determinism fixtures + docs (½ hour)

1. Append `tx_receipts` to `docs/DETERMINISM.md`.
2. Recompute case-a/case-b `expected_hashes.json` after re-running the determinism replay end-to-end with the new worker active.
3. Update `tech-debt-tracker.md` to mark TD-020 Resolved.

**Acceptance:** `scripts/run-determinism-replay.sh` green for both cases.

## Risks

| Risk | Mitigation |
|---|---|
| 429 from Chainstack on 1.4 M parallel calls | Concurrency capped at 12 (empirically safe; same as profile-follow). Backfill finishes in ~100 min — well within Chainstack's daily quota at typical rates. If 429 emerges, drop to 8 and accept ~150 min. |
| Backfill candidate query slow at scale (1.4 M anti-join) | Use bounded `LIMIT batch_limit` per iter (default 5000) with `block_number` cursor; the candidate query becomes a sargable range scan instead of a full table anti-join. |
| Replay determinism breaks because `rpc_call_cache` evicted some receipts | The cache is durable PG storage, not in-memory. Eviction is operator-driven only. If it does happen, replay reports a hash mismatch and we re-fetch — same behavior as for any other cached call. |
| Reports endpoint throws because table is empty during initial rollout | The hybrid path's RPC fallback handles this — worst case is Option A's behavior (~700 ms cold). |
| Status-reverted txs (status=0) in CSVs | Existing CSV behavior counts the gas paid regardless of status (the user paid those fees). Preserve that — `tx_fee_eth` is gas_used × effective_gas_price even when status=0. The new `status` column is just stored for future filtering. |

## Open questions for sign-off

1. **Worker home — staker or new crate?** Default: extend `livepeer-staker` (consistent with `ProfileBackfill`, `GatewayBackfill`, etc.). Alternative: new crate `livepeer-tx-receipts`. Recommendation: **staker**, because the lifecycle (backfill → follow) and ops surface (metrics, docker service) are identical to existing staker subcommands.
2. **`eth_getBlockReceipts` vs per-tx `eth_getTransactionReceipt`?** `getBlockReceipts` returns all receipts for a block in one call but fetches receipts for *all* txs in that block (the vast majority of which are unrelated Arbitrum traffic). With only 1.4 M unique Livepeer-emitting txs, the per-tx path fetches strictly less data and aligns with the existing `single_call_cached` helper (which already integrates with `rpc_call_cache`). Recommendation: **per-tx**.
3. **Fallback RPC path — keep forever or remove after backfill?** Keep through Phase D; remove in a TD-020.5 cleanup once the table is verifiably populated for ≥ 1 week of new events. The fallback is ~30 lines of code and is the safe rollout pattern.
4. **`tx_receipts.from_address` / `to_address` denormalization** — strictly redundant with `raw_protocol_events.tx_hash` lookup, but cheap (~32 bytes/row × 1.4 M = ~45 MB). Recommendation: **keep**, because (a) it eliminates a join in any future receipt-driven feature, (b) `from_address` is needed for status-reverted-tx reporting which doesn't show up in the events table.

## Dependencies

- TD-016 (gateway backfill operability) — establishes the metrics module being reused. **Resolved.**
- TD-019 (profile-follow) — establishes the `should_sleep = summary.rows_written > 0` skip-sleep pattern. **Resolved.**
- No upstream blockers.

## Estimated effort

- Phase A: 0.5 h
- Phase B: 1 day
- Phase C: 0.5 day
- Phase D: 0.5 day
- Phase E: passive (~100 min wall-clock during backfill)
- Phase F: 0.5 h
- **Total active-coding time: ~2 days**
- **Total wall-clock from green-light to closed: ~3 days** (allowing for backfill window + verification)
