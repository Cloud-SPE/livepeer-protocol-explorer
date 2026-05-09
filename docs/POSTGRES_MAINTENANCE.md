# Postgres Maintenance

Operational maintenance guide for the Postgres instance that backs the Livepeer indexer.

This is not a generic cleanup checklist. The database in this repo carries the determinism contract:

- `rpc_call_cache` + seeded SQLite + code revision must be sufficient to replay to the same output
- raw protocol events are immutable except for audited reorg handling
- many "derived" tables are still replay-covered state, not disposable cache

Authoritative references:
- [RUNBOOK.md](RUNBOOK.md)
- [DETERMINISM.md](DETERMINISM.md)
- [product-specs/v1-livepeer-indexer.md §12.4](product-specs/v1-livepeer-indexer.md#124-replay-determinism-test)

## 1. Safety rules

Before any maintenance:

1. Take a full logical backup.
2. Confirm whether the action is read-only, low-risk write maintenance, or destructive.
3. Stop writers for any blocking maintenance.
4. Record pre/post health checks and table sizes.

Backup command:

```bash
bash scripts/backup-postgres.sh
```

Writer stop order for a maintenance window:

1. stop `livepeer-api` if you want a quiet operator surface
2. stop `livepeer-daemon`
3. stop any one-shot tools or ad hoc backfills
4. leave Postgres up for maintenance

Restart order:

1. Postgres
2. `livepeer-daemon`
3. `livepeer-api`
4. optional `livepeer-alert-bot`

## 2. What is safe to clean up

Routine maintenance that is safe:

- `VACUUM (ANALYZE)` on live tables
- `ANALYZE`
- `REINDEX {INDEX|TABLE} CONCURRENTLY`
- refreshing materialized views
- autovacuum tuning
- planner statistics tuning
- inspecting and removing abandoned replication slots

Not routine cleanup in this repo:

- deleting old rows from `rpc_call_cache`
- deleting old rows from `seeded_event_prices`
- deleting old rows from `raw_protocol_events`
- pruning replay-covered derived tables just to save space
- `VACUUM FULL` without an explicit downtime window
- mass deletes on large tables during normal operation

Reason:

- `rpc_call_cache` is part of the replay contract
- `seeded_event_prices` is part of the trusted historical input set
- `raw_protocol_events` is canonical indexed state, not ephemeral telemetry
- many downstream tables are deterministic projections and should be rebuilt intentionally, not opportunistically trimmed

## 3. Table classes in this repo

Tables/operators should think about the schema in these buckets.

Never prune for routine storage cleanup:

- `rpc_call_cache`
- `seeded_event_prices`
- `contract_abi_registry`
- `raw_protocol_events`
- `reorg_events`
- `reorg_mutations`

Replay-covered derived state: maintain for health, but do not delete as routine retention:

- `event_valuations`
- `valuation_attempts`
- `token_prices_by_block`
- `stake_balances_by_block`
- `delegator_registry`
- `gateway_balances_by_block`
- `gateway_flows`
- `gateway_claimants_by_block`
- `orch_stake_by_round`
- `orch_payouts_daily`
- `orch_rewards_daily`
- `tickets_daily`
- `event_metrics_daily`
- `tx_receipts`
- `decode_failures`
- `rpc_divergence_failures`
- `indexer_checkpoints`

Derived materialized views: refreshable projections, not source tables:

- `orchestrator_profile`
- `broadcaster_profile`

External / operator-managed tables that are not part of replay hashing:

- `orchestrator_ens`
- `broadcaster_ens`
- `name_avatar_overrides`
- `broadcaster_classifications`

## 4. Highest-value recurring checks

### 4.1 Largest tables and indexes

Start here before doing anything else:

```sql
SELECT
  relname,
  pg_size_pretty(pg_total_relation_size(relid)) AS total_size,
  pg_size_pretty(pg_relation_size(relid)) AS table_size,
  pg_size_pretty(pg_total_relation_size(relid) - pg_relation_size(relid)) AS external_size
FROM pg_catalog.pg_statio_user_tables
ORDER BY pg_total_relation_size(relid) DESC
LIMIT 30;
```

```sql
SELECT
  schemaname,
  relname AS table_name,
  indexrelname AS index_name,
  pg_size_pretty(pg_relation_size(indexrelid)) AS index_size,
  idx_scan
FROM pg_stat_user_indexes
ORDER BY pg_relation_size(indexrelid) DESC
LIMIT 30;
```

Expected large tables in this repo:

- `rpc_call_cache`
- `raw_protocol_events`
- `event_valuations`
- `valuation_attempts`
- `tx_receipts`
- `gateway_balances_by_block`

### 4.2 Dead tuples and autovacuum health

```sql
SELECT
  schemaname,
  relname,
  n_live_tup,
  n_dead_tup,
  last_vacuum,
  last_autovacuum,
  last_analyze,
  last_autoanalyze
FROM pg_stat_user_tables
ORDER BY n_dead_tup DESC
LIMIT 25;
```

High-value tables to watch closely:

- `valuation_attempts`
- `rpc_call_cache`
- `gateway_balances_by_block`
- `gateway_flows`
- rollup tables that are frequently upserted

Why those:

- `valuation_attempts` has already shown real growth and restart cost issues in this repo
- `rpc_call_cache` is large and append-heavy
- gateway and rollup workers revisit rows as backfills advance

### 4.3 Long-running or idle transactions

```sql
SELECT
  pid,
  usename,
  application_name,
  client_addr,
  state,
  now() - xact_start AS xact_age,
  now() - query_start AS query_age,
  query
FROM pg_stat_activity
WHERE xact_start IS NOT NULL
ORDER BY xact_start;
```

Investigate anything sitting in `idle in transaction`. Long transactions block vacuum progress and are one of the easiest ways to create avoidable bloat.

### 4.4 Database growth

```sql
SELECT
  datname,
  pg_size_pretty(pg_database_size(datname)) AS size
FROM pg_database
ORDER BY pg_database_size(datname) DESC;
```

Track this over time. In this repo, growth is often legitimate historical state, not garbage. The question is whether growth matches expected backfill / cache accumulation.

## 5. Routine maintenance actions

### 5.1 Targeted `VACUUM (ANALYZE)`

Use normal vacuum first:

```sql
VACUUM (ANALYZE) valuation_attempts;
VACUUM (ANALYZE) event_valuations;
VACUUM (ANALYZE) raw_protocol_events;
VACUUM (ANALYZE) rpc_call_cache;
```

For a quiet maintenance window:

```sql
VACUUM (ANALYZE);
```

This is routine, safe, and high value. It reclaims space for reuse and refreshes planner stats. It does not usually return table files to the OS.

Avoid using `VACUUM FULL` as routine maintenance. It rewrites the table and takes a strong lock.

### 5.2 Targeted `ANALYZE`

Run `ANALYZE` after:

- large backfills
- large data imports
- index changes
- any one-time repair that materially shifts row counts or value distribution

Examples:

```sql
ANALYZE raw_protocol_events;
ANALYZE event_valuations;
ANALYZE gateway_balances_by_block;
ANALYZE tx_receipts;
```

### 5.3 Reindex bloated indexes

Preferred production form:

```sql
REINDEX INDEX CONCURRENTLY index_name;
```

Or for one table:

```sql
REINDEX TABLE CONCURRENTLY raw_protocol_events;
```

Use this when:

- a large index is materially bloated
- read performance regresses without a query-shape change
- write cost is growing due to oversized indexes

Do not drop indexes just because `idx_scan = 0` without checking:

- uptime / stats reset timing
- primary key / unique / foreign key support
- rare API/reporting paths
- maintenance or replay-only queries

### 5.4 Refresh materialized views

These are projections and can be refreshed without touching source truth:

```sql
REFRESH MATERIALIZED VIEW CONCURRENTLY broadcaster_profile;
REFRESH MATERIALIZED VIEW CONCURRENTLY orchestrator_profile;
```

In live mode the daemon already keeps them fresh, but this is safe after maintenance or verification work.

## 6. Autovacuum guidance

Autovacuum should remain enabled. For this workload, per-table tuning is usually better than broad global changes.

Example pattern for hot tables:

```sql
ALTER TABLE valuation_attempts SET (
  autovacuum_vacuum_scale_factor = 0.02,
  autovacuum_vacuum_threshold = 5000,
  autovacuum_analyze_scale_factor = 0.01,
  autovacuum_analyze_threshold = 5000
);
```

Start with hot tables, not the entire cluster:

- `valuation_attempts`
- `gateway_balances_by_block`
- `gateway_flows`
- `event_metrics_daily`
- `orch_payouts_daily`
- `orch_rewards_daily`
- `tickets_daily`

Be careful with `rpc_call_cache`:

- it is mostly append-only
- dead-tuple pressure may be low
- growth there is more likely a retention/capacity question than a vacuum tuning question

Useful cluster settings to inspect:

```sql
SHOW autovacuum;
SHOW autovacuum_naptime;
SHOW autovacuum_vacuum_scale_factor;
SHOW autovacuum_analyze_scale_factor;
SHOW log_autovacuum_min_duration;
```

If you change cluster settings in the current containerized deployment, ensure the change is persisted through your actual Postgres config path. Do not assume ad hoc in-container edits survive container replacement.

## 7. Query-performance maintenance

### 7.1 `pg_stat_statements`

If not already enabled at the cluster level, add `pg_stat_statements` through `shared_preload_libraries`, restart Postgres, then:

```sql
CREATE EXTENSION IF NOT EXISTS pg_stat_statements;
```

Use it to find actual cost centers:

```sql
SELECT
  calls,
  total_exec_time,
  mean_exec_time,
  rows,
  query
FROM pg_stat_statements
ORDER BY total_exec_time DESC
LIMIT 20;
```

This is usually higher value than speculative config tuning.

### 7.2 `EXPLAIN`

After identifying expensive queries:

```sql
EXPLAIN (ANALYZE, BUFFERS)
SELECT ...;
```

Typical wins in this repo are:

- missing composite indexes
- stale statistics
- scans against full-history event tables
- candidate queries that should short-circuit earlier

## 8. Bloat and space reclamation

Normal vacuum is the default answer.

Escalate only when:

- a table or index is materially bloated
- disk pressure is real
- normal vacuum is not stabilizing size or performance

Escalation order:

1. `VACUUM (VERBOSE, ANALYZE)` to inspect behavior
2. `REINDEX CONCURRENTLY` for index bloat
3. `pg_repack` for table/index rewrite with less blocking
4. `VACUUM FULL` only with an explicit downtime window

For this repo, prefer `pg_repack` over `VACUUM FULL` when reclaiming large table space in production.

## 9. Replication slots and WAL

The shipped compose deployment is single-host and does not configure replication by default, so this may often be empty. Still check it.

```sql
SELECT
  slot_name,
  active,
  restart_lsn,
  confirmed_flush_lsn
FROM pg_replication_slots;
```

More useful disk-retention form:

```sql
SELECT
  slot_name,
  active,
  restart_lsn,
  pg_size_pretty(pg_wal_lsn_diff(pg_current_wal_lsn(), restart_lsn)) AS retained
FROM pg_replication_slots
ORDER BY pg_wal_lsn_diff(pg_current_wal_lsn(), restart_lsn) DESC;
```

Inactive slots can retain WAL indefinitely and fill disks. Do not drop an inactive slot unless you know the consumer is truly gone.

## 10. What not to do casually

Do not do these as “cleanup”:

- `DELETE` old rows from `rpc_call_cache`
- `DELETE` old rows from `raw_protocol_events`
- `TRUNCATE` replay-covered tables to save disk
- `VACUUM FULL` on hot tables during normal hours
- non-concurrent `REINDEX` on production traffic paths
- dropping indexes based on one snapshot of `idx_scan = 0`

If you want retention or archival for replay-covered tables, that is a design change. Write it up first and decide how the determinism contract will be preserved.

## 11. Suggested cadence

Daily:

- check Postgres health
- check database size growth
- check long-running transactions
- check replication slots if any exist

Weekly:

- review largest tables and indexes
- review dead tuples and autovacuum timestamps
- run targeted `VACUUM (ANALYZE)` on hot tables if autovacuum is lagging
- review top queries from `pg_stat_statements`

Monthly:

- review index usage and candidate bloat
- reindex large bloated indexes concurrently where justified
- review cluster settings and maintenance logs

After every large backfill or repair:

- `ANALYZE` affected tables
- refresh materialized views if needed
- compare row counts / API behavior / query plans before and after

## 12. Incident checklist

If disk usage spikes:

1. check `pg_replication_slots`
2. check `pg_wal` growth cause
3. identify largest tables and indexes
4. inspect dead tuples and long-running transactions
5. do not start deleting replay inputs under pressure

If query latency regresses:

1. check `pg_stat_statements`
2. run `EXPLAIN (ANALYZE, BUFFERS)` on the worst query
3. check `last_analyze` / `last_autoanalyze`
4. check whether a specific index needs concurrent rebuild

If autovacuum falls behind:

1. identify the specific tables
2. run targeted `VACUUM (ANALYZE)`
3. tighten per-table autovacuum settings
4. investigate blocking long transactions

## 13. PostgreSQL features worth using

Official PostgreSQL docs relevant to this guide:

- `VACUUM`: https://www.postgresql.org/docs/current/sql-vacuum.html
- routine vacuuming / autovacuum: https://www.postgresql.org/docs/16/routine-vacuuming.html
- monitoring statistics: https://www.postgresql.org/docs/16/monitoring-stats.html
- `pg_stat_statements`: https://www.postgresql.org/docs/16/pgstatstatements.html

PG16-specific notes useful here:

- `pg_stat_io` exists and is useful for I/O visibility
- `pg_stat_statements` remains one of the highest-value performance tools
- cluster-wide maintenance privileges can be delegated with `pg_maintain` if you want a non-superuser maintenance role

Use those features, but keep the repo invariant first: maintenance must not quietly destroy replayability.
