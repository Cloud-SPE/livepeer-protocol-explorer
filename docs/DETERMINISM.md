# Determinism

The load-bearing correctness guarantee is:

Given a fixed `rpc_call_cache`, the seeded SQLite input, and a fixed code
revision, replaying from cleared derived state produces a byte-identical output
database.

Authoritative reference: [SPEC §12.4](product-specs/v1-livepeer-indexer.md#124-replay-determinism-test).

## Current runtime contract

Strict replay is implemented through:
- `livepeer-orchestrator replay`
- explicit `--to-block`
- cache-only behavior by default
- failure on missing cached RPC inputs

Current replay inputs:
- `rpc_call_cache`
- `seeded_event_prices` import source when relevant
- recorded finality replay inputs stored from live finality passes

Escape hatch:

```sh
livepeer-orchestrator replay ... --allow-live-rpc
```

That mode is for debugging only. It is not the determinism contract.

## What replay covers now

Replay currently reconstructs:
- `raw_protocol_events` when `--keep-raw-events` is not set
- `event_valuations`
- `valuation_attempts`
- `token_prices_by_block`
- `stake_balances_by_block`
- `delegator_registry`
- `orch_stake_by_round` — per-round orchestrator snapshots (TD-026)
- `gateway_balances_by_block` — per-event gateway snapshots (TD-014)
- `orch_payouts_daily`
- `orch_rewards_daily`
- `tickets_daily`
- `tx_receipts` — deterministic projection of cached
  `eth_getTransactionReceipt` responses (TD-020)

The valuator's incremental-scan cursors are **cleared** on replay (they are a
scan-reduction hint, not hashed output):
- `valuator_cursors` — per-pass `finalized_at` high-water marks + the SEED
  change-detector marker (migration 047). `livepeer-orchestrator` truncates this
  alongside the other derived tables (`reset.rs`) so every pass cold-starts and
  re-scans full history. It **must** be cleared: a stale watermark left from a
  prior live run would make the ETH/LPT/MULTI passes scan only the recently
  finalized tail and silently skip rebuilding historical valuations — and the
  valuator's own cold-start guard (which keys off `event_valuations` being
  empty for the version) cannot catch this on its own, because the seed pass
  runs first and repopulates `event_valuations` before those passes read the
  cursor.

Materialized views (deterministic projections of the above; require an
explicit `REFRESH MATERIALIZED VIEW` before their content is observable
post-replay):
- `orchestrator_profile` — `SELECT DISTINCT ON (address) ... ORDER BY round DESC`
  over `orch_stake_by_round` (TD-026)
- `broadcaster_profile` — `SELECT DISTINCT ON (gateway) ... ORDER BY block_number DESC`
  over `gateway_balances_by_block` (TD-025)

In live mode, `livepeer-daemon` runs a 30-second `REFRESH MATERIALIZED VIEW
CONCURRENTLY` loop that keeps both matviews fresh. In replay mode (no
daemon), `scripts/run-determinism-replay.sh` issues a `REFRESH` before
computing hashes so the matviews materialize their post-replay state.

Key points:
- indexer `eth_getLogs` calls now go through `rpc_call_cache`
- valuator and staker RPC reads already route through cached call helpers
- finality replay uses recorded L1 timestamp inputs from the original live run
  instead of resolving live `latest`
- external tables remain explicitly out of scope for replay hashing:
  `orchestrator_ens`, `broadcaster_ens`, `name_avatar_overrides`,
  `broadcaster_classifications`

## Fixture contract

The committed replay fixtures live under `tests/fixtures/<case>/`:
- `seed.sqlite`
- `rpc_cache.csv`
- `replay_checkpoints.csv`
- `fixture.env`
- `expected_hashes.json`

The CI path is script-driven:

```sh
bash scripts/run-determinism-replay.sh
```

That script:
1. applies migrations
2. loads the cached RPC rows + replay finality checkpoints
3. runs strict replay over each committed fixture window
4. recomputes stable table hashes
5. diffs against `expected_hashes.json`

## Expected operator flow

1. Run a live bounded backfill or bootstrap
2. Preserve:
   - `rpc_call_cache`
   - seeded SQLite input
3. Clear derived state
4. Run:

```sh
livepeer-orchestrator replay \
  --source-sqlite /path/sqlite-4.0.db \
  --from-block <from> \
  --to-block <to>
```

5. Compare row hashes or baseline outputs

## Fixture coverage target

Per spec, the eventual committed fixture set must cover:
- `Reward`
- `EarningsClaimed` multi-asset
- `Bond`
- `Unbond`
- `WinningTicketRedeemed`
- `NewRound`
- `TranscoderUpdate`
- seeded pricing path
- non-seeded on-chain pricing path
- all required cached RPC inputs

## CI state

The GitHub determinism workflow now:

1. stands up empty Postgres
2. applies migrations via `livepeer-orchestrator migrate-only`
3. loads the committed fixture cache + replay checkpoints
4. runs strict replay
5. hashes the derived tables by stable primary-key order
6. compares to committed expected hashes

Failure blocks merge.
