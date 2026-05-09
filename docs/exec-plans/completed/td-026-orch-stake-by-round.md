# TD-026: Orchestrator Stake-by-Round Historical Table

**Status:** Draft, awaiting sign-off
**Author:** 2026-05-08
**Severity:** medium
**Source:** Profile-follow throughput investigation 2026-05-08 — orch fanout walks every NewRound × 1,936 orchs and discards intermediate results

## Problem

`livepeer-staker profile-follow` walks every `NewRound` event in `raw_protocol_events`. For each NewRound, it fans out to all 1,936 known orchestrators and reads:
- `BondingManager.transcoderTotalStake(orch)` at the NewRound's block
- `Controller.getContract("ServiceRegistry")` (per-block, cached after first orch)
- `ServiceRegistry.getServiceURI(orch)` at the NewRound's block

The output is upserted into `orchestrator_profile` keyed by `(chain_id, address)` with `WHERE EXCLUDED.last_event_id > orchestrator_profile.last_event_id` — only the latest snapshot survives.

**Every prior snapshot is read and discarded.** Unlike gateway snapshots (which persist into `gateway_balances_by_block`), there is **no historical sibling table** for orchestrator total stake. The walk produces real per-round data but throws it away after deciding it's not the latest.

The cost:
- ~1,703 historical NewRound events × 1,936 orchs × 2 RPCs ≈ 6.6 M RPC reads (mostly cached after first run)
- Observed advance rate: ~26 M blocks/hr → **~6 hours to catch up**
- Output: 1,936 unique orchestrator rows (one per known address)

The data we throw away is meaningful: per-round total-stake history is the foundation for any "stake history chart," "stake leaderboard time series," "orch decline detection," etc. Today none of those features can exist because the historical data was never persisted.

## Resolution

