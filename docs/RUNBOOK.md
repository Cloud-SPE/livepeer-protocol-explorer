# Runbook

Operational procedures for the Livepeer indexing and valuation system.

This document reflects the current runtime shape:

- bounded historical runs via `livepeer-orchestrator bootstrap`
- deterministic rebuilds via `livepeer-orchestrator replay`
- near-head continuous operation via `livepeer-daemon follow`
- read API via `livepeer-api`

Authoritative reference: [SPEC §19](product-specs/v1-livepeer-indexer.md#19-operations-runbook).

## 1. Daily operations

Primary processes:
- `livepeer-daemon follow`
- `livepeer-api`
- Postgres

Health checks:
- daemon: `curl http://127.0.0.1:9107/health`
- daemon metrics: `curl http://127.0.0.1:9107/metrics`
- api: `curl http://127.0.0.1:8080/health`
- api metrics: `curl http://127.0.0.1:8080/metrics`

Healthy indicators:
- daemon `/health` returns `ok`
- api `/health` returns `ok`
- `livepeer_chain_head_block` is moving
- `livepeer_task_lag_blocks{task="indexer"}` stays bounded
- `livepeer_iterations_total{task=...}` increases over time
- `livepeer_iteration_failures_total` remains flat or near-flat

Useful SQL checks:

```sql
SELECT name, last_processed_block, updated_at
FROM indexer_checkpoints
ORDER BY name;
```

```sql
SELECT finality, COUNT(*)
FROM raw_protocol_events
GROUP BY finality
ORDER BY finality;
```

```sql
SELECT status, COUNT(*)
FROM event_valuations
GROUP BY status
ORDER BY status;
```

## 2. Historical backfill

First-time or catch-up backfill:

```sh
livepeer-orchestrator bootstrap \
  --source-sqlite /path/sqlite-4.0.db \
  --from-block 6072093 \
  --to-block <target_block>
```

What it does:
- runs migrations
- seeds ABI registry
- imports SQLite seed if provided
- runs bounded indexer passes
- runs one finality pass
- runs valuator
- runs staker backfill + pending refresh
- optionally runs the SQLite cross-check

Use `bootstrap`, not `follow`, when lag is large or the database is empty.

## 3. Deterministic replay

Strict replay requires:
- committed or preserved `rpc_call_cache`
- seeded SQLite if seed import is part of the original run
- explicit `--to-block`

Command:

```sh
livepeer-orchestrator replay \
  --source-sqlite /path/sqlite-4.0.db \
  --from-block 6072093 \
  --to-block <target_block>
```

Current replay contract:
- strict by default
- fails on missing cached RPC inputs
- does not resolve live head
- reuses recorded finality inputs from the live finality pass

Escape hatch for debugging only:

```sh
livepeer-orchestrator replay ... --allow-live-rpc
```

That mode is not the determinism contract.

## 4. Near-head follow mode

Start:

```sh
livepeer-daemon follow --max-start-lag-blocks 50000
```

Behavior:
- refuses to start if current lag is above threshold
- runs bounded loops for:
  - indexer
  - finality
  - reorg watcher
  - valuator
  - staker
- exposes `/metrics` and `/health` on the daemon metrics bind

Current defaults:
- metrics bind: `0.0.0.0:9107`
- process-wide RPC concurrency ceiling: `24`

## 5. Recovery procedures

### Restore from database backup

Use normal Postgres `pg_dump` / restore procedures first when available.

### Deep replay from deterministic inputs

When derived state is corrupted but `rpc_call_cache` and seed inputs are intact:

1. Preserve `rpc_call_cache`, `seeded_event_prices`, `contract_abi_registry`
2. Reset derived state
3. Rerun `livepeer-orchestrator replay --to-block ...`
4. Validate table counts / hashes against the expected baseline

### Partial table recovery

If only derived tables are suspect:
- truncate and rebuild:
  - `event_valuations`
  - `valuation_attempts`
  - `token_prices_by_block`
  - `stake_balances_by_block`
  - `delegator_registry`
- rerun valuator + staker stages

If raw events are suspect:
- rerun full `bootstrap`

## 6. Failure response

### Daemon refuses to start

Likely cause:
- lag exceeds `--max-start-lag-blocks`

Action:
- use `bootstrap` to catch up first

### Replay fails with missing cache row

Likely cause:
- original run did not capture every needed RPC input
- requested range differs from the original cached range

Action:
- inspect the missing method from the error
- either:
  - rerun the original live path to populate cache, then replay again
  - or use `--allow-live-rpc` only for debugging, not determinism validation

### Reorg divergence detected

Action:
- inspect `reorg_events`
- confirm whether divergence is isolated and recent
- remember v1 reorg handling is audit-grade but not full mutation-grade:
  `reorg_mutations` remains incomplete

### Lag grows steadily

Action:
- inspect daemon metrics
- inspect `indexer_checkpoints`
- inspect `livepeer_iteration_failures_total`
- if valuator lags specifically, re-check TD-011 symptoms

## 7. Schema changes

Normal workflow:

```sh
sqlx migrate add <name>
cargo build --workspace
```

Rules:
- migrations are forward-only once merged
- do not rewrite merged migrations
- destructive changes require explicit operational intent

## 8. ABI updates

When a tracked contract implementation changes:

1. Identify upgrade block from controller or contract-management events
2. Fetch the new ABI
3. Compute SHA-256 and insert a new `contract_abi_registry` row
4. Bound the previous ABI row with `to_block`
5. restart relevant workers
6. rerun decode recovery if needed

See the product spec for the full upgrade procedure.
