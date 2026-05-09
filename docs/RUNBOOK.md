# Runbook

Operational procedures for the Livepeer indexing and valuation system.

This document reflects the current runtime shape:

- bounded historical runs via `livepeer-orchestrator bootstrap`
- deterministic rebuilds via `livepeer-orchestrator replay`
- near-head continuous operation via `livepeer-daemon follow`
- read API via `livepeer-api`

Authoritative reference: [SPEC §19](product-specs/v1-livepeer-indexer.md#19-operations-runbook).

Deployment walkthrough:
- [DEPLOYMENT.md](DEPLOYMENT.md)
- [POSTGRES_MAINTENANCE.md](POSTGRES_MAINTENANCE.md)

## 1. Daily operations

Primary processes:
- `livepeer-daemon follow`
- `livepeer-api`
- `livepeer-alert-bot` (when Telegram alerting is enabled)
- Postgres

Health checks:
- daemon: `curl http://127.0.0.1:9107/health`
- daemon metrics: `curl http://127.0.0.1:9107/metrics`
- api: `curl http://127.0.0.1:8080/health`
- api metrics: `curl http://127.0.0.1:8080/metrics`
- alert-bot: `curl http://127.0.0.1:9111/health`

Healthy indicators:
- daemon `/health` returns `ok`
- api `/health` returns `ok`
- `livepeer_chain_head_block` is moving
- `livepeer_task_lag_blocks{task="indexer"}` stays bounded
- `livepeer_iterations_total{task=...}` increases over time
- `livepeer_iteration_failures_total` remains flat or near-flat
- `livepeer_rpc_divergence_total` remains flat

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
- per-task soft caps:
  - indexer `8`
  - finality `2`
  - reorg `2`
  - valuator `16`
  - staker `6`

## 4.1 Alerting

Repo-managed alerting artifacts:
- Prometheus rules: `ops/prometheus/daemon-alerts.yml`
- Alertmanager receiver config: `ops/alertmanager/alertmanager.yml`
- Telegram bridge binary: `livepeer-alert-bot`

Alert flow:
1. Prometheus scrapes daemon `/metrics`
2. Prometheus evaluates `ops/prometheus/daemon-alerts.yml`
3. Alertmanager posts grouped alerts to `livepeer-alert-bot`
4. `livepeer-alert-bot` formats and forwards them to Telegram

Current alert classes:
- `LivepeerRpcDivergenceDetected`
- `LivepeerIndexerLagHigh`
- `LivepeerIterationFailuresHigh`
- `LivepeerRpcTaskSoftCapSaturated`

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
  - `orch_stake_by_round` (TD-026)
  - `tx_receipts` (TD-020)
  - `event_metrics_daily` (TD-018)
- after the truncate, refresh the matviews so they reflect the empty state:
  - `REFRESH MATERIALIZED VIEW broadcaster_profile;`
  - `REFRESH MATERIALIZED VIEW orchestrator_profile;`
- rerun valuator + staker + rollup stages
- (the replay path does all of this for you — see "Deep replay" above)

If raw events are suspect:

`bootstrap` does not reset the database — it assumes a clean DB and is
idempotent on top of existing state. To rebuild raw events from cache:

- **Preferred**: `livepeer-orchestrator replay --to-block <head>` (without
  `--keep-raw-events`). This truncates raw + all derived state, then
  re-runs the indexer using `rpc_call_cache` as the source. Determinism
  contract preserved.
- **Nuclear option**: `DROP DATABASE livepeer_indexer; CREATE DATABASE
  livepeer_indexer;` then run `bootstrap` from scratch. Only viable if
  you have a `seeded_event_prices` SQLite source still — without it,
  the seed import phase has nothing to feed off.

**Do not** "rerun bootstrap" on a populated DB expecting recovery: the
indexer's writes are intentionally idempotent, so dirty rows survive
the rerun. The `truncate_for_bootstrap` function exists in
`crates/livepeer-orchestrator/src/reset.rs` but is intentionally not
wired into `bootstrap::run` — invoking it requires explicit operator
intent.

**Never** truncate `rpc_call_cache`, `seeded_event_prices`, or
`raw_protocol_events` outside of an intentional, backup-protected
deterministic-rebuild operation. See `docs/POSTGRES_MAINTENANCE.md` for
the do-not-purge list.

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

### Telegram alert: `LivepeerRpcDivergenceDetected`

Meaning:
- provider cross-check disagreement occurred
- treat as determinism-risk until explained

Action:
1. inspect daemon logs around the alert timestamp
2. inspect `livepeer_rpc_divergence_total`
3. inspect `rpc_divergence_failures` if present in the current environment
4. do not auto-clear by restart alone; confirm whether the issue is transient provider disagreement or a deeper decoding/cache problem

### Telegram alert: `LivepeerIndexerLagHigh`

Meaning:
- indexer lag stayed above threshold for at least 5 minutes

Action:
1. inspect `livepeer_task_lag_blocks{task="indexer"}`
2. inspect `indexer_checkpoints`
3. inspect `livepeer_iteration_failures_total{task="indexer"}`
4. if iterations are succeeding but lag still grows, inspect RPC provider latency / TD-011 symptoms

### Telegram alert: `LivepeerIterationFailuresHigh`

Meaning:
- a daemon task recorded repeated failed iterations in a short window

Action:
1. identify the task label from the alert payload
2. inspect daemon logs for that task
3. inspect database reachability / provider reachability depending on `error_kind`
4. if failures are persistent, stop assuming self-heal and intervene

### Telegram alert: `LivepeerRpcTaskSoftCapSaturated`

Meaning:
- one daemon task has spent sustained time near its RPC soft cap

Action:
1. inspect `livepeer_task_rpc_in_flight{task=...}`
2. compare against `livepeer_task_rpc_limit{task=...}`
3. determine whether the pressure is expected steady-state load or a pathology
4. if the same task also shows lag/failures, treat this as a real bottleneck rather than noise

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
