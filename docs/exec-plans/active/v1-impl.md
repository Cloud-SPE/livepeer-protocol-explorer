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

#### S7.2 — finality watcher ✅ done (timestamp-heuristic; SPEC-true batch tracking → TD-008)
- [x] `livepeer-finality-watcher` daemon with `--once` flag
- [x] Each iteration reads L1 `latest` and `finalized` block timestamps via `eth_getBlockByNumber`
- [x] Marks `tentative → l1_posted` when L2 block_timestamp ≤ `latest_l1_ts − 600s` (~10 min posting lag per SPEC §9.1)
- [x] Marks `(tentative|l1_posted) → finalized` (with `finalized_at = now()`) when L2 block_timestamp ≤ `finalized_l1_ts − 60s` (1 min safety margin)
- [x] Cadence: 60s (L1 advances slowly)
- [x] Updates `indexer_checkpoints('finality_watcher')` for observability
- [x] Doesn't need L1 archive depth — only reads block tags
- [x] **Live verification:** L1 latest=1777299251 / finalized=1777298123 (~19 min apart). 165 events promoted tentative → finalized in one iteration. Valuator without `--include-tentative` now correctly processes them.

#### TD-008 (added) — SPEC-true batch tracking
- [ ] Watch Arbitrum's `SequencerInbox.SequencerBatchDelivered` on Ethereum L1
- [ ] Decode batches → map to L2 block ranges → mark events `l1_posted` precisely (not by 10-min heuristic)
- [ ] L1 finality of each batch tx → marks events `finalized`
- [ ] Requires L1 archive depth for back-fill (Chainstack Eth L1 archive — user provides URL)
- [ ] liveinfra L1 prunes; useful as the secondary cross-check provider but not for back-fill

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
- [x] Status routing: `priced` / `failed_sequencer_outage` / `failed_missing_oracle`; terminal failures are recorded in `valuation_attempts` and now also persisted as `event_valuations` outcome rows with nullable USD fields
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

#### S8.3 — multi-asset (EarningsClaimed) ✅ done
- [x] `multi_asset.rs` module — fetches unvalued EarningsClaimed events (raw `asset IS NULL`, `decoded.{rewards, fees}` in JSONB)
- [x] Refactored `price_eth_event` and `price_lpt_event` into pure helpers `price_eth_amount(block, ts, amount)` and `price_lpt_amount(...)` so the multi-asset pass reuses them without constructing synthetic CandidateEvents
- [x] Per event: split into LPT (rewards) + ETH (fees) BigDecimal amounts; price each via existing helpers; write 2 `event_valuations` rows
- [x] Zero-amount portions get `pricing_method='no_amount'` rows so the pair (LPT + ETH) is always complete per SPEC §6.8
- [x] **Live verification:** 12 EarningsClaimed → 24 valuation rows. event 30: ETH 0.184 × $2390.62 = $439.87 + LPT 0.000114 × $2.192 = $0.00025 ✓; event 115: LPT 9.436 × $2.181 = $20.59 + ETH 0.0000051 × $2378.51 = $0.0123 ✓. Idempotent.

