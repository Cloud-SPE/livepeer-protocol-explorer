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

Key points:
- indexer `eth_getLogs` calls now go through `rpc_call_cache`
- valuator and staker RPC reads already route through cached call helpers
- finality replay uses recorded L1 timestamp inputs from the original live run
  instead of resolving live `latest`

## What replay still needs for full CI closure

Still pending:
- committed fixture set under `tests/fixtures/`
- committed expected table hashes
- real `tests/replay.rs`
- non-placeholder `.github/workflows/determinism.yml`

So the runtime contract is now much stronger than before, but the fully
automated CI fixture gate is still outstanding.

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

## CI target state

The final CI gate should:

1. stand up empty Postgres
2. apply migrations
3. load fixture cache + seed inputs
4. run strict replay
5. hash tables by stable primary-key order
6. compare to committed expected hashes

Failure should block merge.
