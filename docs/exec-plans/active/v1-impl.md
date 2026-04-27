---
title: v1 implementation
status: in_progress
opened: 2026-04-27
owner: claude+mazup
links:
  - spec: ../../product-specs/v1-livepeer-indexer.md
---

## Goal

Take the v1.3 spec from scaffold to a working v1 system. This plan tracks the implementation rollout. It is updated each time a slice lands.

## Slice plan (depth-first per harness PDF)

Each slice ends with a green `cargo build --workspace`, ideally a runnable smoke test, and a commit. Slices are numbered, not dated — pace varies.

### S1 — foundation ✅ done

Goal: every binary can load config, connect to Postgres, log structured output. No real work yet.

- [x] `core::error` — single thiserror enum
- [x] `core::config` — loads `config/arbitrum.yaml` + `config/env/<env>.yaml` + `.env` via the env_var indirection layer
- [x] `core::db` — sqlx Postgres pool builder
- [x] `core::tracing_init` — JSON subscriber init helper (DRY across bins)
- [x] Migration `001_create_indexer_checkpoints` (up + down)
- [x] `livepeer-seed-migrator`: load config → connect DB → open SQLite read-only → log row counts → exit
- [x] Smoke test: `docker compose up -d postgres && sqlx migrate run && cargo run --bin livepeer-seed-migrator -- --source-sqlite ...` — verified end-to-end against live SQLite (payout=297,105 / reward=158,448 / events=625,463) and live Postgres
- [x] Workspace dep cleanup: dropped `rusqlite`, unified on `sqlx::sqlite` to avoid `libsqlite3-sys` link conflict

### S2 — schema (next)

Goal: all 14 migrations land. Schema verified with `psql \d`.

- [ ] Migrations 002 through 014 (per `migrations/README.md`)
- [ ] `tools/gen-schema-doc` — emit `docs/generated/db-schema.md` from migrations

### S3 — ABI registry + boot validation

- [ ] `core::abi` — load JSON files, compute sha256, populate `contract_abi_registry`
- [ ] `core::boot` — checks per SPEC §16.2: RPC reachable, schema-version match, ABI hashes match, Controller-resolved targets unchanged, Chainlink + sequencer feeds sane, pool cardinality sufficient
- [ ] `tools/verify-providers.sh` — shell version of the same checks

### S4 — RPC layer

- [ ] `core::rpc` — alloy provider, routing matrix per §13.2
- [ ] `core::rpc::cache` — `rpc_call_cache` table writes + reads, raw-bytes cross-check per §7.6
- [ ] Circuit breaker + token-bucket rate limit

### S5 — seed migrator (real work)

- [ ] Read SQLite payout + reward + events.payload (staging)
- [ ] Insert into `seeded_event_prices` per `docs/design-docs/sqlite-seed-mapping.md`
- [ ] Idempotent re-run

### S6 — indexer

- [ ] `eth_getLogs` with dynamic batch size
- [ ] ABI-driven decode against `contract_abi_registry`
- [ ] Strict-decode halt on critical events (§6.2)
- [ ] Atomic batch commit (events + checkpoint advance in one tx)
- [ ] Decode failures → dead letter
- [ ] Idempotent backfill command

### S7 — reorg + finality watchers

- [ ] Reorg watcher (§9.2 algorithm + cadence modes)
- [ ] Finality watcher (L1 batch posting + L1 finalization)

### S8 — valuator

- [ ] Pricing chain (§7.3) — TWAP × Chainlink with provenance JSONB
- [ ] **Degraded path** `v1_degraded_spot_pre_cardinality` (per Q-OD-9 finding — required for v1, not optional)
- [ ] Sequencer outage check (§7.3.4)
- [ ] Determinism guard (§10.5)

### S9 — staker

- [ ] Scope 2 — flow-derived principal + event-triggered pendingStake/pendingFees
- [ ] EarningsClaimed reconciliation
- [ ] `delegator_registry` derived from Bond events

### S10 — API

- [ ] Per SPEC §14.3 (events / valuations / prices / stake / aggregations / governance)
- [ ] Cursor pagination, sort whitelist, `with_valuations=true`, ETag

### S11 — cross-check + determinism CI

- [ ] `livepeer-test cross-check` — compare RPC events to SQLite events.payload (TD-004 / SPEC §24.1)
- [ ] Determinism replay test fixture (§12.4)
- [ ] CI workflow flips `determinism.yml` from placeholder to real

### S12 — observability + alerting

- [ ] Prometheus metrics catalog (§17.2)
- [ ] Telegram alerter (§10.6)
- [ ] Grafana dashboards (§17.3)

## Progress log

- **2026-04-27** Plan opened. Scaffold + SPEC v1.3 in place. All 10 SPEC §22 open data items resolved. Starting S1.
- **2026-04-27** S1 complete. Foundation in place; seed-migrator smoke-tested against live Postgres and live SQLite. Switched from `rusqlite` to `sqlx::sqlite` to resolve a `libsqlite3-sys` link conflict — single SQL library across both stores.