#### S8.4 — determinism guard (§10.5) ✅ scaffold (verify-mode CLI → S8.5)
- [x] `persist::insert_valuation_checked` — SELECTs existing row by PK; on hit, computes diff against the four numeric columns + 3 string columns; on diff fires `error!` log + inserts `valuation_attempts` row with `result_status='failed_determinism_violation'` and JSON diff in `error_detail`; preserves the stored row (no overwrite)
- [x] `DeterminismOutcome::{Inserted, Idempotent, Violation}` returned to callers
- [x] `StoredValuation::diff` reports per-column diffs
- [x] **3 unit tests pass:** match→null, single-column mismatch, multi-column mismatch
- [ ] Verify-mode CLI that re-runs all priced events through the checked path — defer to S8.5; the helper is ready, the routine integration just needs a `--verify` subcommand. Useful only when valuator logic changes without a version bump (a thing the spec says shouldn't happen but the guard exists for that exact case).

### S9 — staker (in_progress)

#### S9.1 — flow-derived principal ✅ done
- [x] `livepeer-staker backfill` walks Bond / Unbond / Rebond / WithdrawStake / EarningsClaimed / TransferBond events in `(block_number, log_index)` order, maintains an in-memory per-delegator running balance, writes one `stake_balances_by_block` row per affected delegator after each event
- [x] `delegator_registry` populated as a side effect — Bond is the canonical first-event; TransferBond auto-registers the receiving side too
- [x] EarningsClaimed reconciliation: rewards portion compounds into the delegator's `bonded_principal`
- [x] TransferBond writes 2 rows per event (one per affected delegator)
- [x] Idempotent: ON CONFLICT (chain_id, delegator_address, block_number) DO UPDATE; same input → same output
- [x] Skipped 22 events on delegators whose Bond is outside the 30K-block test window (full-genesis backfill would not skip)
- [x] **Live verification:** 3 delegators registered, 3 rows persisted (61.91 / 20.97 / 0.9 LPT) — re-run leaves DB row count unchanged

#### S9.2 — pending stake / fees via RPC ✅ done
- [x] BondingManager.pendingStake(delegator) + pendingFees(delegator, endRound) at event block via deterministic cached RPC reads
- [x] Update source = 'pending_call' or 'both' per SPEC §11.10
- [x] Triggered on EarningsClaimed (canonical reconciliation point per SPEC §6.8)
- [x] Bulk refresh implementation shipped in [crates/livepeer-staker/src/pending.rs](../../../crates/livepeer-staker/src/pending.rs) — cache prefetch + bulk `UPDATE … FROM unnest(...)`
- [x] Exact-state reconcile shipped before pending refresh: `getDelegator()` overwrites flow-derived `bonded_principal` / `delegate_address` with contract truth at each stored stake row block

### S10 — API (in_progress)

#### S10.1 — events + valuations + operational ✅ done
- [x] Axum 0.8 service binding `0.0.0.0:8080`
- [x] `GET /health` — liveness
- [x] `GET /backfills/status` — checkpoints + counts (raw_events, valuations, decode_failures, reorg_events)
- [x] `GET /events/{id}` + optional `?with_valuations=true`
- [x] `GET /events?...` — filters: `from_block`, `to_block`, `contract`, `event_name`, `event_type` (legacy alias), `from_address`, `to_address`, `address` (any-role), `asset`, `with_valuations`, `include_tentative`, `include_reorged`, `sort` (whitelist: `block_asc` / `block_desc`), `limit` (default 100, max 1000), `cursor`
- [x] Cursor pagination — opaque `B<block>:<log_index>` token, tuple-comparison SQL (`(block_number, log_index) > (cur_block, cur_log)`) for stable order under append
- [x] `GET /events/{id}/valuation` — list of valuations for the event (multi-asset returns both rows)
- [x] `GET /valuations?...` — filters: `version`, `asset`, `from_block`, `to_block`, `limit`
- [x] All large numerics serialized as strings per SPEC §14.4
- [x] Standard error envelope `{ "error": { "code", "message", "context" } }`
- [x] **Live verification:** Reward 533.21 LPT × $2.191 = $1,168.28 with inline valuations ✓; cursor pagination produces stable strict-after pages ✓; multi-asset EarningsClaimed returns LPT + ETH portions ✓; 404 envelope ✓

#### S10.2 — aggregations + governance + prices ✅ done (stake + ETag → S10.3)
- [x] `GET /aggregations/events` — bucket = day | week | month; metric = count | sum_amount_native | sum_amount_usd | avg_amount_usd; tz default UTC; from/to accept ISO date or block number; LEFT JOIN to `event_valuations` only when metric needs it. **Live verified:** 94 Rewards summing $60,890.17 in one day; 5 WinningTicketRedeemed in one day; total $65,246.65 across 155 valuable events.
- [x] `GET /governance/proposals[?status=executed|not_executed|active|all]` — joins `ProposalCreated` + `ProposalExecuted` + per-proposal `VoteCast` tally (against / for / abstain weight, vote count). Empty result confirmed for our test range (no governance activity).
- [x] `GET /governance/proposals/{proposal_id}` — single proposal fetch.
- [x] `GET /prices/{asset}/{quote}/block/{block}` and `/latest` — backed by `token_prices_by_block`, populated by the valuator's on-chain reads (Chainlink ETH/USD; Uniswap V3 TWAP for LPT/WETH; LPT/USD as the derived product). Live verified: `/prices/LPT/USD/latest` serves $2.194 with pool + oracle addresses.
- [x] Validation: rejects `bucket=hour`, `metric=median`, etc. with conformant 400 + error envelope.

#### S10.3 — stake + ETag + range prices + sort=amount_usd_desc (partial)
- [x] `GET /stake/{delegator}/...`
- [ ] `If-None-Match` / `ETag` / `Cache-Control` per SPEC §14.2
- [x] `GET /prices/{asset}/{quote}/range` with lazy backfill
- [x] `sort=amount_usd_desc` on `/events`

#### S10.4 — transcoder context endpoints ✅ done
- [x] `GET /transcoders/{transcoder}/params/latest`
- [x] `GET /transcoders/{transcoder}/params/block/{block}`
- [x] `GET /transcoders/{transcoder}/params/history`
- [x] `GET /transcoders/{transcoder}/lifecycle/latest`
- [x] `GET /transcoders/{transcoder}/lifecycle/block/{block}`
- [x] `GET /transcoders/{transcoder}/lifecycle/history`
- [x] `GET /transcoders/{transcoder}/profile/block/{block}`
- [x] `GET /transcoders/{transcoder}/delegators/block/{block}`
- [x] Query-path indexes landed in migration `021_api_transcoder_indexes`

### S11 — cross-check + determinism CI (in_progress)

#### S11.1 — cross-check binary ✅ done
- [x] `livepeer-seed-migrator cross-check --source-sqlite <path>` walks the (tx_hash, log_index) intersection of indexer + seed within the indexed window
- [x] Reports matched / missing-in-indexer / missing-in-seed / block-number-mismatches / block-hash-mismatches with up to 10 sample tx-hash#log-index strings per class
- [x] **Live verification on 30K-block window:** 165 indexer ↔ 136 seed events: 131 matched, 4 missing-in-indexer, 33 missing-in-seed, 1 block-hash-mismatch (the synthetic-divergence row from S7.1, correctly surfaced)

#### S11.2 — determinism replay CI gate (partial)
- [x] Committed strict-replay fixtures under `tests/fixtures/<case>/` — split across small real-chain cases so CI stays practical while still covering the SPEC §12.4 surface (Reward, EarningsClaimed multi-asset, Bond, Unbond, WinningTicketRedeemed, NewRound, TranscoderUpdate, seeded + non-seeded across the case set)
- [x] Script-driven replay gate: `scripts/run-determinism-replay.sh` drops into a clean DB, applies migrations, loads fixture cache/checkpoints, runs strict `livepeer-orchestrator replay --skip-cross-check`, hashes the derived tables by stable row order, and diffs against each case's `expected_hashes.json`
- [x] Flip `.github/workflows/determinism.yml` from placeholder to the real test
- [ ] `livepeer-test regenerate-fixture` operator command

### S12 — observability + alerting (in_progress)

#### S12.1 — Prometheus metrics on API ✅ done
- [x] `livepeer-api/src/metrics.rs` builds a Registry + IntCounterVec(route, status)
- [x] `GET /metrics` exposes standard Prometheus exposition format
- [x] `/health` increments the counter — verified live: `api_requests_total{route="/health",status="2xx"} 3` after 3 requests

#### S12.2 — daemon-side metrics + Telegram alerter (partial)
- [x] `livepeer-daemon` now exposes `/health` and `/metrics` on a dedicated bind (`--metrics-bind`, default `0.0.0.0:9107`)
- [x] Core daemon metrics now live in Prometheus format:
  - `livepeer_iterations_total{task}`
  - `livepeer_iteration_failures_total{task,error_kind}`
  - `livepeer_iteration_duration_seconds{task}`
  - `livepeer_chain_head_block`
  - `livepeer_task_checkpoint_block{task}`
  - `livepeer_task_lag_blocks{task}`
- [x] Partial event/outcome counters now emitted:
  - `livepeer_events_indexed_total{contract}`
  - `livepeer_decode_failures_total{contract}`
  - `livepeer_events_valued_total{status}`
  - `livepeer_reorgs_detected_total{severity}`
- [x] Core RPC metrics now emitted through daemon `/metrics`:
  - `livepeer_rpc_calls_total{provider,method,result}`
  - `livepeer_rpc_call_duration_seconds{provider,method}`
  - `livepeer_rpc_divergence_total{method}`
- [ ] Additional lag / circuit gauges (`valuator_pending_events`, `rpc_provider_circuit_state`)
- [ ] Telegram alerter — last priority per SPEC §10.6, behind a feature flag

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
- **2026-04-27** S8.3 + S8.4 scaffolding done. Multi-asset: 12 EarningsClaimed → 24 rows (12 LPT priced via TWAP, 4 ETH via Chainlink, 8 ETH zero-amount). Total event_valuations coverage now: 167 across 4 sources. Determinism guard `insert_valuation_checked` + diff helper ready with 3 unit tests; verify-mode CLI integration deferred to S8.5. 9/9 unit tests pass.
- **2026-04-27** S10.1 done — Axum API with /health, /backfills/status, /events (with cursor pagination + inline valuations), /events/{id}, /events/{id}/valuation, /valuations. Live tests pass: Reward 533.21 LPT × $2.191 = $1,168.28 returned with inline valuations; cursor pagination stable; multi-asset EarningsClaimed returns both LPT + ETH portions; 404 error envelope conforms to SPEC §14.4. Numerics serialized as strings.
- **2026-04-27** S10.2 done — /aggregations/events (94 Rewards = $60,890.17/day verified), /governance/proposals + tally, /prices/{asset}/{quote}/{block,latest}. TD-007 added — token_prices_by_block isn't being written by the valuator yet; rpc_call_cache holds the data instead. Validation envelope confirmed for bad bucket/metric inputs.
- **2026-04-27** S9.1 + stake API done. Flow-derived stake: 3 delegators registered, 3 rows persisted (61.91/20.97/0.9 LPT). 22 events skipped on out-of-window delegators (full-genesis backfill resolves). API: GET /stake/{del}/block/{N} returns at-or-before snapshot with staleness_blocks; /range filters by from/to. /stake/.../block/<pre-bond> returns 404 cleanly. Pending-stake-via-RPC defers to S9.2.
- **2026-04-27** S7.2 done — finality-watcher daemon. Heuristic v1.5: each iteration reads L1 latest + finalized block timestamps, marks L2 events accordingly (10-min posting lag, 1-min margin past L1 finalized). One iteration promoted all 165 test-DB events from tentative → finalized. Valuator now works in production mode (no `--include-tentative`). SPEC-true SequencerBatchDelivered tracking → TD-008 (Chainstack L1 archive URL provided + .env updated).
- **2026-04-27** TD-007 done — valuator now writes token_prices_by_block alongside event_valuations. /prices/LPT/USD/latest serves $2.194 with pool + oracle addresses. /prices/.../range and sort=amount_usd_desc on /events also live (S10.3 partial).
- **2026-04-27** S9.2 done — staker.refresh-pending walks EarningsClaimed events, eth_calls BondingManager.pendingStake / pendingFees at the event block, updates stake rows with source='both'. Sample: delegator with 61.91 LPT bonded_principal in our window has 40,856 LPT pendingStake on chain (full history).
- **2026-04-27** S11.1 done — cross-check binary surfaces real divergences. 131 matched / 4 missing-in-indexer / 33 missing-in-seed / 1 block-hash-mismatch (the S7.1 synthetic-divergence row).
- **2026-04-27** S12.1 done — /metrics endpoint on API binary, Prometheus exposition, IntCounterVec(route, status), /health increments and serves cleanly.
- **2026-05-04** S12.2 partial — `livepeer-daemon` now exists with `follow` mode, shared RPC handles, coordinated shutdown, and Prometheus `/metrics` + `/health` on a dedicated bind. Current metric set covers iteration success/failure/duration plus head/checkpoint/lag gauges; alerting and richer per-event RPC metrics remain open.
- **2026-05-04** S12.2 follow-up — daemon now enforces a process-wide RPC concurrency ceiling (`24`) inside `core::rpc::Provider`, so the shared-provider design has a real in-process request budget rather than relying on topology alone.
- **2026-05-04** S12.2 follow-up — daemon metrics now also emit indexed/decode/valuation/reorg counters from live worker summaries, so the metrics surface is operationally useful without scraping logs for row counts.
- **2026-05-04** S12.2 follow-up — provider-level RPC metrics now live in `core::rpc` and are exported through daemon `/metrics`: per-provider/method call counts, duration histograms, and divergence counters.
- **2026-05-04** TD-012 Phase 1 tightened — `livepeer-orchestrator replay` is now strict by default: it requires explicit `--to-block`, routes indexer `eth_getLogs` through `rpc_call_cache`, records finality replay inputs during live runs, and fails on missing cached RPC inputs unless `--allow-live-rpc` is explicitly requested.
- **2026-05-03** S9.2 strengthened — pending refresh is now bulk and exact-state-aware. `refresh-pending` first reconciles every stored stake row against `BondingManager.getDelegator()` and then bulk-refreshes `pendingStake` / `pendingFees` for every `EarningsClaimed` row from cached RPC inputs. Fresh rerun validation: `no_stake_row = 0`, self-delegated orchestrator spot-checks match on-chain `bondedAmount`.
- **2026-05-04** S10.4 done — transcoder context API shipped: params/history, lifecycle/history, point-in-time profile, and delegators-at-block. Migration `021_api_transcoder_indexes` added the required raw-event expression index plus stake covering index; measured route latencies dropped to ~1ms for params/lifecycle/profile and ~42ms for delegators-at-block.
