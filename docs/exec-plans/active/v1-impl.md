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
- [x] Mint / Burn on LivepeerToken (S6.5)
- [x] RoundsManager NewRound (S6.5) — round + blockHash decoded into raw_event.decoded
- [x] Governor ProposalCreated / VoteCast / ProposalExecuted (S6.5) — proposal_id / voter / weight / etc. decoded
- [x] Naming-bridge: SPEC v1.6 corrected `Withdraw` → `Withdrawal`

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

### S7 — reorg + finality watchers (in_progress)

#### S7.1 — reorg watcher ✅ done
- [x] Long-running daemon binary `livepeer-reorg-watcher` with `--once` flag
- [x] Algorithm v1 (events-only walk): for each block_number with rows in `raw_protocol_events` within `[head − 7500, head]` and `finality='tentative' AND is_canonical=TRUE`, fetch chain hash from secondary RPC and compare to stored
- [x] On mismatch: mark `is_canonical=FALSE` on affected rows + insert `reorg_events` audit row, in one transaction
- [x] Cadence picker: 15s normal, 5s heightened (within 5min of last detection), 60s backoff (after 1h clean)
- [x] Severity → log level: INFO ≤ 2 / WARN 3-50 / CRITICAL > 50
- [x] **Live verification:** synthetic divergence test — poisoned a row's `block_hash` to `0xdeadbeef...`, moved it inside the walk window (head − 100), watcher detected `stored=0xdeadbeef vs chain=0x8c6b40f4...`, marked `is_canonical=FALSE`, audit row inserted; happy-path runs returned 0 divergences cleanly
- [ ] Mutation flow (block_number/block_hash update + `reorg_mutations`) deferred to TD-005

#### S7.2 — finality watcher (after S7.1)
- [ ] Two-tier model per SPEC §9.1: `tentative` → `l1_posted` → `finalized`
- [ ] Needs L1 RPC (Ethereum mainnet) — user has `https://ethereum.liveinfraspe.com/...` available (per legacy `config.toml`)
- [ ] Watches Arbitrum's `RollupCore.SequencerBatchDelivered` (or equivalent) on L1 to mark `l1_posted`, then L1 finality confirmations to mark `finalized`

### S8 — valuator (in_progress)

#### S8.1 — seed-hit pricing path ✅ done
- [x] `livepeer-valuator backfill-from-seed [--version V] [--include-tentative]` subcommand
- [x] LEFT JOIN to find unvalued, valuable, canonical events at the requested version (filters multi-asset out via `asset IS NULL` skip)
- [x] Seed lookup by `(chain_id, tx_hash, asset)` against `seeded_event_prices`
- [x] Per Q-OD-1: amount_native re-derived from chain (`raw_protocol_events.amount_normalized`); price + amount_usd computed from seed's `asset_usd_price`
- [x] One Postgres transaction per event: INSERT `event_valuations` (idempotent ON CONFLICT) + INSERT `valuation_attempts`
- [x] `pricing_chain` JSONB carries seed provenance: source = `trusted_historical_seed_v1`, raw seed row preserved, computation re-derivable
- [x] `--include-tentative` development flag (SPEC §9.1 says only finalized in prod; without finality watcher, all rows are tentative)
- [x] **Live verification:** 155 candidates → 112 priced / 31 seed-misses (Bond/Unbond/etc. → S8.2) / 12 multi-asset skipped (EarningsClaimed → S8.3). Reward 3952 LPT × $2.191 = $8,659.29 ✓; WinningTicketRedeemed 0.0024 ETH × $2,390.93 = $5.75 ✓. 112 audit rows in `valuation_attempts`. Re-run priced 0 — idempotent.

#### S8.2.a — on-chain pricing for ETH events (Chainlink) ✅ done
- [x] `livepeer-valuator backfill-eth-onchain` subcommand
- [x] Sequencer-uptime read at event block (§7.3.4); answer != 0 → `failed_sequencer_outage`
- [x] Chainlink `latestRoundData()` at event block via `cross_check::single_call_cached` — cached forever per SPEC §13.5
- [x] Mandatory checks: `answeredInRound >= roundId`, staleness ≤ 86400s; WARN at > 14400s
- [x] `pricing_chain` JSONB with full provenance (oracle address, raw_round, checks block, result)
- [x] Status routing: `priced` / `failed_sequencer_outage` / `failed_missing_oracle` (rows in `valuation_attempts` regardless)
- [x] Refactored `persist.rs` so seed.rs and onchain.rs share `insert_valuation` / `insert_attempt`
- [x] **Live verification:** 6 ETH candidates → 6 priced. WithdrawFees 0.184 ETH × $2,390.62 = $439.87 ✓. Tiny-amount precision check: 0.00000517... ETH × $2378.51 = $0.01230... ✓. Idempotent.

