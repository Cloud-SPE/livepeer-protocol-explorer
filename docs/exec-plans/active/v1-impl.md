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

### S5 — seed migrator (real work) ✅ done (events.payload staging → S11)

- [x] `livepeer-seed-migrator import` — real SQLite read + Postgres write
- [x] 297,105 payouts → `seeded_event_prices(asset='ETH', event_type_hint='payout')`
- [x] 158,448 rewards → `seeded_event_prices(asset='LPT', event_type_hint='reward')`
- [x] Per Q-OD-2: `log_index = -1` sentinel; PK `(chain_id, tx_hash, log_index, asset)`
- [x] Per Q-OD-1: SQLite f64 → BigDecimal::from_f64; valuator re-derives `amount_native` from RPC at valuation time
- [x] Idempotent: re-run inserts 0, totals unchanged at 297,105 / 158,448
- [x] CAST AS REAL on every numeric column to normalize SQLite's INTEGER-affinity-on-whole-numbers (sqlx rejects INT↔f64 decode)
- [x] Spot-checked: payout has ETH price ≈ $2390, reward has LPT price ≈ $5.24 — sane
- [x] **Performance:** 24.7s for 455K rows in batches of 1000 (one PG transaction per table)
- [ ] events.payload staging table + import → S11 (cross-check pass; uses TD-004 plumbing)

### S6 — indexer (in_progress)

#### S6.1 ✅ end-to-end vertical slice for Reward
- [x] alloy added to `livepeer-indexer` deps
- [x] `events.rs` — `sol!` macro for `Reward(address indexed transcoder, uint256 amount)`
- [x] `core::rpc::Provider::eth_get_logs` added
- [x] `backfill.rs` — fetch via `eth_getLogs` → decode via alloy → fetch+cache block timestamps via `single_call_cached` → batch INSERT into `raw_protocol_events` + advance `indexer_checkpoints` in one atomic transaction
- [x] **Live test results:** 4 Rewards across blocks 456735816–456740385 captured cleanly. Sample amount `18.199267584391068228` matches the seed `18.19926758439107` to 14 sig figs, confirming Q-OD-1 precision-loss finding empirically.
- [x] **Idempotency:** re-run inserts 0; ON CONFLICT DO NOTHING on `(chain_id, tx_hash, log_index)` works as designed.
- [x] **Real-data finding:** SQLite `events` table has duplicate rows for some on-chain logs (block 456740385: 2 rows / 1 log). Recorded in TD-004 — cross-check pass must dedupe.

#### S6.2 — remaining event types ✅ for the 3 high-volume contracts (RoundsManager + Governor → S6.5)

- [x] `events.rs` uses `sol!(Contract, "../../abi/X.json")` JSON-path mode — auto-generates Rust types for every event in each ABI. No hand-typed signatures.
- [x] `backfill.rs` generalized: `ContractKind` enum, topic0 dispatch, `eth_getLogs` with multi-topic0 filter (`topics: [[t1, t2, ...]]`)
- [x] BondingManager: Bond, Unbond, Rebond, WithdrawStake, TransferBond, EarningsClaimed (multi-asset → asset=NULL + decoded JSON), WithdrawFees (overload `_0` — has amount), Reward (S6.1), TranscoderActivated/Deactivated/Update (non-monetary)
- [x] TicketBroker: WinningTicketRedeemed, WinningTicketTransfer, DepositFunded, ReserveFunded, Withdrawal (corrected from "Withdraw"), Unlock (non-monetary)
- [x] LivepeerToken: Transfer, Approval (non-monetary)
- [x] **Live test:** 51 events across 13 distinct types in `[456735000, 456741000]` window. Sample correctness: EarningsClaimed multi-asset breakdown landed in `raw_event.decoded`; Transfer mint pattern (0x0→Minter→recipient) captured; WinningTicketRedeemed sender resolved to known Livepeer Inc address.
- [x] **Q-OD-1 verified live again:** Reward amount 7.529685595729787584 LPT matches seed `7.52968559572978800000` to 16 sig figs.
- [x] Idempotent on all 3 contracts (re-run `inserted=0` × 3).
- [ ] Mint / Burn on LivepeerToken — S6.5 if needed; Transfer-from-zero already captures mints semantically
- [ ] RoundsManager NewRound — S6.5
- [ ] Governor ProposalCreated / VoteCast / ProposalExecuted — S6.5
- [ ] Naming-bridge update needed: SPEC §6.4 listed `Withdraw`, real ABI emits `Withdrawal`. Spec amendment pending.

