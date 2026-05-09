# Deployment

Production deployment shape for the current codebase.

This document reflects the shipped runtime split:

- historical catch-up via `livepeer-orchestrator bootstrap`
- deterministic rebuilds via `livepeer-orchestrator replay`
- steady-state follow mode via `livepeer-daemon follow`
- read API via `livepeer-api`

Authoritative references:
- [SPEC §15](product-specs/v1-livepeer-indexer.md#15-deployment--configuration)
- [RUNBOOK.md](RUNBOOK.md)
- [POSTGRES_MAINTENANCE.md](POSTGRES_MAINTENANCE.md)

## 1. Server prerequisites

Install:
- Docker
- Docker Compose plugin
- PostgreSQL client tools (`psql`, `pg_dump`, `pg_restore`)

Server expectations:
- enough disk for Postgres growth, especially `rpc_call_cache`
- firewall controls around metrics and DB ports
- stable archive RPC connectivity

Recommended exposed ports:
- `8080` for the API if you want remote access
- `9107` only to trusted Prometheus hosts if you expose daemon metrics directly
- `5432` bound to localhost/private only

## 2. Environment

Create a real `.env` from [.env.example](../.env.example).

Required values:
- `POSTGRES_USER`
- `POSTGRES_PASSWORD`
- `POSTGRES_DB`
- `POSTGRES_HOST`
- `POSTGRES_PORT`
- `DATABASE_URL`
- `CHAINSTACK_RPC_URL`
- `SECONDARY_RPC_URL`
- `L1_RPC_URL`

Optional alerting values:
- `TELEGRAM_BOT_TOKEN`
- `TELEGRAM_CHAT_ID`
- `ALERT_BOT_BIND`

Environment-specific YAML:
- production services should use `config/env/prod.yaml`

## 3. Compose files

Use:
- [docker-compose.prod.yml](../docker-compose.prod.yml) for production shape
- [docker-compose.yml](../docker-compose.yml) for the older per-binary dev/service layout

Production services in `docker-compose.prod.yml` (9 always-on):
- `postgres`
- `livepeer-daemon` (runs indexer + finality + reorg + valuator + staker
  loops + matview refresh internally per TD-012/25/26)
- `livepeer-api`
- `livepeer-rollups-payouts` / `-rewards` / `-tickets` / `-event-metrics`
  (TD-018; daily aggregations the dashboard reads from)
- `livepeer-enricher` (ENS name + avatar resolution; needs `L1_RPC_URL`)
- `livepeer-staker-tx-receipts` (TD-020; backs report CSVs)

Profile-gated:
- `--profile ops` → `livepeer-alert-bot` (Telegram alerting)
- `--profile tools` → `livepeer-orchestrator`, `livepeer-seed-migrator`
  (one-shot bootstrap / migrate / seed import)

The 6 worker services above are NOT inside the daemon. Without them the
dashboard's 24 h aggregates go stale, ENS names don't refresh, and report
CSVs fall back to slow per-tx RPC for unknown tx fees.

## 4. First-time bootstrap

Do **not** start `follow` on an empty database.

First run:

```bash
docker compose -f docker-compose.prod.yml up -d postgres
```

Then run bootstrap:

```bash
docker compose -f docker-compose.prod.yml run --rm livepeer-orchestrator \
  bootstrap \
  --source-sqlite /seed/sqlite-4.0.db \
  --from-block 6072093 \
  --to-block <target_block>
```

Notes:
- mount the SQLite seed into `./seed/sqlite-4.0.db`
- pick `<target_block>` close enough to head that `follow` can take over

## 5. Start steady-state follow mode

Once bootstrap (or restore from a dev dump per §7) has reduced lag below
the gate:

```bash
# Bring up the full fleet
docker compose -f docker-compose.prod.yml up -d
```

Or, if you want to phase it:

```bash
docker compose -f docker-compose.prod.yml up -d postgres
# wait for healthy
docker compose -f docker-compose.prod.yml up -d livepeer-daemon livepeer-api
docker compose -f docker-compose.prod.yml up -d \
  livepeer-rollups-payouts livepeer-rollups-rewards \
  livepeer-rollups-tickets livepeer-rollups-event-metrics \
  livepeer-enricher livepeer-staker-tx-receipts
```

Optional Telegram alerting:

```bash
docker compose -f docker-compose.prod.yml --profile ops up -d livepeer-alert-bot
```

Health checks (commands run inside the container — the prod compose does
not expose ports by default):

```bash
docker exec livepeer-api    curl -sS http://127.0.0.1:8080/health
docker exec livepeer-daemon curl -sS http://127.0.0.1:9107/health
docker exec livepeer-api    curl -sS http://127.0.0.1:8080/network/stats
```

## 6. Catch-up boundary

`livepeer-daemon follow` enforces a startup lag gate. The current
prod compose uses:

```text
--max-start-lag-blocks 170000
```

Arbitrum produces ~250 ms blocks, so 170k blocks ≈ ~12 hours of lag.
If you start the daemon within that window of the snapshot/bootstrap
cutoff, you're fine. Past that, either bump the flag or run another
bounded `livepeer-orchestrator bootstrap` pass first.

## 7. Backups

Postgres lives in a docker container and runs a server version that
typically newer than the Ubuntu host's `pg_dump`/`pg_restore`. Always
invoke the client tools **inside the container** to match the server
version.

Backup (custom format, parallel-friendly, 4-5 GB compressed for current
prod size; takes ~3-5 min):

```bash
docker exec -e PGPASSWORD="$POSTGRES_PASSWORD" livepeer-valuation-postgres \
  pg_dump -U "$POSTGRES_USER" -d "$POSTGRES_DB" -Fc \
  > backups/livepeer_$(date -u +%Y%m%dT%H%M%SZ).dump
```

The repo helper `scripts/backup-postgres.sh` is host-shell-friendly only
when `DATABASE_URL` resolves from the host (i.e. when you've added
`127.0.0.1` host networking or are running outside docker). For the
standard docker setup, use the command above.

Restore (drop + recreate the target DB, then parallel restore via
`docker cp` of the dump into the container):

```bash
DUMP=/path/to/livepeer_<timestamp>.dump

# 1. drop + recreate
docker exec -e PGPASSWORD="$POSTGRES_PASSWORD" livepeer-valuation-postgres \
  psql -U "$POSTGRES_USER" -d postgres \
    -c "DROP DATABASE IF EXISTS $POSTGRES_DB;" \
    -c "CREATE DATABASE $POSTGRES_DB OWNER $POSTGRES_USER;"

# 2. copy in + parallel restore (--jobs requires a file path, not stdin)
docker cp "$DUMP" livepeer-valuation-postgres:/tmp/restore.dump
docker exec -e PGPASSWORD="$POSTGRES_PASSWORD" livepeer-valuation-postgres \
  pg_restore -U "$POSTGRES_USER" -d "$POSTGRES_DB" \
    --no-owner --no-privileges --jobs 4 --verbose \
    /tmp/restore.dump
docker exec livepeer-valuation-postgres rm /tmp/restore.dump

# 3. refresh matviews to align with restored base tables
docker exec -e PGPASSWORD="$POSTGRES_PASSWORD" livepeer-valuation-postgres \
  psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -c \
  "REFRESH MATERIALIZED VIEW orchestrator_profile;
   REFRESH MATERIALIZED VIEW broadcaster_profile;"
```

Slower stdin alternative (no docker cp; single-threaded):

```bash
docker exec -i -e PGPASSWORD="$POSTGRES_PASSWORD" livepeer-valuation-postgres \
  pg_restore -U "$POSTGRES_USER" -d "$POSTGRES_DB" \
    --no-owner --no-privileges --verbose \
  < "$DUMP"
```

Because deterministic inputs live in Postgres, a full database backup
covers:
- `rpc_call_cache`
- `seeded_event_prices`
- `contract_abi_registry`
- raw/derived tables
- the `tx_receipts` table (TD-020) and rollup tables (TD-017/18)

## 8. Upgrade procedure

Before deploy:
1. take a DB backup
2. build/pull the new image
3. review migrations

Deploy:

```bash
docker compose -f docker-compose.prod.yml up -d postgres
docker compose -f docker-compose.prod.yml run --rm livepeer-orchestrator migrate-only
docker compose -f docker-compose.prod.yml up -d livepeer-daemon livepeer-api
```

If using alerting:

```bash
docker compose -f docker-compose.prod.yml --profile ops up -d livepeer-alert-bot
```

After deploy:
- check daemon `/health`
- check API `/health`
- inspect `/backfills/status`
- inspect daemon `/metrics`

## 9. Recovery modes

Normal recovery:
- restore Postgres backup
- restart `livepeer-daemon` and `livepeer-api`

Deterministic recovery:
- preserve deterministic inputs
- rerun `replay` or `bootstrap` depending on the recovery objective

Useful commands:

```bash
docker compose -f docker-compose.prod.yml run --rm livepeer-orchestrator \
  replay \
  --source-sqlite /seed/sqlite-4.0.db \
  --from-block <from_block> \
  --to-block <to_block>
```

## 10. Monitoring

Daemon metrics:

```bash
curl http://127.0.0.1:9107/metrics
```

API metrics:

```bash
curl http://127.0.0.1:8080/metrics
```

Repo-managed alerting artifacts:
- Prometheus rules: `ops/prometheus/daemon-alerts.yml`
- Alertmanager config: `ops/alertmanager/alertmanager.yml`

Alert path:
- Prometheus scrapes daemon metrics
- Alertmanager routes to `livepeer-alert-bot`
- `livepeer-alert-bot` sends Telegram
