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

### S2 — schema ✅ done (gen-schema-doc deferred to S3)

Goal: all 14 migrations land. Schema verified with `psql \d`.

- [x] Migrations 002 through 014 (per `migrations/README.md`)
- [x] All 14 applied clean against Postgres 15. 8 foreign keys verified, 2 check constraints verified.
- [x] SPEC §11.5 corrected (v1.4): `COALESCE(log_index, -1)` in PK is invalid Postgres syntax — replaced with `NOT NULL DEFAULT -1` sentinel + plain PK
- [ ] `tools/gen-schema-doc` — emit `docs/generated/db-schema.md` from migrations (defer to S3 — needs core::abi too)

### S3 — ABI registry + boot validation (in_progress)

- [x] `core::abi` — `hash_file()`, `verify_against_registry()`, `upsert()`. sha256 hash + idempotent upsert.
- [x] `livepeer-seed-migrator seed-abi-registry` subcommand — populates 7 contracts (Controller, BondingManager, TicketBroker, RoundsManager, LivepeerToken, Minter, Governor) from `abi/`. Idempotent (re-run = `inserted: 0, already: 7`).
- [x] `livepeer-seed-migrator probe` extended with ABI hash verification. **Tamper test passes:** appending a byte to `abi/Minter.json` triggers `AbiHashMismatch` and halts the probe before SQLite read; restoration returns to green.
- [ ] `core::boot` — full §16.2 checks (RPC reachable, schema-version match, Controller-resolved targets unchanged, Chainlink + sequencer feeds sane, pool cardinality sufficient). RPC-related checks need `core::rpc` (S4); ship the rest now.
- [ ] `tools/verify-providers.sh` — shell-level version of the same checks for ops use

### S4 — RPC layer ✅ done (alloy + circuit breaker → S6)

- [x] `core::rpc::provider` — thin reqwest-backed JSON-RPC client. `Provider::call(method, params)` + typed helpers (`eth_chain_id`, `eth_block_number`, `eth_get_block_header`, `eth_call`). `BlockTag` for cache-key semantics.
- [x] `core::rpc::cache` — `compute_call_hash`, `hash_response_bytes`, `store`, `get`. Idempotent inserts on `rpc_call_cache`.
- [x] `core::rpc::cross_check` — `cross_check_call` (raw bytes; for `eth_call`), `cross_check_block_hash` (extracts `.hash`; for `eth_getBlockByNumber`), `single_call_cached` (archive-only path with cache).
- [x] `verify-rpc` subcommand on seed-migrator. End-to-end live test passed against Chainstack + liveinfraspe:
  - chain_id matches expected (42161) on both
  - block heads delta = 0–1 (within tolerance)
  - block-hash cross-check at head − 32 → providers agree
  - Chainlink ETH/USD `latestRoundData()` cached (archive-only)
  - LPT/WETH pool `slot0()` cached, `observation_cardinality = 601 ≥ 144` ✓
- [x] **Real-world finding** — Chainstack and liveinfraspe disagree on JSON shape (post-Pectra `requestsHash` / post-Shanghai `withdrawals` rendering) even when chain data agrees. SPEC §7.6 / §13.2 amended in v1.5 to make cross-check method-aware: raw bytes for `eth_call`, `.hash` extraction for `eth_getBlockByNumber`, per-log raw bytes for `eth_getLogs`. Divergence row from the original raw-bytes attempt was preserved and marked resolved with notes.
- [ ] alloy integration deferred to S6 (needed when we start ABI-decoding logs; reqwest is sufficient through S5)
- [ ] Circuit breaker + token-bucket rate limit deferred to S6

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
- **2026-04-27** S2 complete. All 14 migrations applied, schema matches SPEC §11. SPEC bumped to v1.4 — §11.5 PK corrected (Postgres rejects function calls in PRIMARY KEY). 8 FKs verified, 2 check constraints verified.
- **2026-04-27** S3 partial. `core::abi` + `seed-abi-registry` + ABI hash verification in `probe`. 7 ABIs registered. Tamper test verified — modifying `abi/Minter.json` triggers `AbiHashMismatch` and halts probe. RPC-side boot checks deferred to S4 since they need `core::rpc`.
- **2026-04-27** S4 done. `core::rpc::{provider,cache,cross_check}` + `verify-rpc` subcommand. End-to-end pass against live Chainstack + liveinfraspe; 4 cache rows written; cardinality 601 verified at head. SPEC bumped to v1.5 — cross-check is method-aware (block hash for headers, raw bytes for eth_call, per-log for eth_getLogs). The cross-check fired on a real provider JSON-shape disagreement before the fix; that divergence row is preserved as a record.