#### S6.3 + S6.4 — strict-decode routing + chunked driver ✅ done

- [x] `DispatchOutcome` enum: `Decoded` / `DecodeFailed { is_strict, … }` / `UnknownTopic0`
- [x] `is_strict_event(contract, topic0)` — encodes SPEC §6.2 v1.6 critical-events allowlist statically
- [x] On strict-event decode failure → entire chunk transaction aborts (no events committed, no dead-letters written, no checkpoint advance) per §10.2.1
- [x] On non-strict decode failure or unknown topic0 → row in `decode_failures` per §10.2.2; chunk still commits
- [x] `drive_backfill` walks `[from, to]` in chunks of `current_batch_size`. Starts at 5,000, doubles on success up to 10,000 (cap), halves to a 100-block floor on transient HTTP errors and retries the same range
- [x] `resume_from(pg, requested_from)` reads `indexer_checkpoints('main')` and clamps the start to checkpoint+1 if past requested
- [x] `--no-resume` CLI flag forces start at `--from-block`
- [x] Per-chunk: ONE Postgres transaction wraps `INSERT raw_protocol_events + INSERT decode_failures + UPDATE indexer_checkpoints` so the chunk is atomic
- [x] **Live verification on `[456720000, 456750000]` (30K blocks):**
  - 4 chunks committed (5K → 10K → 10K → 5K — dynamic doubling visible)
  - 125 logs seen, 113 events newly inserted (12 collisions from prior runs, idempotent)
  - 0 dead-letters, 0 strict failures, 0 unresolved divergences
  - Checkpoint advanced to 456,750,000
  - Resume run: "checkpoint already past target — nothing to do" early-exit ✓
  - `--no-resume` re-fetches all 125 logs, inserts 0 (idempotent) ✓
  - `final_batch_size = 10000` (capped) ✓
- [ ] Failure-path runtime testing (planted strict failure / planted dead-letter) — defer until determinism fixture (S11) lands; current happy-path proves the routing wiring; the failure code paths are small + reviewable
- [ ] `recover-decode-failures` subcommand — defer to v1.5 (operator tool, not core flow)
- [ ] Cross-check sampling on backfill — defer to S6.5

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
- **2026-04-27** S5 done. Real seed import: 297K payouts + 158K rewards = 455,553 rows in 24.7s. Idempotent (`inserted_this_run = 0` on second run). Required CAST AS REAL on every numeric column to handle SQLite's per-row INTEGER affinity for whole-number values. Sample rows verified sane (ETH @ $2390, LPT @ $5.24).
- **2026-04-27** S6.1 done. End-to-end indexer slice for Reward — alloy in, sol! macro, eth_getLogs + decode + atomic insert + checkpoint. 4 Rewards captured cleanly. Q-OD-1 precision-loss verified empirically against the seed. Idempotent. SQLite-events duplicate finding noted in TD-004.
- **2026-04-27** S6.2 done for BondingManager + TicketBroker + LivepeerToken (high-volume contracts). 13 distinct event types decoding cleanly via alloy `sol!(Contract, "abi/X.json")` JSON-path mode. 51 events landed in a 6,000-block window, idempotent across all 3 contracts. Real-data findings: SPEC §6.4 named the event `Withdraw` but ABI says `Withdrawal`; BondingManager has two `WithdrawFees` overloads (we use the one with `(delegator, recipient, amount)`). RoundsManager + Governor events deferred to S6.5.
- **2026-04-27** S6.3 + S6.4 done. Driver walks `[from, to]` in dynamic-sized chunks (5K start, doubles to 10K on success, halves to 100 floor on transient errors). Strict-decode allowlist halts the chunk transaction; non-strict failures write to `decode_failures`. Checkpoint resume + `--no-resume` flag verified. 30K-block run did 4 chunks / 125 logs / 113 events / 0 dead-letters cleanly.