#### S8.2.b — on-chain pricing for LPT events (Uniswap V3 TWAP × Chainlink) ✅ done
- [x] `tick_math::get_sqrt_ratio_at_tick(tick) -> U256` — Uniswap V3 TickMath in Rust, deterministic integer math. 6 unit tests covering tick=0, ±MIN_TICK/MAX_TICK, range errors, signed sqrtPriceX96 correlation with tick sign, magnitude check at known tick.
- [x] `read_pool_slot0(pool, block)` — sqrtPriceX96 + tick + observationCardinality via `single_call_cached`
- [x] `read_pool_observe(pool, block, 1800)` — cumulative ticks for [1800, 0] via `single_call_cached`
- [x] `uniswap_arithmetic_mean_tick(delta, secs)` — floor-division semantics matching Uniswap's OracleLibrary (subtract 1 when delta < 0 and not evenly divisible)
- [x] **Default TWAP path:** observe → mean tick → TickMath.getSqrtRatioAtTick → square / 2^192 → LPT/WETH → × Chainlink ETH/USD → LPT/USD
- [x] **Degraded path:** slot0().sqrtPriceX96 → square / 2^192 → spot LPT/WETH → × Chainlink ETH/USD → LPT/USD; version stamped `v1_degraded_spot_pre_cardinality`
- [x] Cardinality precheck (`< 144` → degraded)
- [x] Token0/token1 ordering: LPT < WETH lexicographically, both 18 decimals → price = WETH per LPT, no decimal correction
- [x] Pool-not-yet-deployed handling (slot0 returns empty bytes) → `failed_missing_pool`
- [x] Pool can't serve TWAP window (observe reverts with OLD) → `failed_missing_pool`
- [x] **Live verification (TWAP path):** 25 LPT events priced. Sample: avg tick -69945 → sqrtPriceX96 = 2399491... → LPT/WETH = 0.0009172... × $2394.51 = LPT/USD $2.196 → 326.35 LPT × $2.196 = $716.76 ✓. Idempotent.
- [ ] Live verification of degraded path requires fetching pre-cardinality events (none in current DB; deferred to S11 fixture work). Q-OD-9 says ~17,032 events in `[genesis, ~32M]` will hit it.

#### TD-006 (added) — degraded version derivation
- [ ] `DEGRADED_VERSION_SUFFIX` is currently appended to a hardcoded "v1" prefix. When v2 valuation versions land, derive the prefix from the operator-passed `--version`.

#### S8.3 — multi-asset (EarningsClaimed) — generates 2 valuation rows per event per SPEC §6.8

#### S8.4 — determinism guard (§10.5)
- [ ] On `event_valuations` PK conflict where computed values differ from stored — log CRITICAL alert + write `valuation_attempts` row with `result_status='failed_determinism_violation'` and full diff

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
- **2026-04-27** S6.5 done — RoundsManager.NewRound, Governor (proposals/votes), LivepeerToken Mint/Burn. 15 distinct event types now decoding live. S6 substantively complete.
- **2026-04-27** S7.1 done — reorg-watcher daemon with cadence picker (15s/5s/60s), events-only walk in `[head − 7500, head]`. Synthetic divergence test (poisoned `0xdeadbeef` block hash) verified end-to-end: detected, `is_canonical=FALSE` set, audit row written. Mutation flow (`block_number`/`block_hash` update + `reorg_mutations`) deferred to TD-005.
- **2026-04-27** S8.1 done — valuator's seed-hit path. 112 events priced from seed in one pass; 31 seed-misses + 12 multi-asset deferred to subsequent slices. Q-OD-1 mitigation in action: chain provides `amount_native`, seed provides `asset_usd_price`, valuator multiplies them to get `amount_usd`. Reward 3952 LPT × $2.191 = $8,659.29 verified end-to-end.
- **2026-04-27** S8.2.a done — on-chain Chainlink ETH/USD path. 6 ETH events priced. Sequencer + Chainlink reads cached forever per SPEC §13.5 (12 cache rows added; replay reads from cache). Sample: WithdrawFees 0.184 ETH × $2,390.62 = $439.87 ✓. Sub-microETH amount priced to 18-decimal precision. pricing_chain JSONB carries full provenance (oracle address, raw_round, checks block). Idempotent.
- **2026-04-27** S8.2.b done — Uniswap V3 TWAP × Chainlink for LPT events. tick_math::get_sqrt_ratio_at_tick implemented as deterministic integer math (Uniswap reference algorithm), 6 unit tests pass. Sample: avg tick -69945 → sqrtPriceX96 = 2399491... → LPT/WETH = 0.0009172 × $2394.51 = $2.196 → 326.35 LPT = $716.76 ✓. 25 LPT events priced. Pricing chain provenance carries cumulative ticks, mean tick, sqrtPriceX96, oracle, raw_round. Total event_valuations coverage: 143 across 3 sources (seed 112 + Chainlink ETH 6 + Uniswap LPT 25). Idempotent.
