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
- `9106` and `9107` only to trusted Prometheus hosts
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

Production services in `docker-compose.prod.yml`:
- `postgres`
- `livepeer-daemon`
- `livepeer-api`
- optional `livepeer-alert-bot`
- one-shot tools:
  - `livepeer-orchestrator`
  - `livepeer-seed-migrator`

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

Once bootstrap has reduced lag sufficiently:

```bash
docker compose -f docker-compose.prod.yml up -d livepeer-daemon livepeer-api
```

If Telegram alerting is configured:

```bash
docker compose -f docker-compose.prod.yml --profile ops up -d livepeer-alert-bot
```

Health checks:

```bash
curl http://127.0.0.1:9107/health
curl http://127.0.0.1:8080/health
curl http://127.0.0.1:9111/health
```

## 6. Catch-up boundary

`livepeer-daemon follow` enforces a startup lag gate.

Recommended contract:
- use `bootstrap` for empty DB or large lag
- use `follow` only once lag is under the configured threshold

The compose file uses:

```text
--max-start-lag-blocks 50000
```

If the daemon refuses to start, run another bounded bootstrap pass first.

## 7. Backups

Use full Postgres backups.

Helper script:

```bash
bash scripts/backup-postgres.sh
```

This writes a timestamped custom-format dump under `./backups/`.

Direct command:

```bash
pg_dump "$DATABASE_URL" -Fc -f /backups/livepeer_$(date -u +%Y%m%dT%H%M%SZ).dump
```

Restore:

```bash
pg_restore -d "$DATABASE_URL" /backups/livepeer_<timestamp>.dump
```

Because deterministic inputs live in Postgres, a full database backup covers:
- `rpc_call_cache`
- `seeded_event_prices`
- `contract_abi_registry`
- raw/derived tables

## 8. Upgrade procedure

Before deploy:
1. take a DB backup
2. build/pull the new image
3. review migrations

Deploy:

```bash
docker compose -f docker-compose.prod.yml build
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