Add a new table `orch_stake_by_round` that persists every per-round snapshot the worker produces. Convert `orchestrator_profile` into a derived view (analogous to TD-025's `broadcaster_profile`) that surfaces the latest row per orch.

The existing per-NewRound walk **continues to run** — but now its output is captured permanently instead of overwritten. Once backfill catches up to chain head, the historical record is complete and `orchestrator_profile` is derived from the freshest row.

## Scope

**In scope:**
- New migration `043_create_orch_stake_by_round` — table with PK `(chain_id, address, round)`, columns mirroring what `read_orchestrator_snapshot` reads + lifecycle/cuts joined from event tables
- Migration `044_replace_orchestrator_profile_with_view` — drop the table, create a `MATERIALIZED VIEW` over `orch_stake_by_round`
- Refactor the orch loop in `crates/livepeer-staker/src/profile.rs`:
  - Replace `upsert_orchestrator_profile` with `insert_orch_stake_by_round`
  - PK is `(chain_id, address, round)` so the worker INSERTs, never overwrites
  - `last_event_id`-based monotonic guard not needed (round-keyed)
- Refresh strategy for the matview: same 30 s cadence hook from TD-025 (extend or duplicate)
- API surface: any endpoint reading `orchestrator_profile` continues to work as before (same columns); new endpoints can read `orch_stake_by_round` directly for stake-history features

**Out of scope:**
- New API endpoints for stake-history charts (future work; the data layer is enough)
- Backfill freshness optimization (TD-022 — closed not landed)
- Removing TD-022's `livepeer_core::rpc::multicall` helper (still useful, no callers)
- O1 (chain-head-only orch_profile read for instant freshness) — explicitly deferred. The 6 h catch-up is acceptable since `orchestrator_profile` already lags during the existing walk, and the new table accumulates valuable historical data during that time.

## Architecture

### `orch_stake_by_round` schema

```sql
CREATE TABLE orch_stake_by_round (
    chain_id                  BIGINT       NOT NULL,
    address                   TEXT         NOT NULL,
    round                     BIGINT       NOT NULL,
    block_number              BIGINT       NOT NULL,
    block_timestamp           TIMESTAMPTZ  NOT NULL,
    block_hash                TEXT         NOT NULL,

    -- From the on-chain reads at the NewRound block
    total_stake               NUMERIC(38,18) NOT NULL,
    service_uri               TEXT,

    -- DB-derived from event tables (cuts / lifecycle as of this block)
    latest_fee_cut_percent    NUMERIC(10,4) NOT NULL,
    latest_reward_cut_percent NUMERIC(10,4) NOT NULL,
    latest_fee_share_percent  NUMERIC(10,4) NOT NULL,
    is_active                 BOOLEAN       NOT NULL,
    last_lifecycle_event_at   TIMESTAMPTZ,

    -- Provenance
    triggering_event_id       BIGINT NOT NULL REFERENCES raw_protocol_events(id),
    raw_call                  JSONB,
    created_at                TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    PRIMARY KEY (chain_id, address, round)
);

CREATE INDEX idx_orch_stake_address_round
   ON orch_stake_by_round (address, round DESC);
CREATE INDEX idx_orch_stake_round
   ON orch_stake_by_round (chain_id, round);
```

The `(chain_id, address, round)` PK means every (orch, round) snapshot is preserved. The walk does pure INSERTs on first pass and ON CONFLICT DO NOTHING on retries. **No data loss possible.**

### `orchestrator_profile` matview

```sql
CREATE MATERIALIZED VIEW orchestrator_profile AS
SELECT DISTINCT ON (chain_id, address)
  chain_id,
  address,
  total_stake,
  latest_fee_cut_percent,
  latest_reward_cut_percent,
  latest_fee_share_percent,
  is_active,
  last_lifecycle_event_at,
  block_number     AS as_of_block,
  round            AS as_of_round,
  triggering_event_id AS last_event_id,
  service_uri,
  NOW()            AS updated_at
FROM orch_stake_by_round
ORDER BY chain_id, address, round DESC;

CREATE UNIQUE INDEX orchestrator_profile_pkey
   ON orchestrator_profile (chain_id, address);
CREATE INDEX idx_orchestrator_profile_stake
   ON orchestrator_profile (total_stake DESC, address);
```

Same column shape as today's `orchestrator_profile`. API consumers see no difference.

### Worker refactor

In `run_profile_backfill`'s NewRound branch:

```rust
// OLD: upsert_orchestrator_profile(pg, &orch, &candidate, current_round, &snapshot).await?;
// NEW: insert into the round-keyed historical table; matview refresh advances orchestrator_profile.
insert_orch_stake_by_round(pg, &orch, &candidate, current_round, &snapshot).await?;
```

`insert_orch_stake_by_round` is a multi-row INSERT batched at the end of the per-NewRound fanout (one INSERT per NewRound covers all 1,936 orchs at once via `INSERT ... VALUES (...) ON CONFLICT (chain_id, address, round) DO NOTHING`). Bulk insert is far cheaper than 1,936 individual upserts.

The non-NewRound branch (TransferBond, individual lifecycle events) currently also calls `upsert_orchestrator_profile`. Two choices:
1. **Skip writing for non-NewRound events.** orchestrator_profile derives from the latest round; lifecycle events between rounds don't change the snapshot we care about (cuts come from event tables anyway). Recommended.
2. **Write a "non-round" pseudo-row** with a synthetic round id. More complex; preserves intermediate state at the cost of schema clarity. Not recommended.

Recommendation: option 1. Lifecycle events update `is_active`/`last_lifecycle_event_at` derivable from event tables; the matview can pull these from a left join against `raw_protocol_events` for the latest `TranscoderActivated`/`TranscoderDeactivated`. Simplest design.

### Migration shape

- `043_create_orch_stake_by_round.up.sql` — creates the table
- `043` `.down.sql` — drops it
- `044_replace_orchestrator_profile_with_view.up.sql` — drops the table, creates the matview
- `044` `.down.sql` — recreates the original empty table; on revert, the orch fanout would re-populate it via the existing path (after also reverting the worker refactor)

## Phases

### Phase A — Schema (1 hour)

1. Write `043_create_orch_stake_by_round.{up,down}.sql`.
2. Apply.
3. Verify with `\d orch_stake_by_round`.

**Acceptance:** Empty table exists; PK + indexes correct.

### Phase B — Worker refactor (2 hours)

1. Add `insert_orch_stake_by_round_batch` helper using a multi-row INSERT with `ON CONFLICT (chain_id, address, round) DO NOTHING`. Takes `Vec<(orch, snapshot)>` for the whole NewRound fanout.
2. In `run_profile_backfill`'s NewRound branch, replace the per-orch `upsert_orchestrator_profile` calls with one batched insert at the end of each NewRound's fanout.
3. Remove the non-NewRound branch's call to `upsert_orchestrator_profile` (confirmed unnecessary per architecture decision above).
4. Keep `upsert_orchestrator_profile` function alive for now (used during cutover migration); delete in Phase D.

**Acceptance:** Build clean. Smoke test: run `livepeer-staker profile-backfill --batch-limit 1` against a NewRound; verify ~1,936 rows inserted into `orch_stake_by_round`.

### Phase C — `orchestrator_profile` matview cutover (1 hour)

1. Write `044_replace_orchestrator_profile_with_view.{up,down}.sql`.
2. Before running: confirm `orch_stake_by_round` has been populated for at least the current orch_profile checkpoint range (so the matview, on first refresh, has rows).
3. Apply migration. The drop-then-create-matview happens in one transaction.
4. `REFRESH MATERIALIZED VIEW orchestrator_profile;` — initial population.
5. Verify row count and spot-check 3 orchs against expected current state.

**Acceptance:** matview returns same data as the old table for the same orchs.

### Phase D — Cleanup + refresh hook (1 hour)

1. Delete the now-unused `upsert_orchestrator_profile` function.
2. Add `orchestrator_profile` to the daemon's matview-refresh task added in TD-025 (so both views refresh on the same 30 s cadence).
3. Update `crates/livepeer-api/src/routes/*` if anything references columns not present in the matview (shouldn't be any — column names match).

**Acceptance:** Build clean; `cargo test` green; daemon log shows both matviews refreshing periodically.

### Phase E — Live cutover (½ hour)

1. Build release binaries for staker + daemon.
2. Kill the running profile-follow process.
3. Relaunch with the new binary.
4. Confirm: `orch_stake_by_round` row count growing, `orchestrator_profile` matview returning current data, no errors.

**Acceptance:** Profile-follow advancing as before, but writing to `orch_stake_by_round` instead of overwriting `orchestrator_profile`. After ~6 h backfill, `orch_stake_by_round` has full history.

### Phase F — Tracker + plan move (15 min)

1. Update `tech-debt-tracker.md` to mark TD-026 Resolved.
2. Move plan to `completed/`.

## Risks

| Risk | Mitigation |
|---|---|
| Existing replay fixtures depend on `orchestrator_profile` row hashes | Already stale post-TD-017 (see TD-024). Fixture regeneration is deferred separately; not blocking TD-026. |
| Matview refresh races with worker INSERT | `REFRESH MATERIALIZED VIEW CONCURRENTLY` is non-blocking for readers. INSERTs into the source table proceed in parallel. |
| `orch_stake_by_round` row count blow-up | 1,936 orchs × 1,703 NewRounds × ~2 yr history = ~3.3 M rows steady state. Tiny — fits in cache. Disk: ~500 MB conservatively. |
| Lifecycle (`is_active`, `last_lifecycle_event_at`) goes stale between NewRounds | The matview's lifecycle fields come from `orch_stake_by_round` rows generated at NewRound boundaries (~daily). Between NewRounds the values reflect the prior round. Acceptable resolution; matches today's behavior on the catch-up edge anyway. If finer resolution is needed later, add a cadence-driven matview-refresh-with-lifecycle-rejoin pass. |
| Determinism: matview row hashes change vs old table | Same column shape, same upstream data. The hashes will differ in Postgres' internal storage (matview vs table), but `compute-determinism-hashes.sh` operates over column values, so functionally identical. Re-fixture in TD-024 anyway. |

## Estimated effort

- Phase A: 1 h
- Phase B: 2 h
- Phase C: 1 h
- Phase D: 1 h
- Phase E: 0.5 h
- Phase F: 0.25 h
- **Total: ~5.75 hours**

## Expected impact

| Metric | Before | After |
|---|---|---|
| Per-NewRound work | 1,936 RPC reads + 1,936 row upserts (overwriting) | 1,936 RPC reads + 1 batched INSERT (1,936 rows preserved) |
| Historical orch stake data captured | None | All per-round snapshots (~3.3 M rows steady state) |
| `orchestrator_profile` definition | Real table populated by per-event walk | Matview over `orch_stake_by_round` |
| New features unlocked | — | Stake history chart, leaderboard time series, decline detection |
| 6 h backfill | Wastefully overwrites 1,936 rows over and over | Productively populates 3.3 M historical rows |

The 6 h orch backfill continues to run — but every minute of it now produces permanent value instead of being thrown away.
