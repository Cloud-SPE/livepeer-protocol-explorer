# TD-025: Broadcaster Profile as Derived View

**Status:** Draft, awaiting sign-off
**Author:** 2026-05-08
**Severity:** medium
**Source:** Profile-follow throughput investigation 2026-05-08 — `staker_gateway_profile` does ~600 K per-event RPC walks to materialize 13 rows that already exist in `gateway_balances_by_block`.

## Problem

`livepeer-staker profile-follow` populates `broadcaster_profile` by walking every gateway-touching event in `raw_protocol_events` (`DepositFunded`, `ReserveFunded`, `WinningTicketRedeemed`, `WinningTicketTransfer`, `ReserveClaimed`, `Withdrawal`, `Unlock`, `UnlockCancelled`) and issuing two `eth_call`s per event (`TicketBroker.getSenderInfo` + `TicketBroker.isUnlockInProgress`). Each result upserts into `broadcaster_profile` keyed by `(chain_id, address)` with `WHERE EXCLUDED.last_event_id > broadcaster_profile.last_event_id` — only the latest event wins.

**The walk is fully redundant:** `gateway_balance_backfill` (a separate worker, already at chain head) issues the **same two RPCs** at the **same blocks** for the **same gateways** and persists every (gateway, block) snapshot into `gateway_balances_by_block`. That table is a strict superset of `broadcaster_profile`'s columns, plus two additional fields (`reserve_claimed_in_current_round`, `withdraw_round`) that are currently invisible at the API layer.

Empirical cost of the redundancy:
- 599,434 trigger events; 558,255 still to walk
- ~1.12 M remaining RPCs (cached but each requires a `cache::get` + DB upsert)
- Observed advance rate: ~9.4 M blocks/hr → **~40 hours to catch up**
- Output: 13 unique gateway addresses (the same 13 that would be visible immediately via SQL DISTINCT-ON over `gateway_balances_by_block`)

## Resolution

Replace `broadcaster_profile` with a **materialized view** over `gateway_balances_by_block`. Drop the gateway half of `profile-follow` entirely. Expose the two extra fields. The 40 h backfill stops on cutover.

## Scope

**In scope:**
- Migration `042_replace_broadcaster_profile_with_view` — drop the table, create a `MATERIALIZED VIEW broadcaster_profile` over `gateway_balances_by_block`
- Add `reserve_claimed_in_current_round` and `withdraw_round` columns to the view (and to API responses where `broadcaster_profile` is read)
- Refresh strategy: `REFRESH MATERIALIZED VIEW CONCURRENTLY broadcaster_profile` triggered on a tight cadence (every ~30 s) by a small daemon hook, or on `gateway_balances_by_block` write triggers
- Delete the gateway half of `profile.rs` — `read_gateway_snapshot`, `fetch_gateway_candidates_after`, `GatewayCandidate`, `GatewaySnapshot`, the gateway loop in `run_profile_backfill`, the `GATEWAY_PROFILE_CHECKPOINT` constant
- Delete the `staker_gateway_profile` checkpoint row
- Update `ProfileBackfillSummary` to drop gateway fields
- API surface: any endpoint reading `broadcaster_profile` continues to work; new fields surface in the response shape (additive)

**Out of scope:**
- Backwards-compatibility alias (the columns map cleanly: `latest_deposit` ← `deposit`, `latest_reserve` ← `reserve_funds_remaining`)
- Gateway-balance worker itself — no change needed; it already produces the source data
- Per-block historical broadcaster lookups — already served by `gateway_balances_by_block`

## Architecture

### Materialized view definition

```sql
CREATE MATERIALIZED VIEW broadcaster_profile AS
SELECT DISTINCT ON (chain_id, gateway_address)
  chain_id,
  gateway_address                      AS address,
  deposit                              AS latest_deposit,
  reserve_funds_remaining              AS latest_reserve,
  reserve_claimed_in_current_round,
  withdraw_round,
  unlock_in_progress,
  block_number                         AS as_of_block,
  block_timestamp                      AS as_of_timestamp,
  triggering_event_id                  AS last_event_id,
  NOW()                                AS updated_at
FROM gateway_balances_by_block
ORDER BY chain_id, gateway_address, block_number DESC;

CREATE UNIQUE INDEX broadcaster_profile_pkey
   ON broadcaster_profile (chain_id, address);
CREATE INDEX idx_broadcaster_profile_deposit
   ON broadcaster_profile (latest_deposit DESC, address);
```

The unique index is required for `REFRESH MATERIALIZED VIEW CONCURRENTLY` (no locks on readers during refresh).

### Refresh strategy

Two options, choose at implementation time:
1. **Cadence-based**: a tiny daemon hook does `REFRESH MATERIALIZED VIEW CONCURRENTLY` every 30 s. Simple; bounded staleness ≤ 30 s.
2. **Trigger-based**: `AFTER INSERT/UPDATE` trigger on `gateway_balances_by_block` calls `pg_notify`; daemon listens and refreshes. Lower latency; more code.

Recommend **option 1** — gateway state changes are already at human-perceptible granularity (ticket redeems, deposits) and 30 s is well below any UI refresh expectation.

### Code deletions in `crates/livepeer-staker/src/profile.rs`

