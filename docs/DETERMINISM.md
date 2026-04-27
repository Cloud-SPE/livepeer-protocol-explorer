# Determinism

The load-bearing correctness guarantee: given a fixed `rpc_call_cache` + seeded SQLite, replaying from an empty database produces a byte-identical output database.

This document explains the test contract and how to regenerate fixtures. Authoritative reference: [SPEC §12.4](product-specs/v1-livepeer-indexer.md#124-replay-determinism-test).

> **Status: skeleton.** Implementation pending.

## Test contract

```
GIVEN
  - tests/fixtures/rpc_cache.json (committed)
  - tests/fixtures/seed.sqlite    (committed)
  - empty Postgres
WHEN
  - migrations applied
  - seed-migrator run
  - indexer / reorg-watcher / finality-watcher / valuator / staker run for [from_block, to_block]
THEN
  - SHA256 of every table (rows sorted by PK, all columns) equals tests/fixtures/expected_hashes.json
```

## Fixture coverage required

Per SPEC §12.4:
- `Reward`
- `EarningsClaimed` (multi-asset)
- `Bond`
- `Unbond`
- `WinningTicketRedeemed`
- `NewRound` (non-monetary)
- `TranscoderUpdate` (non-monetary)
- An event with `seeded_event_prices` coverage
- An event without seed coverage (forces on-chain pricing)
- All required RPC cache entries

## Regenerating fixtures

```sh
# TODO: cargo run --bin livepeer-test -- regenerate-fixture --from-block N --to-block M
```

Regeneration is a deliberate action — it hits real RPC, captures into the cache, and commits. Reviewed in PR.

## CI gate

The determinism job (`.github/workflows/determinism.yml`) runs on every PR. Failure = merge blocked.