About 200 lines deleted:
- `GatewayCandidate` struct
- `GatewaySnapshot` struct
- `fetch_gateway_candidates_after`
- `read_gateway_snapshot`
- `read_unlock_period_seconds` (if unused after removal)
- The gateway loop in `run_profile_backfill`
- `GATEWAY_PROFILE_CHECKPOINT` constant
- `gateway_events_seen` / `gateway_rows_written` / `gateways_touched` / `gateway_checkpoint_block` fields in `ProfileBackfillSummary`
- Gateway fields in the corresponding `info!` summary log lines

The orchestrator path is left intact for TD-026.

## Phases

### Phase A — Migration + view + indexes (1 hour)

1. Write `migrations/042_replace_broadcaster_profile_with_view.up.sql`:
   - `DROP TABLE broadcaster_profile;`
   - Create the `MATERIALIZED VIEW` per definition above
   - Create unique + secondary indexes
2. Write the down migration that recreates the original table (migration reversal must work).
3. Apply via `sqlx migrate run` against the live DB.
4. `REFRESH MATERIALIZED VIEW broadcaster_profile;` — initial population.
5. Verify row count matches what the SQL DISTINCT-ON yields.

**Acceptance:** `\d broadcaster_profile` shows a matview with the new columns; `SELECT COUNT(*) FROM broadcaster_profile` returns ≥ 13 (current population).

### Phase B — Code deletion (1 hour)

1. Strip gateway-side code from `crates/livepeer-staker/src/profile.rs`.
2. Strip gateway-side fields from `ProfileBackfillSummary`.
3. Update `crates/livepeer-staker/src/main.rs` and `runner.rs` to remove the gateway summary fields from the log lines.
4. Update `crates/livepeer-api/src/routes/*.rs` callers to handle the new `broadcaster_profile` shape (added `reserve_claimed_in_current_round`, `withdraw_round`; renamed `gateway_address` → `address` if needed).
5. Build clean.

**Acceptance:** `cargo build --release -p livepeer-staker -p livepeer-api` green; `cargo test` green; profile-follow logs no longer reference gateway fields.

### Phase C — Refresh hook (½ hour)

1. Add a small daemon-hosted task that runs `REFRESH MATERIALIZED VIEW CONCURRENTLY broadcaster_profile` every 30 s. Place in `crates/livepeer-daemon/src/jobs/refresh_broadcaster_profile.rs` (new file) or inline in the existing follow-loop coordinator.
2. Add a Prometheus gauge `broadcaster_profile_refresh_seconds` (last refresh duration).

**Acceptance:** `broadcaster_profile.updated_at` advances every ≤ 30 s when the daemon is running; gauge exposed at `/metrics`.

### Phase D — Cutover (1 hour)

1. Build release binaries for staker + daemon + api.
2. Restart `livepeer-staker profile-follow` (smaller code path now).
3. Restart `livepeer-daemon follow` to pick up the refresh hook.
4. Confirm `broadcaster_profile` row count matches `gateway_balances_by_block` distinct gateway count (~13-100 depending on chain head).
5. Spot-check 3 gateways via the API: response should include the two new fields.
6. Confirm `staker_gateway_profile` checkpoint is no longer being touched (won't be queried; entry can stay for now and be cleaned up later).

**Acceptance:**
- `broadcaster_profile` populated to chain head immediately
- API responses include `reserve_claimed_in_current_round` + `withdraw_round`
- Profile-follow log no longer mentions gateway processing
- Daemon log shows periodic refreshes

### Phase E — Tracker close + plan move (15 min)

1. Update `tech-debt-tracker.md` to mark TD-025 Resolved.
2. Move plan from `active/` to `completed/`.

**Acceptance:** Tracker shows Resolved; plan in completed directory.

## Risks

| Risk | Mitigation |
|---|---|
| API consumers expect old `broadcaster_profile` columns | Old column names preserved (`latest_deposit`, `latest_reserve`, `unlock_in_progress`, `as_of_block`, `last_event_id`). Only additive: two new fields. |
| Matview refresh contention with concurrent reads | `REFRESH MATERIALIZED VIEW CONCURRENTLY` requires the unique index — included. No reader blocking. |
| Migration reversal | `down.sql` recreates the original empty table; data lives in `gateway_balances_by_block`, can be re-derived with the same SQL on rollback. |
| `gateway_balance_backfill` worker fails / regresses | Same risk as today — `broadcaster_profile` accuracy is now strictly tied to `gateway_balances_by_block` accuracy. The current dual-population is also subject to this; no new exposure. |

## Determinism contract

`broadcaster_profile` was always a derived current-state table — no replay-fixture entry depends on it via per-event walks. The matview is a deterministic SQL projection; its row hashes are stable as long as `gateway_balances_by_block` is. No replay fixture changes required.

## Estimated effort

- Phase A: 1 h
- Phase B: 1 h
- Phase C: 0.5 h
- Phase D: 1 h
- Phase E: 0.25 h
- **Total: ~4 hours**

## Expected impact

| Metric | Before | After |
|---|---|---|
| `broadcaster_profile` catch-up time | ~40 h | **immediate** (single REFRESH) |
| Profile-follow RPC budget consumed by gateway side | ~1.12 M cached `eth_call` lookups per backfill | **0** |
| Profile-follow code surface | ~200 lines of gateway path | **deleted** |
| API columns exposed | `latest_deposit`, `latest_reserve`, `unlock_in_progress` + 4 metadata | + `reserve_claimed_in_current_round`, `withdraw_round` |
| Refresh latency | event-driven (variable) | ≤ 30 s bounded |
