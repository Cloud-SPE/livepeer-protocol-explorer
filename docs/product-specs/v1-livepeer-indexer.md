# Livepeer Protocol Event Indexing & Exact Historical Valuation System

## Technical Specification v1.9

**Status:** Living spec for the implemented v1 system
**Target chain:** Arbitrum One (chain_id 42161)
**Primary asset:** Livepeer Token (LPT)
**Secondary asset:** Ethereum (ETH)
**Document version:** 1.9

### Changes since v1.8 (2026-05-05)

- §14 — v1 reintroduces the legacy parity surfaces previously deferred from scope: CSV report endpoints, orchestrator/gateway metadata endpoints, and the `job_type` (`ai`/`transcoding`) filter via the TD-017 overlay + rollup design.
- §14.3 — added the endpoint families reinstated by TD-017: payout/reward leaderboards and period summaries, ticket timeseries, CSV exports, entity profile lists, and governance vote history.
- TD-017 Phase 0 also clarifies transcoder fee semantics: APIs must expose both protocol-perspective `fee_share_percent` and operator-perspective `fee_cut_percent` to avoid silent old/new parity drift.

### Changes since v1.7 (2026-05-05)

- §14 — API surface updated to match the shipped service. v1 no longer promises conditional GET / `ETag` / `If-None-Match`; the actual poll model is plain JSON polling. The spec now documents the shipped endpoint families for transcoders and gateways plus the machine-readable `/openapi.json` and interactive `/docs` surfaces.
- §15 / §16 — deployment and configuration sections updated to reflect the current runtime shape: `livepeer-daemon follow`, `livepeer-api`, optional `livepeer-alert-bot`, one-shot `livepeer-orchestrator` / `livepeer-seed-migrator`, and API metrics served on `:8080` rather than a dedicated `:9106`.
- §24 / Appendix — acceptance criteria and design-decision log updated so they no longer require `ETag`, and they reflect the shipped standalone API service rather than the earlier “existing API bolt-on” assumption.

### Changes since v1.6 (2026-05-03)

- §1 / §10 / §11 / §14 / §24 — `event_valuations` is now the canonical **valuation outcome** table, not a success-only table. Terminal outcomes (`failed_missing_pool`, `failed_missing_oracle`, `failed_sequencer_outage`) also get immutable rows there, with nullable `native_usd_price` / `amount_usd` and full `pricing_chain` detail. This aligns the spec with the shipped Option A implementation and migration `017_event_valuations_terminal_failures`.
- §24.1 — acceptance criteria clarified for the two-version LPT model: completion means every finalized, valuable event has an `event_valuations` row under the applicable valuation version (`v1_lpt_weth_twap_30min_x_chainlink_eth` or `v1_degraded_spot_pre_cardinality`), not only the primary version.

### Changes since v1.5 (2026-04-27)

- §6.4 — TicketBroker withdrawal event renamed `Withdraw` → `Withdrawal` (the actual on-chain event name; v1.0–v1.5 had it wrong). The amount is `deposit + reserve` since the event drains both at once. Verified by `sol!(TicketBroker, "abi/TicketBroker.json")` and confirmed against the SQLite seed which also uses `Withdrawal`.
- §6.3 — `WithdrawFees` clarified: BondingManager has two overloads. The current form is `WithdrawFees(address indexed delegator, address recipient, uint256 amount)`. The legacy single-arg form `WithdrawFees(address indexed delegator)` exists in the ABI but is not emitted in v1; if encountered it has no amount field.

### Changes since v1.4 (2026-04-27)

- §7.6 / §13.2 — cross-check is **method-aware**, not unconditionally raw-bytes. Verified empirically: Chainstack and liveinfraspe agree on chain data but disagree on JSON shape for `eth_getBlockByNumber` (Chainstack emits `requestsHash`/`withdrawals` as `null`; liveinfraspe omits them). The load-bearing invariant from §9.2 is "block N has hash H" — `cross_check_block_hash` extracts `.hash` from each response and compares. `eth_call` results are hex-blob strings with no provider rendering choices, so raw-bytes compare remains correct there.

### Changes since v1.3 (2026-04-27)

- §11.5 — `seeded_event_prices.log_index` corrected: was `INT` nullable with `PRIMARY KEY (..., COALESCE(log_index, -1), ...)`. Postgres rejects function calls in PRIMARY KEY syntax. Replaced with `INT NOT NULL DEFAULT -1` and a plain PK. Same semantics; valid SQL. Verified by applying migration 004 against Postgres 15.

### Changes since v1.2 (2026-04-27)

- §13.1 — provider topology corrected: there is no self-hosted Nitro node. The secondary is **liveinfraspe** — a hosted HTTP RPC, non-archive. The routing matrix in §13.2 is unchanged (same logical roles), only the physical provider name changed. Resolves Q-OD-5.
- §7.3.2 — added the verified historical impact: ~17,032 monetary events in the cardinality-degraded window `[genesis, ~33M]` are not in the seed and must take a degraded valuation version. v1 implementation must build the degraded path, not stub it. Resolves Q-OD-9.
- Resolved (no spec change beyond §7.3.2 / §13.1): Q-OD-8 (Chainlink ETH/USD aggregator on Arbitrum = `0x639Fe6ab55C921f74e7fac1ee960C0B6293ba612`), Q-OD-10 (L2 Sequencer Uptime Feed = `0xFdB631F5EE196F0ed6FAa767959853A9F217697D`). Addresses persisted in `config/arbitrum.yaml`. Verification details in `docs/design-docs/on-chain-references.md`.
- `config/arbitrum.yaml` — corrected `livepeer_arbitrum_genesis_block` from `6738090` to `6072093` (off by ~666K blocks; verified from earliest event in the SQLite seed).

### Changes since v1.1 (2026-04-27)

- §8 — `block_cursors` SQLite table dropped from the seed-migration scope. The valuator does a flat `(chain_id, tx_hash, asset)` lookup against `seeded_event_prices`; no per-event-type bound vector. Simpler and equivalent: a hit means the seed has it, a miss means on-chain pricing.
- §11.2 — removed the `seed_<event_type>` checkpoint names; only `'main'`, `'reorg_watcher'`, `'finality_watcher'`, `'valuator_v1'`, `'staker'` remain.
- §14.3.1 — Events endpoint augmented: cursor-based pagination (opaque cursor, stable under append), sort whitelist, dual-address filter (`from_address` / `to_address` / `address`), `with_valuations=true` join, `asset`, `contract`, `event_name`.
- §14.3.6 — NEW `/aggregations/events` endpoint covering daily/weekly/monthly USD totals + ticket-count timeseries. Replaces 4 legacy summary routes.
- §14.3.7 — NEW `/governance/proposals` convenience endpoint joining `ProposalCreated` + `ProposalExecuted` + `VoteCast` aggregates.
- §14 — CSV report endpoints, orchestrator/gateway metadata endpoints, and the `job_type` (`ai`/`transcoding`) filter are back in scope as of v1.9 via TD-017.

### Changes from v1.0 → v1.1 (2026-04-27)

- §6.2 — added `TransferBond` to the critical-events allowlist (strict-decode).
- §6.3 — added `TransferBond` (LPT-valued) and `WithdrawFees` (ETH-valued) rows to the BondingManager event catalog. Both surfaced from the SQLite seed but absent in v1.0. Resolves TD-003.
- §6.3 — replaced `TBD per ABI inspection` amount-field placeholders with verified field names from `abi/BondingManager.json`. Resolves Q-OD-7.
- §24.1 — added explicit acceptance criterion for the seed/canonical event cross-check pass (TD-004).

---

## Document Overview

This specification defines the design, behavior, and operational contract for a system that:

1. Indexes all critical Livepeer protocol events from Arbitrum.
2. Computes USD valuations at block-level precision for every monetary event when possible, otherwise persists an explicit terminal valuation outcome.
3. Tracks delegator stake balances at event-touching blocks.
4. Persists all data immutably with full audit provenance.
5. Exposes the resulting data via HTTP API.

The system is built around a single overriding principle: **byte-deterministic replay**. A complete database wipe followed by a re-run from cached RPC inputs and the seeded historical prices must produce identical output, row by row.

This specification is the authoritative source for v1 implementation. Material changes require explicit approval and a version bump.

---

## Table of Contents

1. [Purpose & Scope](#1-purpose--scope)
2. [Design Principles](#2-design-principles)
3. [System Architecture](#3-system-architecture)
4. [Technology Stack](#4-technology-stack)
5. [Livepeer Contract Registry](#5-livepeer-contract-registry)
6. [Event Catalog](#6-event-catalog)
7. [Pricing Methodology](#7-pricing-methodology)
8. [SQLite Seed Migration](#8-sqlite-seed-migration)
9. [Finality & Reorg Model](#9-finality--reorg-model)
10. [Failure Policy & Status Lifecycle](#10-failure-policy--status-lifecycle)
11. [Database Schema (Consolidated DDL)](#11-database-schema-consolidated-ddl)
12. [Concurrency, Idempotency & Determinism](#12-concurrency-idempotency--determinism)
13. [RPC Architecture](#13-rpc-architecture)
14. [API Surface](#14-api-surface)
15. [Deployment Topology](#15-deployment-topology)
16. [Configuration & Secrets](#16-configuration--secrets)
17. [Observability](#17-observability)
18. [Backup & Recovery](#18-backup--recovery)
19. [Runbook Outline](#19-runbook-outline)
20. [Out of Scope (v1)](#20-out-of-scope-v1)
21. [v2 Roadmap](#21-v2-roadmap)
22. [Open Data Items](#22-open-data-items)
23. [Master Requirements List](#23-master-requirements-list)
24. [Acceptance Criteria](#24-acceptance-criteria)

---

## 1. Purpose & Scope

### 1.1 Goals

The system shall:

- Index every critical Livepeer protocol event from Livepeer's Arbitrum deployment (Feb 2022) onward.
- Compute a valuation outcome for every monetary event: either a USD valuation at block-level precision, or an explicit terminal failure row explaining why no USD price was available.
- Compute stake balances for every active delegator at every event-touching block.
- Persist all data immutably with full audit provenance, including the intermediate pricing chain for every valuation.
- Provide HTTP API access to events, valuations, prices, and stake balances.
- Support deterministic backfill, replay, and recovery.

### 1.2 Primary use cases

- Protocol economic accounting (orchestrator earnings, fee revenue, reward inflation).
- Operator P&L computation.
- Stake economics analysis.
- Historical valuation queries for any indexed event.

### 1.3 Out of v1 scope

See Section 20 for full list. Briefly:

- Tax-lot accounting / cost-basis tracking.
- User-facing dashboards.
- Real-time (sub-finality) valuations.
- Multi-chain support beyond Arbitrum.
- Manual price overrides.
- Push-based event distribution (webhooks, Kafka).

### 1.4 Determinism contract

For any fixed input set (RPC cache + seeded SQLite), running the system from an empty database produces a byte-identical output database. This property is enforced by a CI test (§12.4) and is the system's load-bearing correctness guarantee.

---

## 2. Design Principles

### 2.1 Events are immutable

Raw blockchain events are append-only. The single exception is reorg-induced block reassignment, where `block_number` and `block_hash` may be updated under full audit (§9.3, §10.3). All other fields are write-once.

### 2.2 Block numbers are the source of truth

Valuations, stake balances, and all derived state are anchored to specific Arbitrum block numbers. Timestamps are recorded for human readability and never used as primary join keys.

### 2.3 Historical pricing is exact

Prices for past events are derived from one of two trusted sources:

1. The pre-existing SQLite database with verified historical prices, loaded once via the seed migrator.
2. On-chain reads via Uniswap V3 `observe()` and Chainlink `latestRoundData()` at the exact archive block of the event.

External price APIs (CoinGecko, etc.) are forbidden for primary pricing.

### 2.4 Valuations are immutable and versioned

`event_valuations` rows are never updated. New pricing logic requires a new `valuation_version`. Reports declare the version they consume; old reports remain self-consistent indefinitely.

### 2.5 Determinism is provable

A permanent RPC cache (§13.5) plus the trusted SQLite seed constitutes the deterministic input set. Dropping all derived data and replaying produces byte-identical output. A CI test verifies this on every pull request.

### 2.6 Failures are explicit

Every failure mode has a named status, a defined retry policy, and an observable signal. Nothing fails silently.

### 2.7 External dependencies are minimized

The system runs on Rust + Postgres + two RPC providers. No third-party indexing framework, no external pricing API, no managed services beyond the RPC providers and the Postgres host.

---

## 3. System Architecture

### 3.1 Service topology

The codebase still contains the original worker binaries, but the current v1
runtime shape is:

- `livepeer-daemon follow` for steady-state near-head processing
- `livepeer-api` for HTTP reads
- `livepeer-seed-migrator` as a one-shot tool
- `livepeer-orchestrator` for bounded `bootstrap`, `replay`, and `migrate-only`

The worker binaries remain valid internal execution units and bounded CLI entry
points:

| Service | Role |
|---|---|
| `livepeer-indexer` | Pulls logs from RPC, decodes against ABI registry, writes raw events. |
| `livepeer-reorg-watcher` | Validates parent-hash chain continuity, marks reorg'd rows non-canonical. |
| `livepeer-finality-watcher` | Observes L1 batch posting and L1 finalization, advances `finality` field. |
| `livepeer-valuator` | Prices finalized events, writes valuations under named versions. |
| `livepeer-staker` | Computes and persists stake balances at event-touching blocks. |
| `livepeer-daemon` | Runs the bounded worker loops continuously near head (`indexer`, `finality`, `reorg`, `valuator`, `staker`). |
| `livepeer-api` | Exposes data via HTTP (Axum). |
| `livepeer-seed-migrator` | One-shot tool. Imports trusted historical prices from SQLite. |
| `livepeer-orchestrator` | One-shot tool. Runs bounded `bootstrap`, strict `replay`, and `migrate-only`. |

All long-running services run as exactly one instance in v1. Horizontal scaling deferred to v2.

### 3.2 Data flow

```
Trusted SQLite seed              Arbitrum archive RPC (Chainstack)
        ↓                                    ↓
  seed-migrator / orchestrator        livepeer-daemon follow
        ↓                                    ↓
  seeded_event_prices            raw_protocol_events / finality / reorg
                                                  ↓
                                       valuations + stake snapshots
                                                  ↓
                                             livepeer-api
```

### 3.3 Service interaction model

- **Communication via Postgres only.** No shared queues, no IPC, no in-process coupling.
- **Crash-safe.** Each service can be killed at any time without data corruption. Atomic batch commits ensure clean recovery.
- **Independent failure.** Failure in one service does not cascade to others.
- **Idempotent.** Every write operation is safe under re-execution.

---

## 4. Technology Stack

### 4.1 Core stack

| Component | Choice | Rationale |
|---|---|---|
| Language | Rust (stable, pinned via `rust-toolchain.toml`) | Determinism, type safety, ecosystem |
| Async runtime | Tokio | Standard, mature |
| RPC client | Alloy | Modern, actively maintained, ABI handling included |
| Database | PostgreSQL 15+ | Battle-tested, JSONB, partial indexes, FK support |
| DB client | SQLx | Compile-time query verification — load-bearing for determinism |
| HTTP framework | Axum | Tokio-native, ergonomic, production-grade |
| Migration tool | sqlx-cli | Native to SQLx, raw SQL files (§7.4) |
| Logging | tracing | Structured JSON output |
| Metrics | prometheus crate | Standard exporter format |
| CLI | clap | Standard |
| Errors | anyhow + thiserror | Standard |

### 4.2 External tools

- **Foundry** (specifically `cast`) is the canonical debugging tool. **Every pricing computation must be reproducible by a human running `cast call ... --block N`.** This is a documented invariant; if a price cannot be reproduced this way, the pricing logic has a bug.

### 4.3 Explicitly rejected

- **rindexer.** Evaluated and rejected. The framework's own README marks it "brand new and actively under development." Reorg handling is undocumented. The abstraction would create determinism risks for our use case. The indexer is built in-house using alloy primitives.
- **External pricing APIs** (CoinGecko, CoinMarketCap, etc.). Forbidden for primary pricing or audit-trail use.
- **The Graph / Subgraph.** Outside scope.
- **Kubernetes.** Deferred to v2 if/when scale requires.

### 4.4 Version pinning

All Rust crates are pinned to exact versions in `Cargo.toml`. The `rust-toolchain.toml` pins the compiler version. The Postgres major version is pinned in deployment. Floating versions are forbidden — every pin moves through PR review.

---

## 5. Livepeer Contract Registry

### 5.1 Confirmed Arbitrum addresses (Delta version)

| Contract | Address |
|---|---|
| Governor | `0xD9dEd6f9959176F0A04dcf88a0d2306178A736a6` |
| Controller | `0xD8E8328501E9645d16Cf49539efC04f734606ee4` |
| LivepeerToken | `0x289ba1701C2F088cf0faf8B3705246331cB8A839` |
| Minter | `0xc20DE37170B45774e6CD3d2304017fc962f27252` |
| BondingManager (Proxy) | `0x35Bcf3c30594191d53231E4FF333E8A770453e40` |
| TicketBroker (Proxy) | `0xa8bB618B1520E284046F3dFc448851A1Ff26e41B` |
| RoundsManager (Proxy) | `0xdd6f56DcC28D3F5f27084381fE8Df634985cc39f` |
| BondingVotes (Proxy) | `0x0B9C254837E72Ebe9Fe04960C43B69782E68169A` |
| L2LPTGateway | `0x6D2457a4ad276000A615295f7A80F79E48CcD318` |

### 5.2 Address resolution at boot

Implementation targets are **never hardcoded**. At service boot, every long-running service queries the Controller via `Controller.getContract(keccak256(name))` for each tracked contract and:

1. Compares the resolved target address to the entries in `contract_abi_registry`.
2. Refuses to start if any target has changed since the last boot without a corresponding registry update.
3. Logs the resolved set on every boot for audit.

The Controller is the only address in the codebase that is hardcoded; all others flow through it.

### 5.3 Listening addresses

The indexer subscribes to **proxy** addresses, never targets. Events are emitted by the proxy (via `delegatecall` semantics). `raw_protocol_events.contract_address` always references the proxy.

### 5.4 ABI registry

Per-block-range ABI mapping enables decoding events from contracts that have been upgraded over time. Schema in §11.2.

For v1 implementation: the registry is populated with the **current Delta-version ABIs** for the entire `from_block = livepeer_arbitrum_genesis` to `to_block = NULL` range. Any pre-Delta event whose signature differs from current goes to `decode_failures` (§10.2).

For v2 (if needed): index the Controller's `SetContractInfo` events to build a complete upgrade history, fetch each historical implementation's ABI, populate per-range registry rows.

### 5.5 ABI integrity

Each ABI JSON file is committed to the repo at `abi/{ContractName}_{version}.json`. Its sha256 is recorded in `contract_abi_registry.abi_hash`. At service boot, every loaded ABI's hash is recomputed and compared to the registry; mismatch = refuse to start.

---

## 6. Event Catalog

This section enumerates every event the system tracks, with valuation semantics.

### 6.1 Event categories

Each event falls into exactly one of three categories:

| Category | `is_valuable` | Pricing path | Examples |
|---|---|---|---|
| LPT-valued | TRUE | LPT/USD | `Reward`, `Bond`, `Transfer` |
| ETH-valued | TRUE | ETH/USD | `WinningTicketRedeemed`, `DepositFunded` |
| Multi-asset | TRUE | both | `EarningsClaimed` |
| Non-monetary | FALSE | none | `NewRound`, `TranscoderUpdate`, `ServiceURIUpdate` |

### 6.2 Critical-events allowlist (strict-decode)

The following events trigger **strict decode halt** (§10.2) on any decode failure. The indexer refuses to advance past a block containing one of these in a non-decodable form:

- `Bond`, `Unbond`, `Rebond`, `WithdrawStake`, `TransferBond` (BondingManager — stake-worker depends on these)
- `Reward`, `EarningsClaimed` (BondingManager — high-value protocol events)
- `WinningTicketRedeemed`, `WinningTicketTransfer` (TicketBroker — high-value)
- `Transfer` (LivepeerToken — canonical token movement)

All other events go to `decode_failures` on decode error and indexing continues.

### 6.3 BondingManager events

| Event | Category | Amount field | `is_valuable` | Strict | Notes |
|---|---|---|---|---|---|
| `Bond` | LPT | `additionalAmount` | TRUE | YES | Stake inflow. **NOT `bondedAmount`** — that is the running post-bond total. |
| `Unbond` | LPT | `amount` | TRUE | YES | Stake outflow (unbonding lock created) |
| `Rebond` | LPT | `amount` | TRUE | YES | Restoration of unbonding lock to stake |
| `WithdrawStake` | LPT | `amount` | TRUE | YES | Final withdrawal of unbonded stake |
| `TransferBond` | LPT | `amount` | TRUE | YES | Stake position transferred between delegators (`old_delegator` → `new_delegator`). LPT-denominated movement; same severity class as `Bond`/`Transfer`. |
| `Reward` | LPT | `amount` | TRUE | YES | Newly minted LPT credited to transcoder + delegators (valued at market price; mint vs transfer distinction preserved in `event_name`) |
| `EarningsClaimed` | LPT + ETH | `rewards` (LPT) + `fees` (ETH) | TRUE | YES | Multi-asset event — generates two `event_valuations` rows |
| `WithdrawFees` | ETH | `amount` | TRUE | NO | Delegator withdrawal of accumulated ETH fees. Stake-worker reads `pendingFees()` directly (§11.10), so doesn't depend on event flow — non-strict. |
| `TranscoderActivated` | non-monetary | n/a | FALSE | NO | Active set entry |
| `TranscoderDeactivated` | non-monetary | n/a | FALSE | NO | Active set exit |
| `TranscoderUpdate` | non-monetary | n/a | FALSE | NO | rewardCut / feeShare changes |

### 6.4 TicketBroker events

| Event | Category | Amount field | `is_valuable` | Strict | Notes |
|---|---|---|---|---|---|
| `WinningTicketRedeemed` | ETH | `faceValue` | TRUE | YES | Primary ETH-flow event for orchestrator earnings |
| `WinningTicketTransfer` | ETH | `amount` | TRUE | YES | Cross-sender reserve transfer (less common) |
| `DepositFunded` | ETH | `amount` | TRUE | NO | Gateway/broadcaster deposit |
| `ReserveFunded` | ETH | `amount` | TRUE | NO | Gateway/broadcaster reserve |
| `Unlock` | non-monetary | n/a | FALSE | NO | Unlock initiated |
| `Withdrawal` | ETH | `deposit + reserve` (sum) | TRUE | NO | Gateway/broadcaster withdrawal — drains both deposit and reserve in one event. The on-chain event name is `Withdrawal`; the legacy SQLite seed labels it `Withdrawal` too. (Spec v1.0–v1.5 mistakenly called it `Withdraw`.) |

### 6.5 RoundsManager events

| Event | Category | `is_valuable` | Strict | Notes |
|---|---|---|---|---|
| `NewRound` | non-monetary | FALSE | NO | Round boundary marker. Used by stake-worker as a refresh trigger in v2 (out of scope for v1 per §20). |

### 6.6 LivepeerToken events

| Event | Category | Amount field | `is_valuable` | Strict | Notes |
|---|---|---|---|---|---|
| `Transfer` | LPT | `value` | TRUE | YES | Standard ERC20 transfer |
| `Approval` | non-monetary | n/a | FALSE | NO | Allowance change — not valued |
| `Mint` | LPT | `amount` | TRUE | NO | Underlying mint event (Reward emits this) |
| `Burn` | LPT | `amount` | TRUE | NO | Burn event |

### 6.7 Governance events (BondingVotes / Governor)

Indexed for completeness but not valued:

| Event | Category | `is_valuable` | Strict | Notes |
|---|---|---|---|---|
| `ProposalCreated` | non-monetary | FALSE | NO | |
| `VoteCast` | non-monetary | FALSE | NO | |
| `ProposalExecuted` | non-monetary | FALSE | NO | |

### 6.8 Multi-asset event handling: `EarningsClaimed`

`EarningsClaimed` carries both an LPT reward amount and an ETH fee amount in a single log. The canonical key `(chain_id, tx_hash, log_index)` is preserved (no synthetic sub-index).

Implementation:

- One row in `raw_protocol_events` with `asset = NULL` and the multi-asset breakdown preserved in `raw_event` JSONB.
- Two rows in `event_valuations`, distinguished by `asset` (which is part of the primary key): one with `asset = 'LPT'` for the reward portion, one with `asset = 'ETH'` for the fee portion.
- API responses for this event return an array of valuations.

### 6.9 Non-monetary event handling

Events with `is_valuable = FALSE`:

- Are indexed in `raw_protocol_events`.
- Are never picked up by the valuator (filtered by `WHERE is_valuable = TRUE`).
- Have no rows in `event_valuations`.
- Are queryable via the API for context (e.g., reconstructing round boundaries, observing rewardCut changes).

### 6.10 Event ABI confirmation requirement

The "TBD per ABI inspection" entries above will be populated during implementation by inspecting the deployed Delta-version BondingManager and TicketBroker ABIs from Arbiscan and confirming each event's field layout. This work is part of the implementation kickoff, not part of the spec.

---

## 7. Pricing Methodology

### 7.1 Pricing version policy

Every valuation is stamped with a `valuation_version` string. New pricing logic, new TWAP windows, new oracle sources — any of these — get a new version. **Old valuation rows are never updated.**

Version strings follow the pattern `vN_<descriptor>`:

- `v0_debug_spot` — Spot read at event block. Diagnostic only. Kept as a comparison point against TWAP, never used as the canonical valuation.
- `v1_lpt_weth_twap_30min_x_chainlink_eth` — Primary v1 method (see §7.3).
- Future versions follow the same pattern.

### 7.2 Pricing assets

Two assets are priced in v1:

- **LPT** — Livepeer Token. Priced via Uniswap V3 LPT/WETH pool × Chainlink ETH/USD.
- **ETH** — Ethereum. Priced via Chainlink ETH/USD directly.

USDC is not priced in v1. The LPT/USDC pool on Arbitrum has $108 in liquidity and is unusable. See §7.4.

### 7.3 v1 primary method: TWAP × Chainlink

```
LPT/USD = TWAP_30min(LPT/WETH @ pool 0x4fd47e5102dfbf95541f64ed6fe13d4ed26d2546)
          × Chainlink(ETH/USD @ Arbitrum aggregator)

ETH/USD = Chainlink(ETH/USD @ Arbitrum aggregator)
```

#### 7.3.1 LPT/WETH from Uniswap V3 `observe()`

The Uniswap V3 pool exposes `observe(secondsAgos[])` which returns cumulative tick observations. To compute the 30-minute TWAP:

```
secondsAgos = [1800, 0]
(tickCumulatives, _) = pool.observe(secondsAgos, blockNumber=N)
twapTick = (tickCumulatives[1] - tickCumulatives[0]) / 1800
twapPrice = 1.0001^twapTick   (with token decimal correction)
```

Decimal correction: LPT and WETH are both 18-decimal. The raw tick-derived price is `WETH per LPT` (or its inverse, depending on token0/token1 ordering at the pool). Token0/token1 ordering is determined at pool deployment and verified at boot.

#### 7.3.2 Pool observation cardinality

Uniswap V3 pools store a ringbuffer of price observations. The default cardinality is 1, which makes 30-minute TWAP impossible. Someone must call `increaseObservationCardinalityNext` to enable longer windows.

**Boot-time check:** the indexer verifies pool cardinality at the start of every backfill window. If cardinality is insufficient at any historical block range, valuations in that range fall back to a degraded version (e.g., spot, or shorter TWAP) and are tagged with an explicit version such as `v1_degraded_spot_pre_cardinality`.

**Verified historical impact (2026-04-27):** the LPT/WETH pool was deployed shortly after Livepeer's Arbitrum genesis (block 6,072,093, 2022-02-15) and cardinality stayed at 1–2 through ~block 33M (~late 2022) before jumping to 601 in (33M, 35M]. Approximately **17,032 monetary events** (`Bond`, `Unbond`, `Rebond`, `TransferBond`, `WithdrawStake`, `EarningsClaimed`, `WithdrawFees`, `DepositFunded`, `ReserveFunded`, `ReserveClaimed`, `Withdrawal`) fall in `[genesis, 32M)` and have no SQLite seed coverage — these must take a degraded valuation version. v1 implementation must therefore build the degraded-version path, not stub it. `Reward` and `WinningTicketRedeemed` events in the same window are seeded and bypass on-chain pricing entirely. See `docs/design-docs/on-chain-references.md` for full counts and bisection log.

#### 7.3.3 Chainlink ETH/USD on Arbitrum

The Chainlink aggregator address is recorded in the configuration and verified at boot via a `cast call` to confirm it returns sane data. The decimals (8) and heartbeat (24h) are confirmed from the deployed aggregator metadata.

`latestRoundData()` returns `(roundId, answer, startedAt, updatedAt, answeredInRound)`. The pricing module:

1. Calls `latestRoundData` at the event block.
2. Validates `answeredInRound >= roundId` (mandatory check — fails as `failed_missing_oracle` if violated).
3. Validates staleness: `block.timestamp(N) - updatedAt <= 86400` (24h heartbeat). If violated, fails as `failed_missing_oracle`.
4. Logs WARN if staleness exceeds 4 hours (anomaly investigation signal).

#### 7.3.4 L2 Sequencer Uptime Feed

Before any pricing computation, the system reads the L2 Sequencer Uptime Feed at the event block. Feed address recorded in config.

If the sequencer was down or in grace period at the event block (or within the 30-minute TWAP window prior), the valuation is `failed_sequencer_outage`. No on-chain data from that period is trusted.

#### 7.3.5 Post-block state convention

`eth_call` at block N returns the post-block state. If a Chainlink round update transaction is included in block N at log_index 7, and our event of interest is at log_index 5, `eth_call(latestRoundData, blockNumber=N)` returns the post-update round.

This is documented EVM convention. The system uses post-block state without correction. The error bound this introduces is bounded by Chainlink's deviation threshold (0.5%).

### 7.4 Excluded methods

| Method | Status | Reason |
|---|---|---|
| `lpt_usdc_pool` | EXCLUDED v1 | Pool TVL is $108 (1 LPT, 84 USDC). Unusable. |
| Hardcoded `usdc_usd_price = 1.0` | EXCLUDED | March 2023 USDC depeg precedent — would have introduced 13% error. |
| External APIs (CoinGecko, etc.) | FORBIDDEN | Determinism principle — no off-chain API in primary pricing path. |
| Daily candle prices | FORBIDDEN | Block-precision requirement. |
| Mainnet pool prices | FORBIDDEN | Use Arbitrum pool only. |

### 7.5 Pricing chain provenance

Every `event_valuations` row carries a `pricing_chain` JSONB field documenting the full derivation:

```json
{
  "steps": [
    {
      "asset": "LPT",
      "quote": "WETH",
      "price": "0.003478",
      "source": "uniswap_v3_twap_30min",
      "pool": "0x4fd47e5102dfbf95541f64ed6fe13d4ed26d2546",
      "block_number": 194500000,
      "raw_observe": { "tickCumulatives": ["...", "..."], "secondsAgos": [1800, 0] }
    },
    {
      "asset": "WETH",
      "quote": "USD",
      "price": "4500.12",
      "source": "chainlink",
      "oracle": "0x639Fe6...",
      "block_number": 194500000,
      "raw_round": { "roundId": "...", "answer": "...", "updatedAt": "..." }
    }
  ],
  "result": {
    "asset": "LPT",
    "quote": "USD",
    "price": "15.65"
  }
}
```

An auditor can re-derive the final USD value end-to-end from this JSON without making any RPC call.

### 7.6 Pricing cross-check

Going-forward (post-seed-boundary) pricing reads are made against **two RPC providers** for `eth_getLogs` and block-hash reads (§13.3). For historical state reads (`eth_call` at archive blocks), only the archive provider can respond — these are single-source.

Cross-check is **method-aware**:

- For `eth_call` (e.g. `slot0`, `observe`, `latestRoundData`) the result is a hex-blob string with no provider rendering choices, so **raw-bytes compare** is correct and bit-exact.
- For `eth_getBlockByNumber` providers disagree on JSON shape — Chainstack emits `requestsHash`/`withdrawals` as `null`; liveinfraspe omits them — even when chain data agrees. Cross-check therefore extracts `.hash` from each response and compares the hashes. The load-bearing invariant from §9.2 is "block N has hash H"; full-header byte compare would be perpetually noisy.
- For `eth_getLogs` cross-check is logs-by-(tx_hash, log_index) at raw byte level on each entry; ordering may differ between providers.

Any mismatch on the chosen invariant is `failed_rpc_divergence` — never auto-retried, always surfaced for human review.

---

## 8. SQLite Seed Migration

### 8.1 Source database overview

A pre-existing SQLite database contains trusted historical USD valuations for a subset of Livepeer events. This database is loaded **once** via the `livepeer-seed-migrator` tool and never re-loaded.

The SQLite is treated as **trusted**: no re-verification pass at migration time. Its contents become the canonical price source for any event whose `tx_hash` matches a seeded row.

### 8.2 Source schema (relevant tables)

The following SQLite tables are consumed:

#### 8.2.1 `payout` — TicketBroker payouts with USD valuations

```sql
CREATE TABLE payout (
   transaction_id TEXT PRIMARY KEY NOT NULL,
   timestamp INTEGER NOT NULL,
   face_value NUMBER NOT NULL DEFAULT 0.00,
   face_value_usd NUMBER NOT NULL DEFAULT 0.00,
   recipient_id TEXT NOT NULL DEFAULT "",
   eth_price NUMBER NOT NULL DEFAULT 0.00,
   orch_commission NUMBER NOT NULL DEFAULT 0.00,
   orch_commission_usd NUMBER NOT NULL DEFAULT 0.00,
   fee_cut NUMBER NOT NULL DEFAULT 0.00,
   transaction_fee NUMBER NOT NULL DEFAULT 0.00,
   sender_id TEXT NOT NULL DEFAULT "...",
   sent_to_discord INTEGER DEFAULT 0
);
```

Maps to ETH-valued `WinningTicketRedeemed` events. `face_value` (ETH amount), `face_value_usd` (USD valuation), `eth_price` (ETH/USD at time of event) are imported.

#### 8.2.2 `reward` — BondingManager rewards with USD valuations

```sql
CREATE TABLE reward (
   transaction_id TEXT PRIMARY KEY NOT NULL,
   eth_address TEXT NOT NULL,
   timestamp INTEGER NOT NULL,
   total_tokens NUMBER NOT NULL DEFAULT 0.00,
   orch_tokens NUMBER NOT NULL DEFAULT 0.00,
   orch_tokens_usd NUMBER NOT NULL DEFAULT 0.00,
   reward_cut NUMBER NOT NULL DEFAULT 0.00,
   transaction_fee NUMBER NOT NULL DEFAULT 0.00,
   transaction_fee_usd NUMBER NOT NULL DEFAULT 0.00,
   eth_price NUMBER NOT NULL DEFAULT 0.00,
   lpt_price NUMBER NOT NULL DEFAULT 0.00
);
```

Maps to LPT-valued `Reward` events. `total_tokens` (LPT amount), `lpt_price` (LPT/USD), and derived `*_usd` fields are imported.

#### 8.2.3 `events` — Generic event log with payload

The `payload` column is the canonical decoded source. All other tables in the SQLite are derived from `payload` plus on-chain reads at indexing time.

The structure of `payload` requires inspection of sample rows during migration utility implementation (open data item Q-OD-3, §22).

### 8.3 Tables explicitly ignored

The following SQLite tables are **not** consumed:

- `orchestrator` — current-state metadata (not historical)
- `broadcaster` — current-state metadata
- `proposals` — governance state
- `votes` — governance state
- `block_cursors` — per-event-type checkpoints. Not needed: the valuator does a flat `(chain_id, tx_hash, asset)` lookup against `seeded_event_prices` — a hit means the seed has it, a miss means on-chain pricing. No per-type bound vector required.

Governance and orchestrator metadata may be re-derived from on-chain events post-v1; they are not part of the seed migration.

### 8.4 Migration target schema

Imported into `seeded_event_prices` (§11.5). The migrator inserts one row per (transaction_id, asset) tuple, marked `source = 'trusted_historical_seed_v1'`.

For payouts: one row with `asset = 'ETH'`, carrying `face_value`, `face_value_usd`, `eth_price`.

For rewards: one row with `asset = 'LPT'`, carrying `total_tokens`, `orch_tokens_usd`, `lpt_price`.

The full SQLite row is preserved in the `raw` JSONB column for audit.

### 8.5 Architectural model

The SQLite is a **price overlay**, not an event mirror. The indexer fetches canonical events from RPC for the entire history (including the seeded range) — this gives us canonical `(chain_id, tx_hash, log_index)`, `block_hash`, all event types, and reorg-resistance.

The valuator's pricing logic checks `seeded_event_prices` **before** doing on-chain pricing:

```
For each unvalued (event_id, version, asset) tuple:
  1. Look up seeded_event_prices by (chain_id, tx_hash, asset).
  2. If hit: use seeded price, stamp source='trusted_historical_seed_v1', insert valuation.
  3. Else: do on-chain TWAP/Chainlink read, stamp source='uniswap_v3_dual_rpc', insert valuation.
```

### 8.6 Migration utility behavior

`livepeer-seed-migrator` is a one-shot command:

1. Connects to source SQLite (read-only).
2. Connects to target Postgres.
3. Iterates `payout` rows — inserts into `seeded_event_prices` with `event_type_hint = 'payout'`.
4. Iterates `reward` rows — inserts into `seeded_event_prices` with `event_type_hint = 'reward'`.
5. Imports `events.payload` rows into a staging table for the seed/canonical cross-check pass (§24.1).
6. Logs a summary: rows imported per table, min/max block.
7. Exits.

Migration is idempotent: re-running on the same input is safe (`ON CONFLICT DO NOTHING`).

### 8.7 Data items requiring spot-check

Before final implementation:

- `Q-OD-1` — NUMBER precision spot-check. Compare `reward.total_tokens` for a known reward against the on-chain `Reward` event amount. If lossy (SQLite REAL is 53-bit mantissa), `amount_native` is re-derived from RPC at valuation time and only `*_usd_price` and `*_usd` fields are taken from SQLite.
- `Q-OD-2` — `transaction_id` uniqueness. Verify no transactions exist with multiple rewards or multiple payouts. If they do, the migrator needs `log_index` from RPC to disambiguate.
- `Q-OD-3` — `events.payload` structure. Inspection of sample rows determines whether and how to use this table.
- `Q-OD-4` — `block_cursors` contents. The actual per-event-type bounds.

---

## 9. Finality & Reorg Model

### 9.1 Finality threshold policy

The system operates a **two-tier model**:

| Phase | What happens | Latency |
|---|---|---|
| Indexing | Events written to `raw_protocol_events` as soon as they are sequencer-confirmed. `finality = 'tentative'`. | Seconds |
| L1 batch posting | `finality-watcher` observes batch on Ethereum L1. Updates rows to `finality = 'l1_posted'`. | ~10 minutes |
| L1 finalization | `finality-watcher` observes Ethereum finality of the batch tx. Updates rows to `finality = 'finalized'`. | ~25-30 minutes total |

**The valuator only consumes `finality = 'finalized'` rows.** Once a valuation is written, the underlying event is by definition immutable on L1, and the valuation never needs to be retracted.

### 9.2 Reorg watcher

Operating on the tentative window (rows not yet finalized), the reorg watcher validates parent-hash chain continuity.

#### 9.2.1 Algorithm

```
loop {
    head = chain_head_block()
    walk_depth = 7500
    
    for n in (head - walk_depth) ..= head {
        chain_hash = rpc.get_block_hash(n)
        stored_hash = db.stored_block_hash(n)
        
        if stored_hash.is_some() && stored_hash != chain_hash {
            // Divergence at block n
            mark_blocks_non_canonical(n ..= last_known_tip)
            insert_reorg_event(n, depth, old_hashes, new_hashes)
            trigger_reindex(n ..= head)
            alert(severity_for_depth(depth))
            break
        }
    }
    
    sleep(cadence)
}
```

#### 9.2.2 Cadence

- **Normal mode:** poll every 15 seconds.
- **Heightened mode:** if a reorg was detected in the last 5 minutes, poll every 5 seconds. Decays back to normal after 5 minutes of clean polls.
- **Backoff mode:** after 1 hour of clean polls in normal mode, drop to 60 seconds. First detected divergence returns to normal mode.

#### 9.2.3 Walk depth

7,500 L2 blocks (~30 minutes at Arbitrum's ~250ms block time). Covers the entire pre-finality window with margin.

### 9.3 Reorg-induced mutation

When a reorg moves the same transaction to a different block, the `raw_protocol_events` row's `block_number` and `block_hash` are updated. **This is the only mutation ever applied to `raw_protocol_events`.**

Every such mutation is logged in `reorg_mutations` (§11.13) with:

- The reorg event ID
- The affected `raw_event_id`
- Old and new block numbers / hashes
- Mutation timestamp

`event_valuations` rows are never mutated under any circumstances. If a reorg invalidates a previously-finalized event (extremely rare — requires Ethereum L1 reorg or fraud proof), see §10.5.

### 9.4 Reorg severity thresholds

| Reorg depth | Severity | Notification |
|---|---|---|
| 0-2 blocks | INFO | log only |
| 3-50 blocks | WARN | log + Telegram |
| > 50 blocks | CRITICAL | log + Telegram (urgent) |

### 9.5 Un-finalization policy

Theoretical scenarios where a finalized event becomes invalid:

1. Ethereum L1 reorg of the batch-posting transaction (vanishingly rare).
2. Successful Arbitrum fraud proof (has not happened in production).

If either occurs:

- `raw_protocol_events.is_canonical` is set FALSE on affected rows.
- `event_valuations` rows are **never touched**.
- A new `valuation_version` is created (e.g., `v1_post_reorg_2026_03_15`).
- New events from the canonical chain are valued under the new version.
- A CRITICAL alert pages a human regardless of the configured "no-pager" rule (this overrides Q5.D — it's catastrophic).

Reports declare which version they used; pre-event reports remain self-consistent.

---

## 10. Failure Policy & Status Lifecycle

### 10.1 Status enum

`event_valuations.status` values:

- `priced` — Successfully valued.
- `priced_with_warning` — Valued, but with a flag (e.g., TWAP-vs-spot deviation outside tolerance).
- `failed_missing_pool` — Terminal outcome row; the pool did not exist or could not serve the required window at this block.
- `failed_missing_oracle` — Terminal outcome row; Chainlink data was unavailable or stale after policy-defined retries.
- `failed_sequencer_outage` — Terminal outcome row; the L2 sequencer outage policy exhausted without a safe valuation window.

For the three terminal failure statuses above, `event_valuations.native_usd_price` and `event_valuations.amount_usd` are `NULL`; `pricing_chain` contains the failure detail/provenance.

`valuation_attempts.result_status` values include all of the above plus:

- `pending` — Initial state (not yet attempted).
- `not_applicable` — Non-monetary event, will never be valued.
- `failed_rpc` — Transient RPC error.
- `failed_rpc_divergence` — Two providers disagreed.
- `failed_decode` — ABI decode failed.
- `failed_invalid_event` — Event semantics violated (manual review).

### 10.2 Decode failure handling: tiered strictness

Two paths based on whether the event is on the **critical-events allowlist** (§6.2):

#### 10.2.1 Critical events: strict halt

If a log on a critical event signature fails to decode:

1. The indexer batch fails entirely.
2. The checkpoint does not advance.
3. A CRITICAL alert fires (Telegram).
4. Indexing halts at this block until the operator updates the ABI registry and resolves the failure.

This protects against silent stake-worker drift and silent valuator misses on high-value events.

#### 10.2.2 Non-critical events: permissive dead-letter

If a log on a non-critical event signature fails to decode:

1. The log is written to `decode_failures` (§11.4).
2. A WARN alert fires.
3. The indexer continues with the rest of the batch.
4. The checkpoint advances.

Operator can later run `livepeer-indexer recover-decode-failures --abi <ContractName>` after updating the ABI registry. Successfully-decoded recovered failures are inserted into `raw_protocol_events`; the original `decode_failures` row is marked resolved with a reference.

### 10.3 Per-status retry matrix

| Status | Auto-retry | Backoff | Max attempts | After exhaust |
|---|---|---|---|---|
| `failed_rpc` | YES | 1m, 5m, 30m, 2h, 12h | 5 | human review |
| `failed_rpc_divergence` | NO | — | — | immediate human review |
| `failed_missing_pool` | NO | — | — | terminal |
| `failed_missing_oracle` | YES once | +24h | 1 | terminal if still missing |
| `failed_sequencer_outage` | YES | re-evaluate every 1h | until sequencer feed reports recovery + TWAP window valid | terminal if 30 days elapsed |
| `failed_decode` | NO | — | — | terminal until ABI registry updated |
| `failed_invalid_event` | NO | — | — | terminal — manual investigation |

Retries are deterministic: timing is derived from event block timestamp + attempt number, not wall-clock. Replay produces identical retry sequences.

### 10.4 Idempotent backfill commands

Three operator commands must always be safe to re-run:

1. `livepeer-indexer backfill --from-block N --to-block M` — re-fetches and re-inserts events; no duplicates created.
2. `livepeer-valuator backfill --version v1` — values all unvalued events at the given version; already-valued are skipped.
3. `livepeer-staker backfill --from-block N --to-block M` — refreshes stake balances; conflicts no-op.

Each is essentially a `WHERE NOT EXISTS`-driven worker. No `--force` flag in v1.

### 10.5 Determinism violations

If the valuator attempts to insert a row into `event_valuations` whose key `(event_id, valuation_version, asset)` already exists, and the new computed values differ from stored values:

1. The insert is rejected (`ON CONFLICT DO NOTHING`).
2. A CRITICAL alert fires.
3. The mismatch details are logged to `valuation_attempts` with `result_status = 'failed_determinism_violation'` and full diff.

This is the determinism guard. In a correctly-functioning system, this should never fire. If it does, either pricing logic has changed without a version bump, or there's a bug.

### 10.6 Alerting and escalation

| Trigger | Severity | Channel |
|---|---|---|
| Reorg detected, depth ≤ 2 blocks | INFO | log only |
| Reorg detected, depth > 2 blocks | WARN | log + Telegram |
| Un-finalization of finalized event | CRITICAL | log + Telegram (urgent) |
| `failed_rpc_divergence` | CRITICAL | log + Telegram |
| Determinism violation (§10.5) | CRITICAL | log + Telegram (urgent) |
| Decode failure rate > 1% over 5 min | WARN | log + Telegram |
| Single decode failure on critical-events allowlist | WARN | log + Telegram |
| Indexer checkpoint lag > 1000 blocks | WARN | log + Telegram |
| Indexer checkpoint lag > 10000 blocks | CRITICAL | log + Telegram |
| Valuator backlog > 1000 unvalued events | WARN | log + Telegram |
| RPC error rate > 20% on any provider sustained 60s | WARN | log + Telegram |
| Both providers down | CRITICAL | log + Telegram |
| Sequencer feed reports outage | INFO | log only (informational) |
| Chainstack monthly call budget > 80% used | WARN | log + Telegram |
| Postgres connection pool exhausted | CRITICAL | log + Telegram |

**Notification channels:**

- **log only:** structured JSON to stdout, captured by external log aggregation.
- **Telegram:** webhook to a configured bot/channel. No paging service in v1.

**Build-order note:** structured logging and Prometheus metric emission are active from day one (zero-cost when no scraper is configured). Telegram alert wiring and Grafana dashboards are last-priority — built after indexer, valuator, stake-worker, API, and migrator are functional.

---

## 11. Database Schema (Consolidated DDL)

This section is the canonical schema for v1. All migrations starting from migration `001` reproduce these tables exactly.

### 11.1 Migration tooling and discipline

Migrations are managed with **sqlx-cli**. Each migration is a pair of SQL files:

```
migrations/
  001_create_indexer_checkpoints.up.sql
  001_create_indexer_checkpoints.down.sql
  002_create_contract_abi_registry.up.sql
  002_create_contract_abi_registry.down.sql
  ...
```

Migration discipline rules:

1. **Migrations are immutable once merged.** New changes require new migration files.
2. **Down migrations exist for development; never run in production.** Production rollback is forward-only via a new corrective migration.
3. **No destructive migrations without explicit operator command.** Drops, narrowings, type changes are prefixed `_destructive_` and require `--allow-destructive` to apply.
4. **Migrations are idempotent on re-run.** `IF NOT EXISTS` clauses where appropriate.
5. **Migrations include a comment block.** Purpose, ticket reference, author, runtime impact.

### 11.2 `indexer_checkpoints`

Per-service progress tracking.

```sql
CREATE TABLE indexer_checkpoints (
  name TEXT PRIMARY KEY,
  chain_id BIGINT NOT NULL,
  last_processed_block BIGINT NOT NULL,
  last_processed_block_hash TEXT,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

Names used: `'main'`, `'reorg_watcher'`, `'finality_watcher'`, `'valuator_v1'`, `'staker'`.

### 11.3 `contract_abi_registry`

Per-block-range ABI mapping with strict-decode flag.

```sql
CREATE TABLE contract_abi_registry (
  contract_name TEXT NOT NULL,
  proxy_address TEXT NOT NULL,
  target_address TEXT NOT NULL,
  from_block BIGINT NOT NULL,
  to_block BIGINT,
  abi_path TEXT NOT NULL,
  abi_hash TEXT NOT NULL,
  strict_decode BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (contract_name, from_block)
);

CREATE INDEX idx_abi_registry_proxy ON contract_abi_registry (proxy_address, from_block);
```

### 11.4 `raw_protocol_events`

Central event table. Event-level data is denormalized for query speed; full payload preserved in JSONB.

```sql
CREATE TABLE raw_protocol_events (
  id BIGSERIAL PRIMARY KEY,
  
  -- Canonical identity
  chain_id BIGINT NOT NULL,
  tx_hash TEXT NOT NULL,
  log_index INT NOT NULL,
  
  -- Block context
  block_number BIGINT NOT NULL,
  block_hash TEXT NOT NULL,
  block_timestamp TIMESTAMPTZ NOT NULL,
  
  -- Contract / event identity
  contract_address TEXT NOT NULL,
  contract_name TEXT NOT NULL,
  event_name TEXT NOT NULL,
  event_signature TEXT NOT NULL,        -- topic0
  
  -- Semantics
  asset TEXT,                            -- 'LPT' | 'ETH' | NULL for non-monetary
  amount_raw NUMERIC(78, 0),
  amount_normalized NUMERIC(38, 18),
  is_valuable BOOLEAN NOT NULL,
  
  -- Common decoded fields
  from_address TEXT,
  to_address TEXT,
  
  -- Lifecycle
  finality TEXT NOT NULL DEFAULT 'tentative',
  is_canonical BOOLEAN NOT NULL DEFAULT TRUE,
  finalized_at TIMESTAMPTZ,
  l1_batch_tx_hash TEXT,
  
  -- Full payload
  raw_event JSONB NOT NULL,
  
  -- Provenance
  abi_hash_used TEXT NOT NULL,
  
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  
  UNIQUE (chain_id, tx_hash, log_index),
  CHECK (finality IN ('tentative', 'l1_posted', 'finalized'))
);

CREATE INDEX idx_events_block ON raw_protocol_events (chain_id, block_number);
CREATE INDEX idx_events_contract_event ON raw_protocol_events (contract_name, event_name, block_number);
CREATE INDEX idx_events_valuable_finality ON raw_protocol_events (is_valuable, finality, is_canonical) 
  WHERE is_valuable = TRUE;
CREATE INDEX idx_events_from_address ON raw_protocol_events (from_address) WHERE from_address IS NOT NULL;
CREATE INDEX idx_events_to_address ON raw_protocol_events (to_address) WHERE to_address IS NOT NULL;
CREATE INDEX idx_events_block_timestamp ON raw_protocol_events (block_timestamp);
```

The partial index `idx_events_valuable_finality` is the hot path for the valuator's "find unvalued events" query.

### 11.5 `seeded_event_prices`

Trusted historical valuations imported from SQLite.

```sql
CREATE TABLE seeded_event_prices (
  chain_id BIGINT NOT NULL,
  tx_hash TEXT NOT NULL,
  log_index INT NOT NULL DEFAULT -1,     -- -1 sentinel for seed rows; concrete log_index for cross-checked imports (Q-OD-2: tx_hash is unique per seed table, so -1 collisions are correct).
  event_type_hint TEXT NOT NULL,         -- 'reward' | 'payout'
  asset TEXT NOT NULL,
  
  amount_native NUMERIC(38, 18) NOT NULL,
  amount_usd NUMERIC(38, 18) NOT NULL,
  asset_usd_price NUMERIC(38, 18) NOT NULL,
  
  source TEXT NOT NULL DEFAULT 'trusted_historical_seed_v1',
  raw JSONB,
  imported_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  
  PRIMARY KEY (chain_id, tx_hash, log_index, asset)
);

CREATE INDEX idx_seeded_lookup ON seeded_event_prices (chain_id, tx_hash, asset);
```

### 11.6 `token_prices_by_block`

On-chain price reads (cache for valuator).

```sql
CREATE TABLE token_prices_by_block (
  chain_id BIGINT NOT NULL,
  asset TEXT NOT NULL,
  quote TEXT NOT NULL,
  
  block_number BIGINT NOT NULL,
  block_hash TEXT NOT NULL,
  block_timestamp TIMESTAMPTZ NOT NULL,
  
  price NUMERIC(38, 18) NOT NULL,
  
  source TEXT NOT NULL,                  -- 'uniswap_v3_twap_30min' | 'uniswap_v3_spot' | 'chainlink' | 'trusted_historical_seed_v1'
  pool_address TEXT,
  oracle_address TEXT,
  
  raw JSONB,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  
  PRIMARY KEY (chain_id, asset, quote, block_number, source)
);

CREATE INDEX idx_token_prices_lookup ON token_prices_by_block (chain_id, asset, quote, block_number DESC);
```

### 11.7 `event_valuations`

Immutable per `(event_id, valuation_version, asset)`. The asset column in the PK supports multi-asset events (e.g., `EarningsClaimed` produces two rows per version). This table records the terminal valuation **outcome** for each priced asset slice: successful prices have numeric USD fields; terminal failures carry `NULL` USD fields plus failure provenance in `pricing_chain`.

```sql
CREATE TABLE event_valuations (
  event_id BIGINT NOT NULL REFERENCES raw_protocol_events(id),
  valuation_version TEXT NOT NULL,
  asset TEXT NOT NULL,
  pricing_method TEXT NOT NULL,
  
  chain_id BIGINT NOT NULL,
  block_number BIGINT NOT NULL,
  
  amount_native NUMERIC(38, 18) NOT NULL,
  native_usd_price NUMERIC(38, 18),
  amount_usd NUMERIC(38, 18),
  
  pricing_chain JSONB NOT NULL,
  
  status TEXT NOT NULL,                  -- 'priced' | 'priced_with_warning' | 'failed_missing_pool' | 'failed_missing_oracle' | 'failed_sequencer_outage'
  source TEXT NOT NULL,                  -- 'trusted_historical_seed_v1' | 'uniswap_v3_dual_rpc' | 'chainlink_dual_rpc'
  
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  
  PRIMARY KEY (event_id, valuation_version, asset),
  CHECK (
    status IN (
      'priced',
      'priced_with_warning',
      'failed_missing_pool',
      'failed_missing_oracle',
      'failed_sequencer_outage'
    )
  )
);

CREATE INDEX idx_valuations_version_block ON event_valuations (valuation_version, block_number);
CREATE INDEX idx_valuations_asset_block ON event_valuations (asset, block_number);
```

### 11.8 `valuation_attempts`

Audit trail of every pricing attempt.

```sql
CREATE TABLE valuation_attempts (
  id BIGSERIAL PRIMARY KEY,
  event_id BIGINT NOT NULL REFERENCES raw_protocol_events(id),
  valuation_version TEXT NOT NULL,
  asset TEXT NOT NULL,
  attempt_number INT NOT NULL,
  
  attempted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  result_status TEXT NOT NULL,
  error_detail JSONB,
  next_retry_at TIMESTAMPTZ,
  
  UNIQUE (event_id, valuation_version, asset, attempt_number)
);

CREATE INDEX idx_attempts_retry ON valuation_attempts (next_retry_at) WHERE next_retry_at IS NOT NULL;
CREATE INDEX idx_attempts_event ON valuation_attempts (event_id, valuation_version, asset, attempt_number DESC);
```

### 11.9 `decode_failures`

Dead-letter table for non-critical decode failures.

```sql
CREATE TABLE decode_failures (
  id BIGSERIAL PRIMARY KEY,
  chain_id BIGINT NOT NULL,
  block_number BIGINT NOT NULL,
  block_hash TEXT NOT NULL,
  tx_hash TEXT NOT NULL,
  log_index INT NOT NULL,
  contract_address TEXT NOT NULL,
  topics TEXT[] NOT NULL,
  data BYTEA NOT NULL,
  attempted_abi_hash TEXT NOT NULL,
  error_message TEXT NOT NULL,
  resolved_at TIMESTAMPTZ,
  resolved_event_id BIGINT REFERENCES raw_protocol_events(id),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  
  UNIQUE (chain_id, tx_hash, log_index)
);

CREATE INDEX idx_decode_failures_unresolved ON decode_failures (created_at) WHERE resolved_at IS NULL;
```

### 11.10 `stake_balances_by_block`

Per-event-block stake snapshots. Scope 2 (event-triggered, not full fan-out).

```sql
CREATE TABLE stake_balances_by_block (
  chain_id BIGINT NOT NULL,
  delegator_address TEXT NOT NULL,
  delegate_address TEXT NOT NULL,
  block_number BIGINT NOT NULL,
  block_timestamp TIMESTAMPTZ NOT NULL,
  block_hash TEXT NOT NULL,
  
  bonded_principal NUMERIC(38, 18) NOT NULL,
  pending_stake NUMERIC(38, 18),
  pending_fees NUMERIC(38, 18),
  pending_round BIGINT,
  
  source TEXT NOT NULL,                  -- 'flow_derived' | 'pending_call' | 'both'
  raw_call JSONB,
  
  triggering_event_id BIGINT REFERENCES raw_protocol_events(id),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  
  PRIMARY KEY (chain_id, delegator_address, block_number)
);

CREATE INDEX idx_stake_delegator_recent ON stake_balances_by_block (delegator_address, block_number DESC);
CREATE INDEX idx_stake_delegate ON stake_balances_by_block (delegate_address, block_number DESC);
```

### 11.11 `delegator_registry`

Fast lookup of all known delegators, derived from Bond events.

```sql
CREATE TABLE delegator_registry (
  chain_id BIGINT NOT NULL,
  delegator_address TEXT NOT NULL,
  first_bond_block BIGINT NOT NULL,
  first_bond_event_id BIGINT NOT NULL REFERENCES raw_protocol_events(id),
  last_seen_block BIGINT NOT NULL,
  last_seen_event_id BIGINT NOT NULL REFERENCES raw_protocol_events(id),
  is_active BOOLEAN NOT NULL DEFAULT TRUE,
  
  PRIMARY KEY (chain_id, delegator_address)
);

CREATE INDEX idx_delegator_active ON delegator_registry (is_active) WHERE is_active = TRUE;
```

### 11.12 `rpc_call_cache`

Determinism backbone — every archive RPC call cached with raw response bytes.

```sql
CREATE TABLE rpc_call_cache (
  call_hash TEXT PRIMARY KEY,            -- sha256(method || canonical_params || block)
  method TEXT NOT NULL,
  params JSONB NOT NULL,
  block_number BIGINT,
  response_bytes BYTEA NOT NULL,
  response_hash TEXT NOT NULL,
  provider TEXT NOT NULL,
  cross_check_provider TEXT,
  cross_check_response_hash TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_rpc_cache_method_block ON rpc_call_cache (method, block_number);
```

### 11.13 `rpc_divergence_failures`

Captured when two RPC providers returned different responses for the same query.

```sql
CREATE TABLE rpc_divergence_failures (
  id BIGSERIAL PRIMARY KEY,
  method TEXT NOT NULL,
  params JSONB NOT NULL,
  block_number BIGINT,
  provider_a TEXT NOT NULL,
  response_a_bytes BYTEA NOT NULL,
  response_a_hash TEXT NOT NULL,
  provider_b TEXT NOT NULL,
  response_b_bytes BYTEA NOT NULL,
  response_b_hash TEXT NOT NULL,
  detected_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  resolved_at TIMESTAMPTZ,
  resolution_notes TEXT
);

CREATE INDEX idx_divergence_unresolved ON rpc_divergence_failures (detected_at) WHERE resolved_at IS NULL;
```

### 11.14 `reorg_events`

Audit log of detected chain reorganizations.

```sql
CREATE TABLE reorg_events (
  id BIGSERIAL PRIMARY KEY,
  chain_id BIGINT NOT NULL,
  detected_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  divergence_block BIGINT NOT NULL,
  depth INT NOT NULL,
  old_block_hashes TEXT[] NOT NULL,
  new_block_hashes TEXT[] NOT NULL,
  affected_event_count INT NOT NULL,
  notes TEXT
);
```

### 11.15 `reorg_mutations`

Audit trail for the limited mutation case (block_number/block_hash update on reorg).

```sql
CREATE TABLE reorg_mutations (
  id BIGSERIAL PRIMARY KEY,
  reorg_event_id BIGINT NOT NULL REFERENCES reorg_events(id),
  raw_event_id BIGINT NOT NULL REFERENCES raw_protocol_events(id),
  old_block_number BIGINT NOT NULL,
  old_block_hash TEXT NOT NULL,
  new_block_number BIGINT NOT NULL,
  new_block_hash TEXT NOT NULL,
  mutated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

### 11.16 Numeric precision

- `NUMERIC(78, 0)` for raw on-chain integer amounts (handles uint256 max ~10^77).
- `NUMERIC(38, 18)` for normalized amounts and prices. 38 digits at 18 decimal places = up to ~10^20 in whole units, comfortably above realistic LPT or ETH amounts.

### 11.17 Partitioning

**Not partitioned in v1.** Postgres handles low-millions-of-rows tables fine without partitioning. A separate migration to partition `raw_protocol_events` and `event_valuations` by month is provisioned for if/when row counts exceed ~50M.

### 11.18 Foreign keys

All cross-table references use FOREIGN KEY constraints. The slight write cost is justified by integrity guarantees.

---

## 12. Concurrency, Idempotency & Determinism

### 12.1 Worker concurrency model

**Single-instance per worker in v1.** Each long-running service runs as exactly one process. No claim mechanism, no `FOR UPDATE SKIP LOCKED`, no parallel insertion ordering questions.

This simplifies the determinism story: workers are sequential, the only concurrency boundary is between distinct services, and they only interact via Postgres with idempotent writes.

Horizontal scaling is deferred to v2. If/when needed, the migration to `FOR UPDATE SKIP LOCKED` is straightforward — the schema already supports it (no in-flight state held in process memory).

### 12.2 Idempotency contract

Every write path has a defined conflict key and conflict behavior:

| Write path | Conflict key | On conflict |
|---|---|---|
| Insert into `raw_protocol_events` | `(chain_id, tx_hash, log_index)` | DO NOTHING |
| Update `raw_protocol_events` block_number/block_hash on reorg | `id` | UPDATE; log to `reorg_mutations` |
| Insert into `decode_failures` | `(chain_id, tx_hash, log_index)` | DO NOTHING |
| Insert into `seeded_event_prices` | `(chain_id, tx_hash, log_index, asset)` | DO NOTHING (seed is one-shot) |
| Insert into `token_prices_by_block` | `(chain_id, asset, quote, block_number, source)` | DO NOTHING |
| Insert into `event_valuations` | `(event_id, valuation_version, asset)` | DO NOTHING + alert if values would have differed |
| Insert into `valuation_attempts` | `(event_id, valuation_version, asset, attempt_number)` | DO NOTHING |
| Insert into `stake_balances_by_block` | `(chain_id, delegator_address, block_number)` | DO NOTHING |
| Insert into `delegator_registry` | `(chain_id, delegator_address)` | UPDATE last_seen to GREATEST(stored, new) |
| Insert into `rpc_call_cache` | `call_hash` | DO NOTHING |
| Update `indexer_checkpoints` | `name` | UPDATE last_processed_block to GREATEST(stored, new) |

Two non-trivial cases:

**Determinism alert on `event_valuations` mismatch.** If a worker tries to insert a row whose key already exists and computed values differ from stored values: ON CONFLICT DO NOTHING applies, but a CRITICAL alert fires. This is the in-system determinism guard.

**GREATEST trick on checkpoints and registry.** Out-of-order processing (which can occur on retry-after-restart) doesn't regress the value. `last_processed_block = GREATEST(stored, new)` ensures monotonic forward progress.

### 12.3 Transaction boundaries

**Single transaction:**

- **Indexer batch:** all events in `[from, to]` + checkpoint advance. Atomic.
- **Reorg detection:** marking N rows non-canonical + `reorg_events` row + N `reorg_mutations` rows.
- **Single valuation:** valuation row + attempt row.
- **Single stake refresh:** stake row + delegator_registry update.

**Separate transactions (no atomicity required):**

- **RPC cache writes:** async, no consistency requirement.
- **Decode failure writes:** dead-letter is best-effort.

Crash safety: if any service crashes mid-transaction, the whole transaction rolls back. Restart resumes cleanly from the last committed state. No partial state exposure.

The valuator does its RPC reads **outside** the DB transaction (RPC calls are slow and the cache makes them retry-safe). The DB transaction wraps only writes. Worker crash mid-RPC = no DB state change = clean replay.

### 12.4 Replay determinism test

This is the load-bearing CI gate.

**Test contract:**

```
GIVEN:
  - A fixed snapshot of rpc_call_cache (committed test fixture)
  - A fixed seeded SQLite (committed test fixture)
  - Empty Postgres database

WHEN:
  - All migrations applied
  - Seed migrator run
  - Indexer run with from_block=X, to_block=Y (uses cache, never hits real RPC)
  - Reorg watcher run for the same range
  - Finality watcher run for the same range
  - Valuator run for valuation_version=v1
  - Stake-worker run for the same range

THEN:
  - Compute SHA256 of every table's contents (sorted by primary key, all columns)
  - Compare to committed expected hashes (expected_hashes.json)
  - Test passes IFF all hashes match
```

**Properties guaranteed:**

- Two runs with identical inputs produce byte-identical output.
- A drop-DB-and-replay produces byte-identical output.
- Refactoring code without changing logic doesn't change outputs.
- Bugs in pricing math, ABI decoding, or schema interpretation surface immediately.

**Properties NOT guaranteed:**

- Real-world correctness (the fixture might encode a bug).
- Performance.
- Behavior under conditions not represented in the fixture.

**Fixture coverage requirements** (committed in `tests/fixtures/`):

- A `Reward` event
- An `EarningsClaimed` event with both LPT and ETH portions
- A `Bond` event
- An `Unbond` event
- A `WinningTicketRedeemed` event
- A `NewRound` event (non-monetary)
- A `TranscoderUpdate` event (non-monetary)
- An event with `seeded_event_prices` coverage
- An event without seed coverage (forces on-chain pricing)
- All required RPC cache entries

**Fixture regeneration:** `livepeer-test regenerate-fixture --from-block N --to-block M` runs once against real RPC, captures into the cache, commits. Regeneration is a deliberate action, reviewed in PR.

**CI integration:** the determinism test runs on every PR. Failure = merge blocked.

### 12.5 Database unavailability policies

| Failure mode | Worker behavior |
|---|---|
| Postgres unreachable | Worker pauses, retries with backoff (1s, 5s, 30s, 5m, indefinite). WARN throughout, alert at 5m. No data loss because no work is consumed without successful commit. Workers never buffer in-process — if write fails, read doesn't happen. |
| Connection pool exhausted | CRITICAL alert. Pool size 10 per service × 5 services = 50 connections, well within Postgres defaults. Exhaustion implies a bug (long transaction, leaked connection). |
| Postgres data corruption | Out of v1 automation. Recovery: restore from pg_dump (§18) or full replay from RPC cache. |

---

## 13. RPC Architecture

### 13.1 Provider topology

| Role | Provider | Capability |
|---|---|---|
| Archive primary | Chainstack | Full Arbitrum archive, all historical state |
| Secondary | liveinfraspe (hosted HTTP RPC) | Recent state, all logs, no historical archive |

The terms "local" and "secondary" are used interchangeably in §13.2; both refer to the non-archive provider above. Earlier draft versions of this spec assumed a self-hosted Arbitrum Nitro node; the actual operational topology is two hosted HTTP RPCs (one archive, one not).

### 13.2 Routing matrix

Per-call-type routing, codified in code:

| Call type | Primary | Fallback | Cross-check |
|---|---|---|---|
| `eth_getLogs` (live, recent blocks) | local | archive | Optional sample (1% of batches) |
| `eth_getLogs` (backfill, historical) | archive | local | YES — both, compare |
| `eth_getBlockByNumber` (header) | local | archive | YES — both, compare hash |
| `eth_call` (historical state — pricing) | archive | NONE | NO (only archive can serve) |
| `eth_call` (recent state — staker) | local | archive | Optional |
| `eth_blockNumber` (chain head poll) | local | archive | NO |

Cross-check operates at **raw response level** — bytes compared, not derived values.

### 13.3 Retry, backoff, circuit breaker

**Per-call retry policy:**

- **Transient errors** (timeout, 429, 5xx, connection reset): retry with exponential backoff. Base 250ms, max 30s, up to 5 attempts.
- **Determinism-fatal errors** (response differs from cross-check, JSON parse error, schema mismatch): NO retry. Fail loudly, write `rpc_failures` row, alert.
- **Permanent errors** (block not available on this provider, archive depth exceeded): no retry on same provider. Try fallback once. If fallback also fails, fail loudly.

**Per-provider circuit breaker:** if error rate > 20% over the last 60 seconds, open circuit for 30 seconds (route everything to other provider). Half-open with one probe call after 30s.

**Hard rule:** indexer's main loop never retries the same `eth_getLogs(from, to)` more than 5 times in a single batch. After 5 failures, batch fails, checkpoint doesn't advance. Better to stall than to skip.

### 13.4 Rate limiting

**Token bucket per provider**, configured in YAML:

```yaml
rpc:
  archive:
    url: ${CHAINSTACK_RPC_URL}
    rate_limit_rps: 500
    burst: 1000
    max_concurrent: 50
  secondary:
    url: ${SECONDARY_RPC_URL}
    rate_limit_rps: 2000
    burst: 4000
    max_concurrent: 100
```

**Dynamic batch size:** indexer's `eth_getLogs` batch starts at 5000 blocks, halves on rate-limit error, doubles on success up to 10000. Hard cap at 10000 (Chainstack's documented per-call limit).

When rate-limited, callers wait — never error. Backfill paces itself to the limit.

### 13.5 RPC response caching

**Two-layer cache:**

1. **In-process LRU** (per service, ~100MB): hot blocks, current TWAP windows, recent rounds. Sub-millisecond hits. Killed on restart.
2. **Postgres `rpc_call_cache`** (§11.12): keyed by `(method, canonical_params_hash, block_number)`, raw response bytes preserved. Never evicted.

**Determinism guarantee:** every archive call at a fixed block returns the same response forever. Once cached, the call is never re-made.

**Cross-check at write time:** when both providers respond, both `response_hash`es stored. On cache hit, return cached bytes. On disagreement at write time, NEITHER is cached and `rpc_divergence_failures` row is written.

**Replay backbone:** drop everything except `rpc_call_cache` and `seeded_event_prices`, re-run from scratch — every other table populates identically. The cache IS the deterministic input.

**Size estimate:** at ~1KB/response × ~4M unique cached calls (over 4-year backfill + steady-state), ~4GB. Fine for Postgres.

### 13.6 RPC budget estimate

**One-time backfill (Feb 2022 → today):**

- Logs (`eth_getLogs` batched at 10K blocks): ~50K calls
- Block headers (reorg-watcher seed): not needed for backfill
- Historical pricing: ~200K archive calls (one observe + one Chainlink per unique event block)
- Stake-worker (Scope 2 — event-triggered only): ~100K archive calls

Total backfill: ~350K archive calls. At Chainstack's $0.0001-0.0005/call: $35-$175 one-time.

**Steady-state per day:**

- Live indexer: ~14K calls/day (mostly local node, free)
- Reorg watcher: ~430K block-hash reads/day (mostly local, batched)
- Pricing: ~200/day
- Stake worker: ~100/day

Daily archive load: ~300 calls. Single-digit dollars per month.

### 13.7 RPC budget alerting

Tracked as a Prometheus gauge. Alerts:

- 80% of monthly Chainstack budget used → WARN
- 95% → CRITICAL

Budget tracking is informational; the system does not auto-throttle on budget signals.

---

## 14. API Surface

### 14.1 Integration model

v1 ships a dedicated `livepeer-api` service. It is a standalone Axum HTTP server
backed by the replayable Postgres state produced by the indexer / valuator /
staker pipeline. Operators may place it behind a reverse proxy, but the spec no
longer assumes an “existing API bolt-on” deployment shape.

### 14.2 Polling-only model

v1 exposes **poll-based** endpoints only. No webhooks, no Kafka, no push. To make polling efficient:

- Responses are JSON over ordinary HTTP GET.
- Clients poll the relevant endpoint families directly.
- OpenAPI documentation is exposed by the API service itself:
  - `GET /openapi.json`
  - `GET /docs`
  - `GET /docs/`

### 14.3 Endpoints

#### 14.3.1 Events

```http
GET /events/{id}
GET /events
  ?from_block=        &to_block=
  ?contract=          (e.g. BondingManager, TicketBroker, Governor)
  ?event_name=        (e.g. WinningTicketRedeemed, Reward, Bond)
  ?event_type=        (legacy alias for event_name)
  ?from_address=      &to_address=     &address=    (any-role match)
  ?asset=             (LPT | ETH)
  ?with_valuations=   (default false; joins event_valuations rows inline)
  ?sort=              (block_asc | block_desc | amount_usd_desc)   ← whitelist only
  ?limit=             (default 100, max 1000)
  ?cursor=            (opaque, returned by prior response)
  ?include_tentative=false
  ?include_reorged=false
```

Defaults to `is_canonical = TRUE` and `finality = 'finalized'`. `include_tentative=true` relaxes the finality filter; `include_reorged=true` relaxes the canonical filter (audit/forensic only).

**Pagination is cursor-based.** Response carries `next_cursor` (null if no more). The cursor is an opaque string encoding the last `(block_number, log_index)` pair for the chosen sort order — stable under append. `offset`-based paging is not supported (it would skip events arriving mid-walk on an append-only store).

**`sort` is a whitelist.** Only the listed values are accepted; arbitrary user-supplied SQL ordering is rejected. Default is `block_asc`.

**`with_valuations=true`** inlines the matching `event_valuations` rows under the `valuations` key (an array, since multi-asset events have multiple). The default is `false` to keep responses small for callers that don't need them.

#### 14.3.2 Valuations

```http
GET /events/{id}/valuation?version=v1
GET /valuations?from=&to=&version=&asset=
```

Returns valuation rows. Multi-asset events return an array of valuations.

Terminal failure rows are returned from the same endpoints. For those rows:

- `status` is one of `failed_missing_pool`, `failed_missing_oracle`, `failed_sequencer_outage`
- `native_usd_price` is `null`
- `amount_usd` is `null`
- `pricing_chain` / `source` / `pricing_method` still explain how the outcome was derived

#### 14.3.3 Prices

```http
GET /prices/{asset}/{quote}/block/{block}
GET /prices/{asset}/{quote}/latest
GET /prices/{asset}/{quote}/range?from_block=&to_block=
```

The `range` endpoint lazily computes prices for blocks not yet in `token_prices_by_block`, caches the result, and returns. Bounded by reasonable `to_block - from_block` spans (rejected if the range exceeds a configured max to prevent abuse).

#### 14.3.4 Stake

```http
GET /stake/{delegator}/block/{block}
GET /stake/{delegator}/range?from_block=&to_block=
```

Returns the stake snapshot at or before the requested block, with `staleness_blocks` field indicating how stale the answer is. v1 implements Scope 2 (event-triggered + EarningsClaimed reconciliation), so staleness is bounded by the delegator's event activity.

#### 14.3.5 Gateways / TicketBroker

```http
GET /gateways/{gateway}/balance/latest
GET /gateways/{gateway}/balance/block/{block}
GET /gateways/{gateway}/balance/history?from_block=&to_block=&limit=
GET /gateways/{gateway}/claimants/block/{block}
GET /gateways/{gateway}/claimants/history?from_block=&to_block=&limit=
GET /gateways/{gateway}/flows?from_block=&to_block=&limit=
GET /gateways/{gateway}/payouts?from_block=&to_block=&limit=&semantics=net|gross
GET /gateways/{gateway}/recipients?from_block=&to_block=&limit=&semantics=net|gross
GET /gateways/{gateway}/summary?days=&semantics=net|gross
GET /gateways/{gateway}/analytics/summary?days=&semantics=net|gross
```

These endpoints expose the TicketBroker sender model under the “gateway” name:

- exact sender balance state (`getSenderInfo()` / `isUnlockInProgress()`)
- materialized sender balance history (`gateway_balances_by_block`)
- claimant reserve history (`gateway_claimants_by_block`)
- materialized funding/payout flow ledger (`gateway_flows`)
- recipient leaderboards and higher-level payout analytics

`payouts`, `recipients`, and analytics routes expose explicit payout semantics:

- `net`:
  - includes `ticket_redeemed`
  - includes `reserve_claimed`
  - excludes paired `reserve_transfer`
- `gross`:
  - includes `ticket_redeemed`
  - includes `reserve_claimed`
  - includes paired `reserve_transfer`

Default is `net`.

#### 14.3.6 Transcoders

```http
GET /transcoders/{transcoder}/params/latest
GET /transcoders/{transcoder}/params/block/{block}
GET /transcoders/{transcoder}/params/history?from_block=&to_block=&limit=
GET /transcoders/{transcoder}/lifecycle/latest
GET /transcoders/{transcoder}/lifecycle/block/{block}
GET /transcoders/{transcoder}/lifecycle/history?from_block=&to_block=&limit=
GET /transcoders/{transcoder}/profile/block/{block}
GET /transcoders/{transcoder}/delegators/block/{block}
```

These are historical convenience views over transcoder configuration and stake
context:

- reward cut / fee share history
- activation / deactivation lifecycle history
- point-in-time combined transcoder profile
- delegator set snapshots at a block

Whenever fee policy is exposed, the API must return both:

- `fee_share_percent` — the protocol-perspective share routed to delegators
- `fee_cut_percent` — the operator-perspective share kept by the transcoder

These fields sum to 100 by construction. This is required for legacy API
parity because the old service stored fee semantics from the operator
perspective.

#### 14.3.7 Operational

```http
GET /backfills/status
GET /health
GET /metrics
```

Status endpoint returns indexer + derived-state progress. Health endpoint is a
simple liveness check. `/metrics` exposes Prometheus-format API metrics on the
same HTTP listener as the API itself.

#### 14.3.8 Aggregations

```http
GET /aggregations/events
  ?contract=          &event_name=
  ?bucket=            (day | week | month)
  ?from=              &to=             (ISO date YYYY-MM-DD or block number)
  ?address=           &from_address=   &to_address=
  ?asset=             (LPT | ETH)
  ?metric=            (count | sum_amount_native | sum_amount_usd | avg_amount_usd)
  ?valuation_version= (defaults to current default version)
  ?tz=                (IANA tz; default UTC) — controls bucket-edge alignment
```

Returns a time series of buckets:

```json
{
  "bucket": "day",
  "tz": "UTC",
  "results": [
    { "bucket_start": "2026-04-01", "count": 12345, "sum_amount_usd": "..." },
    { "bucket_start": "2026-04-02", ... }
  ],
  "next_cursor": null
}
```

Backed by `GROUP BY date_trunc(bucket, block_timestamp)` over `raw_protocol_events` joined with `event_valuations` (when `metric` includes USD or `_amount_usd`). Bounded by a configured max-bucket count to prevent unbounded scans. Replaces 4 legacy summary routes (daily/weekly/monthly payouts + ticket-count timeseries).

`metric=count` requires no valuation join — fast for ticket-count timeseries.

#### 14.3.9 Governance

```http
GET /governance/proposals
  ?status=            (active | succeeded | executed | defeated | all)
  ?limit=             ?cursor=
GET /governance/proposals/{proposal_id}
GET /governance/proposals/{proposal_id}/votes
```

Convenience layer over `/events`. Each proposal row joins `ProposalCreated` with the `ProposalExecuted` row (if any) and includes a per-support-side vote-weight tally aggregated from `VoteCast`. Saves callers a 3-event-type query.

Underlying data is still queryable via raw `/events?contract=Governor&event_name=...` for forensic use.

#### 14.3.10 Legacy-parity surfaces (TD-017)

The legacy API families reintroduced in v1.9 are:

```http
GET /orchestrators
GET /orchestrators/{address}
GET /gateways
GET /gateways/{address}/profile
GET /payouts/leaderboard
GET /payouts/summary/daily/{date}
GET /payouts/summary/weekly/{date}
GET /payouts/summary/monthly/{date}
GET /rewards/leaderboard
GET /rewards/summary/daily/{date}
GET /rewards/summary/weekly/{date}
GET /rewards/summary/monthly/{date}
GET /tickets/timeseries/daily
GET /reports/payouts.csv
GET /reports/rewards.csv
GET /reports/gateway-payouts.csv
GET /governance/votes
```

These routes restore the previously deferred CSV report, metadata/profile, and
`job_type` filter use cases. Their implementation may combine deterministic
on-chain state, deterministic rollups, and explicit non-deterministic overlays
(ENS / operator-curated labels), but the deterministic tables remain subject to
the replay contract from §12.4.

### 14.4 Response format

JSON. All numeric values that may exceed JavaScript's safe integer range (block numbers, raw amounts, USD values) are serialized as **strings**, not numbers. This is mandatory.

Standard error envelope:

```json
{
  "error": {
    "code": "missing_finalized",
    "message": "Event 12345 exists but is not yet finalized",
    "context": { "event_id": 12345, "current_finality": "tentative" }
  }
}
```

### 14.5 Auth

No application-layer auth is built into `livepeer-api` itself in v1. Operators
may place it behind reverse-proxy auth, private networking, or other
environment-specific access controls.

### 14.6 Rate limiting

Not implemented in v1. Existing API service may already have rate limiting; if not, this is a v2 concern.

---

## 15. Deployment Topology

### 15.1 Pattern

**Docker Compose, single host.** The current steady-state production shape is:

- `postgres`
- `livepeer-daemon follow`
- `livepeer-api`
- optional `livepeer-alert-bot`

One-shot tools run out-of-band:

- `livepeer-orchestrator` (`bootstrap`, `replay`, `migrate-only`)
- `livepeer-seed-migrator`

Prometheus + Grafana may run externally.

### 15.2 docker-compose.prod.yml shape

```yaml
services:
  postgres:
    image: postgres:17
    volumes:
      - livepeer-valuation-pgdata:/var/lib/postgresql/data
    restart: unless-stopped
  
  livepeer-daemon:
    image: livepeer-valuation-system:latest
    command: livepeer-daemon --env-config config/env/prod.yaml follow --max-start-lag-blocks 50000
    depends_on:
      - postgres
    restart: unless-stopped
    ports:
      - "9107:9107"  # /metrics + /health
  
  livepeer-api:
    image: livepeer-valuation-system:latest
    command: livepeer-api --env-config config/env/prod.yaml
    depends_on:
      - postgres
    restart: unless-stopped
    ports:
      - "8080:8080"  # HTTP + /metrics

  livepeer-alert-bot:
    image: livepeer-valuation-system:latest
    command: livepeer-alert-bot --env-config config/env/prod.yaml

volumes:
  livepeer-valuation-pgdata:
```

`livepeer-orchestrator` and `livepeer-seed-migrator` are invoked as one-shot
tools via `docker compose run`.

### 15.3 Metrics endpoints

Metrics topology in the shipped runtime:

- `livepeer-daemon`: `:9107` serves `/metrics` and `/health`
- `livepeer-api`: `:8080` serves normal API routes plus `/metrics` and `/health`

Network-level access control (firewall, security groups, reverse proxy) should
restrict who can reach those ports.

### 15.4 Backups

The Postgres volume is backed up daily via `pg_dump`. See §18.

### 15.5 v2 migration path

If/when scale or operational needs require Kubernetes: each service is already a stand-alone binary with no host coupling. Migration is mechanical (Helm chart + StatefulSet for Postgres).

---

## 16. Configuration & Secrets

### 16.1 Three-layer config model

**Layer 1 — Static config** (committed to repo, PR-reviewed):

`config/arbitrum.yaml` — chain ID, contract addresses (Controller-resolved at boot), pool addresses, oracle addresses, default valuation version, batch sizes, retry policies, observation cardinality requirements.

**Layer 2 — Environment-specific config** (committed, environment-tagged):

`config/env/{dev,staging,prod}.yaml` — RPC URL env-variable names, Postgres
connection-string env-variable name, log levels, and alerting env-variable
names.

**Layer 3 — Secrets** (never committed):

`.env` files (dev) or environment variables / external secret manager (prod):

- `CHAINSTACK_RPC_URL` — full URL with API key
- `POSTGRES_USER`, `POSTGRES_PASSWORD`, `POSTGRES_DB`
- `TELEGRAM_BOT_TOKEN`, `TELEGRAM_CHAT_ID`

`.env` files have file mode `0600`, owned by the service user. No commits, no logs.

### 16.2 Boot-time validation

Every service validates configuration at startup and refuses to start on any failure:

- All RPC URLs reachable (HTTP 200 on a `eth_chainId` probe).
- Postgres reachable, expected schema version present (compare against migration manifest).
- All ABI files load successfully and hashes match `contract_abi_registry.abi_hash`.
- Controller-resolved contract addresses match expected pattern (proxy addresses unchanged).
- Chainlink aggregator returns sane data on a test `latestRoundData()` call.
- Pool observation cardinality at current block ≥ required for configured TWAP window.

Boot fails loudly. No "started successfully but actually broken" state.

### 16.3 Secret management evolution

v1 uses `.env`. If the operational environment introduces HashiCorp Vault, AWS Secrets Manager, or similar, integration is straightforward — services read from environment variables, and a side-car or init container can populate them from any source.

---

## 17. Observability

### 17.1 Structured logging (active day one)

- **Format:** JSON to stdout, captured by `docker compose logs` or external log shipper.
- **Required fields:** `timestamp`, `service`, `level`, `event`, plus context-specific fields (`block_number`, `tx_hash`, `event_id`, `valuation_version`, `error_type`).
- **Conventions:** human-readable `event` strings, machine-parseable structured fields.

### 17.2 Prometheus metrics catalog

**Counters:**

- `events_indexed_total{contract, event_name}`
- `events_valued_total{version, asset, status}`
- `decode_failures_total{contract}`
- `rpc_calls_total{provider, method, status}`
- `reorgs_detected_total{depth_bucket}`
- `rpc_divergence_total{method}`

**Gauges:**

- `indexer_lag_blocks{name}` — checkpoint vs chain tip
- `valuator_pending_events{version}` — unvalued backlog
- `rpc_provider_circuit_state{provider}` — 0=closed, 1=half-open, 2=open
- `db_connection_pool_used`, `db_connection_pool_max`
- `rpc_monthly_budget_used_pct{provider}`

**Histograms:**

- `rpc_call_duration_seconds{provider, method}`
- `event_processing_duration_seconds{stage}`
- `valuation_attempts_per_event`

### 17.3 Grafana dashboards (build last per Q9.E)

Dashboard plan:

- **Indexer Health:** lag, throughput, decode failure rate.
- **Valuation Health:** backlog, success rate by status, retry depth.
- **RPC Health:** provider latency, error rate, divergence count, budget burn.
- **Determinism Watch:** reorg events, mismatch alerts, RPC cache hit rate.

### 17.4 Tracing

**Out of v1 scope.** OpenTelemetry distributed tracing is valuable but adds complexity. Singleton workers + structured logs with correlation IDs are sufficient for v1.

---

## 18. Backup & Recovery

### 18.1 Backup strategy

**Daily logical backup** via `pg_dump`:

- Run nightly via cron / scheduled task.
- Compressed (`-Fc`) and encrypted at rest.
- Retained for 30 days.
- Stored offsite (S3 or equivalent).
- Restore procedure documented in runbook.

**Optional WAL archiving** for point-in-time recovery within the last 7 days. Adds operational complexity; may or may not be enabled depending on RPO requirements.

### 18.2 The deep recovery path

The killer recovery path is **drop the DB and replay from `rpc_call_cache` + seeded SQLite**. This is the deterministic backbone — as long as we have these inputs preserved, every other table can be regenerated bit-identically without any new RPC calls.

This means:

- Corrupted `event_valuations` → fixable by re-running `livepeer-valuator backfill`.
- Corrupted `raw_protocol_events` → fixable by re-running indexer (uses cache, free).
- Corrupted whole DB → fixable by replaying seed migrator + indexer + valuator + stake-worker in sequence.

### 18.3 Recovery objectives

| Path | RTO | RPO |
|---|---|---|
| pg_dump restore | < 1 hour | 24 hours worst case |
| pg_dump + WAL | < 1 hour | 1 hour worst case (if WAL enabled) |
| Cache replay | < 24 hours | Zero (cache is authoritative) |

Pairing: pg_dump for fast recovery, cache replay for deep recovery. Belt-and-suspenders.

### 18.4 Cache preservation

The `rpc_call_cache` table is included in pg_dump. **It is the most important table to back up** — losing it means re-running RPC calls (cost) without breaking determinism (the calls are pure reads). Losing the cache + the original SQLite seed = losing the deterministic input set.

The seed SQLite is a separate artifact, archived alongside backups for the lifetime of the system.

---

## 19. Runbook Outline

The runbook lives at `docs/RUNBOOK.md` in the repository, maintained alongside code, PR-reviewed.

### 19.1 Sections

1. **Daily operations** — health checks, dashboard interpretation, common log patterns.
2. **Backfill procedures** — running the seed migration, backfilling a date range, re-valuing under a new version.
3. **Recovery procedures** — pg_dump restore, deep replay, partial recovery of a corrupted single table.
4. **Failure response** — what each alert means, response procedure, escalation criteria.
5. **Schema changes** — migration authoring, testing, deployment.
6. **ABI updates** — when Livepeer upgrades a contract, procedure to add a registry row, recover decode failures, regenerate the determinism fixture.

### 19.2 Critical procedures (excerpt)

#### 19.2.1 Adding a new ABI version

When Livepeer upgrades the BondingManager (or any other tracked contract):

1. Identify the upgrade block from the Controller's `SetContractInfo` event.
2. Fetch the new implementation's ABI from Arbiscan.
3. Compute its sha256 hash.
4. Insert a new row into `contract_abi_registry` with appropriate `from_block` (the upgrade block).
5. Update the previous row's `to_block` to `upgrade_block - 1`.
6. Restart all services — boot validation will accept the new registry.
7. Run `livepeer-indexer recover-decode-failures` to retry any rows that failed under the old ABI.
8. Regenerate the determinism test fixture if it covered any blocks affected.

#### 19.2.2 Responding to `failed_rpc_divergence`

Two providers returned different responses for the same query:

1. Pull the row from `rpc_divergence_failures` — examine `response_a` vs `response_b`.
2. Manually re-query both providers; compare to recorded responses.
3. Determine which provider was correct (compare to a third source if available — block explorer, alternate RPC).
4. Investigate why the wrong provider was wrong (cache poisoning, sync lag, malicious response).
5. If transient: mark `resolved_at`, document, do not auto-retry.
6. If systemic: switch primary provider until trust is re-established.

#### 19.2.3 Responding to determinism violation alert

A worker tried to insert a valuation that conflicts with stored values:

1. Pull the failing `valuation_attempts` row — examine the diff.
2. Determine cause: pricing logic changed without version bump? RPC cache poisoning? Race condition?
3. If logic changed: revert the change OR bump the version (if the change is intentional).
4. If cache issue: invalidate cache entry, alert investigation continues.
5. Never silently update `event_valuations` to match new logic — that violates the determinism contract.

---

## 20. Out of Scope (v1)

Each item below is explicitly out of scope for v1. Each has a v2 entry point documented for future planning.

| Item | v2 entry point |
|---|---|
| Multi-chain (Ethereum, Base, Optimism, etc.) | Add `chains:` array to config, instantiate per-chain indexers, schema needs no migration (chain_id already on every table). |
| Mainnet (L1) Livepeer events | Add Ethereum mainnet as a second chain via the multi-chain mechanism. |
| Stake balance per-`NewRound` fan-out | Add scheduled task triggered by `NewRound` to fan out `pendingStake` calls for all `delegator_registry WHERE is_active = TRUE`. Schema unchanged. |
| **Tax-lot accounting / cost-basis (v2 PRIORITY)** | New `cost-basis-worker` service reading from `event_valuations`, producing `tax_lots` table. Lot tracking, FIFO/LIFO, gain attribution. |
| **Frontend dashboard (v2 PRIORITY)** | Separate frontend project consuming the API. Not part of this codebase. |
| Manual price overrides | A `manual_price_overrides` table the valuator checks before on-chain pricing, with strict audit trail. **Determinism is compromised at this boundary.** |
| Sub-finality (real-time) valuations | On-demand pricing endpoint computing tentative valuations without persisting; explicit warning flags. |
| Push-based event distribution (webhooks, Kafka) | Subscription model + outbox pattern + retry policy. Plain polling over the documented HTTP endpoints (§14.2) handles v1 needs. |
| Horizontal worker scaling | Switch to `FOR UPDATE SKIP LOCKED` claim mechanism. Schema already supports it. |
| OpenTelemetry tracing | Add tracing spans to RPC calls, DB queries, worker iterations. Connect to a backend (Jaeger, Tempo, etc.). |
| Real-time orchestrator/broadcaster metadata | Re-derive from on-chain events. Possibly a separate `metadata-worker`. |
| Governance event valuation (proposals, votes) | Currently indexed but not valued (non-monetary). v2 might add weighted-vote analytics. |

---

## 21. v2 Roadmap

Priorities expressed by stakeholders during spec development:

### 21.1 High priority

1. **Tax-lot accounting / cost-basis tracking.** Builds on top of `event_valuations`. Produces lot inventory per delegator, FIFO/LIFO accounting, realized vs unrealized gain attribution.
2. **Frontend dashboard.** Consumes the v1 API. Visualizes orchestrator earnings, delegator P&L, protocol-wide metrics.

### 21.2 Medium priority (driven by need)

3. **Stake balance per-`NewRound` fan-out.** Densifies `stake_balances_by_block`. ~$100/year RPC cost; deferred from v1 to control cost.
4. **Multi-chain support.** Ethereum mainnet first (for L1 LPT context), then potentially Base / Optimism if Livepeer expands.
5. **Push distribution.** Webhooks or Kafka for consumers needing low-latency event signals.

### 21.3 Opportunistic

6. **Horizontal worker scaling.** Only if v1 single-instance becomes a bottleneck. Currently no projection it will.
7. **OpenTelemetry tracing.** Operational nice-to-have.
8. **Manual price overrides.** Only if a real audit case demands it. Compromises determinism — use sparingly.

---

## 22. Open Data Items

Items that block precise implementation but not architectural design. Resolved during implementation kickoff:

| ID | Item | Source | Status |
|---|---|---|---|
| Q-OD-1 | NUMBER precision spot-check on `reward.total_tokens` vs on-chain `Reward` event amount | Sample SQLite reward + RPC comparison | Pending |
| Q-OD-2 | `transaction_id` uniqueness check (multi-reward / multi-payout transactions) | SQL query against SQLite | Pending |
| Q-OD-3 | Structure of SQLite `events.payload` column (3-5 sample rows) | SQLite query | Pending |
| Q-OD-4 | `block_cursors` actual contents (per-event-type bounds) | SQLite query | Pending |
| Q-OD-5 | Self-hosted Nitro node spec (version, archive flag, log retention) | Operator | Pending |
| Q-OD-6 | Existing API service details (framework, routes, DB topology, auth) | Operator | Pending |
| Q-OD-7 | Bond/Rebond on-chain event semantics (which field carries the LPT amount) | Arbiscan ABI inspection | Pending |
| Q-OD-8 | Chainlink ETH/USD aggregator address on Arbitrum (current and historical) | `cast call` confirmation | Pending |
| Q-OD-9 | Pool observation cardinality history for LPT/WETH pool | `cast call` at sample blocks | Pending |
| Q-OD-10 | L2 Sequencer Uptime Feed address | Chainlink docs / cast confirmation | Pending |

None of these block the spec. All are filled in during implementation kickoff and updated in implementation tickets, not in this spec document.

---

## 23. Master Requirements List

Numbered and immutable. Each requirement is testable. References to themes / sections in parentheses.

### Foundational

1. Chain: Arbitrum One (chain_id 42161). Single chain in v1. (§1.3, §20)
2. Canonical event key: `(chain_id, tx_hash, log_index)`. (§2, §11.4)
3. Event block number is the source of truth for valuation timing. (§2.2)
4. Raw events are immutable except for reorg-induced block_number/block_hash mutation, fully audited. (§2.1, §9.3)
5. Valuations are immutable per `(event_id, valuation_version, asset)`. (§2.4, §11.7)
6. Determinism: nuke-and-replay must produce byte-identical output. Enforced by CI test. (§2.5, §12.4)
7. No CoinGecko or external pricing API in primary pricing or audit trail. (§2.7, §7.4)
8. Foundry `cast` is the canonical debugging tool; pricing must be reproducible by `cast call ... --block N`. (§4.2)

### Tech stack

9. Rust + Postgres + Tokio + Alloy + SQLx + Axum. (§4.1)
10. No rindexer. Indexer built in-house using Alloy primitives. (§4.3, §3.1)
11. All Rust crates pinned to exact versions. (§4.4)
12. sqlx-cli with raw SQL up/down migrations. (§11.1)

### Schema

13. Schema supports multi-asset events via `event_valuations.asset` in primary key. (§6.8, §11.7)
14. Three event categories: LPT-valued, ETH-valued, non-monetary. `is_valuable` flag. (§6.1)
15. `pricing_chain` JSONB on `event_valuations` for full provenance. (§7.5, §11.7)
16. `block_hash` on `raw_protocol_events` and `token_prices_by_block` for reorg detection. (§11.4, §11.6)
17. `is_canonical` BOOLEAN on `raw_protocol_events`; valuator filters on this. (§11.4)
18. `is_valuable` BOOLEAN NOT NULL on `raw_protocol_events`, set at decode time. (§11.4, §6.9)
19. `finality` field supports tentative → l1_posted → finalized lifecycle. (§9.1, §11.4)
20. `contract_abi_registry` table with per-block-range mapping and `strict_decode` flag. (§5.4, §11.3)
21. `decode_failures` dead-letter table. (§10.2.2, §11.9)
22. `seeded_event_prices` overlay table. (§8.4, §11.5)
23. `rpc_call_cache` permanent cache as determinism backbone. (§13.5, §11.12)
24. `rpc_divergence_failures` for cross-check failures. (§11.13)
25. `reorg_events` and `reorg_mutations` audit tables. (§9.3, §11.14, §11.15)
26. `valuation_attempts` audit trail. (§10.3, §11.8)
27. `stake_balances_by_block` table with bonded_principal + nullable pending stake/fees. (§11.10)
28. `delegator_registry` derived from Bond events. (§11.11)
29. `NUMERIC(38, 18)` for normalized, `NUMERIC(78, 0)` for raw on-chain. (§11.16)
30. No partitioning in v1. (§11.17)
31. Strict FKs everywhere. (§11.18)
32. Five migration discipline rules: immutable, no production downs, destructive migrations gated, idempotent, comment block. (§11.1)

### Pricing

33. Primary v1 method: TWAP 30-min × Chainlink ETH/USD. (§7.3)
34. Spot pricing kept as `v0_debug_spot` for diagnostics. (§7.1)
35. Method A (lpt_usdc) excluded — pool TVL is $108. (§7.4)
36. Pool observation cardinality verified at backfill window start. (§7.3.2)
37. L2 Sequencer Uptime Feed checked at every event block. (§7.3.4)
38. Chainlink 24h staleness threshold + WARN at >4h. (§7.3.3)
39. Chainlink post-block state convention documented. (§7.3.5)
40. Mandatory `answeredInRound >= roundId` check. (§7.3.3)
41. Reward events valued as income at LPT/USD market price; mint-vs-transfer distinction preserved in `event_name`. (§6.3, §7)

### Contracts

42. Livepeer addresses resolved via Controller at boot, not hardcoded. (§5.2)
43. Listening on Proxy addresses; targets used only for ABI selection. (§5.3)
44. ABI hash verification at boot. (§5.5)

### SQLite seed

45. SQLite is a price overlay, not an event mirror. (§8.5)
46. Seed migrator is one-shot; trusted, no re-verification pass. (§8.6)
47. Per-event-type cutoff tracking from `block_cursors`. (§8.2.4, §8.6)

### Indexer / RPC

48. Indexer behavior uniform across history; seed is valuation-layer overlay. (§8.5)
49. Atomic batch commit pattern: events + checkpoint advance in single transaction. (§12.3)
50. Idempotent inserts (`ON CONFLICT DO NOTHING`) on every persistence path. (§12.2)
51. Chainstack archive RPC primary; self-hosted Nitro secondary. (§13.1)
52. Cross-check applies to `eth_getLogs` and block hashes only; historical state is single-source archive. (§13.2)
53. Routing matrix per-call-type, codified in code. (§13.2)
54. Retry / backoff / circuit breaker per provider. (§13.3)
55. Token-bucket rate limits + dynamic batch size. (§13.4)
56. Two-layer RPC cache (in-process LRU + permanent Postgres). (§13.5)

### Reorg / finality

57. Two-tier finality: index tentative, value finalized. (§9.1)
58. Reorg watcher walks 7,500 blocks every 30s with backoff/heightened modes. (§9.2)
59. Owned reorg watcher service (not delegated to a framework). (§3.1, §9.2)
60. Reorg-induced block_number/block_hash mutation allowed and audited. (§9.3, §11.15)
61. Never mutate valuations; un-finalization triggers a new version + critical alert. (§9.5)
62. API defaults to finalized only; opt-in for tentative. (§14.3.1)

### Workers

63. Single-instance per worker in v1. Horizontal scaling deferred. (§12.1)
64. Five service binaries + one-shot migrator. (§3.1)
65. Services communicate via Postgres only; no shared queue, no IPC. (§3.3)
66. Stake balance v1 — Scope 2: principal from flows + event-triggered pending stake. (§11.10)
67. EarningsClaimed triggers stake refresh. (§11.10)
68. Stake API exposes `staleness_blocks` field. (§14.3.4)
69. Stake balance per-NewRound fan-out deferred to v2. (§20)

### Failure handling

70. Granular failure statuses with per-status retry policy. (§10.3)
71. Tiered decode strictness: critical-events allowlist halts indexer; others go to dead-letter. (§10.2)
72. `failed_rpc_divergence` is a first-class status; never auto-retried. (§10.3)
73. Determinism violation alert on event_valuations conflict with differing values. (§10.5, §12.2)
74. Idempotent re-runnable backfill commands. (§10.4)

### Operations

75. Docker Compose single-host deployment; Prometheus + Grafana on external pre-provisioned host. (§15.1)
76. Three-layer config + .env secrets. (§16.1)
77. Boot-time validation of RPC, DB, ABIs, addresses, oracles, pool cardinality. (§16.2)
78. Telegram for notifications; no pager service in v1. (§10.6)
79. Prometheus metrics emission active day one; Grafana dashboards built last. (§17.2, §17.3)
80. Daily pg_dump + cache as deep recovery. (§18.1, §18.2)
81. Six-section runbook. (§19.1)

### API

82. Standalone `livepeer-api` service is the HTTP surface for v1. (§14.1)
83. Poll-only API with JSON GET endpoints; no push/webhook model in v1. (§14.2)
84. Numeric values that exceed JS safe integer range serialized as strings. (§14.4)

---

## 24. Acceptance Criteria

The system is ready for v1 production declaration when **all** of the following are demonstrated:

### 24.1 Functional

- [ ] Indexer backfills all events from Livepeer Arbitrum genesis to current finalized head, with zero `decode_failures` on critical-events allowlist.
- [ ] Every backfilled event has `chain_id`, `tx_hash`, `log_index`, `block_number`, `block_hash`, `block_timestamp`, `contract_name`, `event_name`, `event_signature`, `is_valuable`, `finality`, `is_canonical`, `abi_hash_used` populated correctly.
- [ ] Seed migrator imports all `payout` and `reward` rows from SQLite into `seeded_event_prices` without loss.
- [ ] Valuator produces `event_valuations` rows for every finalized, valuable event under the applicable valuation version: `v1_lpt_weth_twap_30min_x_chainlink_eth` or `v1_degraded_spot_pre_cardinality`.
- [ ] `EarningsClaimed` events produce two rows in `event_valuations` (LPT + ETH) per version.
- [ ] Terminal valuation failures still produce `event_valuations` rows, with nullable `native_usd_price` / `amount_usd` and `status IN ('failed_missing_pool', 'failed_missing_oracle', 'failed_sequencer_outage')`.
- [ ] Stake-worker produces `stake_balances_by_block` rows for every stake-touching event, with both `bonded_principal` and `pending_stake`/`pending_fees` populated.
- [ ] API exposes all endpoints listed in §14.3.
- [ ] Seed/canonical event cross-check pass completes with a discrepancy report (`livepeer-test cross-check`). For every `(tx_hash, log_index)` present in both the SQLite `events` table and `raw_protocol_events` after backfill, decoded field values match. Discrepancies must be triaged and either resolved or explicitly accepted before v1 sign-off. (Resolves TD-004.)

### 24.2 Determinism

- [ ] CI test (§12.4) passes — full replay produces byte-identical output to expected hashes.
- [ ] Test fixture covers Reward, EarningsClaimed (multi-asset), Bond, Unbond, WinningTicketRedeemed, NewRound (non-monetary), TranscoderUpdate (non-monetary), seeded event, non-seeded event.
- [ ] Drop-DB-and-replay from a real production snapshot of `rpc_call_cache` + seed produces identical output to the original (manually verified).

### 24.3 Reliability

- [ ] System survives crash of any service mid-batch with no data loss or corruption (verified by chaos test).
- [ ] System resumes cleanly from checkpoint after restart (verified by integration test).
- [ ] System handles RPC provider failure (one provider down) without indexing stoppage.
- [ ] Reorg watcher detects and handles a synthetic reorg correctly (verified by test fixture).

### 24.4 Failure handling

- [ ] All status values in §10.1 are exercised by at least one integration test.
- [ ] Critical-events allowlist halt is triggered by a planted decode failure on a critical event (verified by test).
- [ ] `failed_rpc_divergence` is triggered by a planted disagreement between providers (verified by test).
- [ ] Determinism violation alert fires when forcibly inserting a conflicting valuation.

### 24.5 Operational

- [ ] All services expose `/metrics` reachable from external Prometheus host.
- [ ] All services emit structured JSON logs with required fields.
- [ ] Boot-time validation catches: missing RPC URL, wrong Postgres schema version, mismatched ABI hash, changed Controller-resolved target, non-functional Chainlink oracle.
- [ ] pg_dump + restore cycle recovers a full database in < 1 hour.
- [ ] Cache replay recovers a full database from empty in < 24 hours.
- [ ] Runbook documents all six required sections.

### 24.6 Documentation

- [ ] This spec document is up-to-date.
- [ ] README explains: how to build, how to deploy, how to run a backfill, how to migrate the seed.
- [ ] RUNBOOK covers daily ops, backfill procedures, recovery, failure response, schema changes, ABI updates.
- [ ] DETERMINISM.md explains the test contract and how to regenerate fixtures.
- [ ] Open data items (§22) are resolved or explicitly deferred with justification.

---

## Appendix: Key Design Decisions Log

This appendix summarizes the substantive decisions made during spec development, for future readers.

| # | Decision | Rationale |
|---|---|---|
| 1 | Rust + Alloy + Postgres, no rindexer | Determinism requires owning every line of indexer code. rindexer is "brand new" and reorg handling is undocumented. |
| 2 | TWAP 30-min as v1, not spot | LPT/WETH pool is $443-656K liquidity — spot reads can be off by 5-15% due to thin pool manipulation. TWAP is the only defensible primary. |
| 3 | LPT/USDC pool excluded | $108 in liquidity is not a price source. |
| 4 | Two-tier finality (index tentative, value finalized) | Accounting use case can absorb 30-min latency; correctness benefits are large. |
| 5 | Single-instance workers, no claim mechanism | Determinism story is cleaner; load profile doesn't require horizontal scaling. |
| 6 | RPC cache as permanent table | The deterministic input set must survive a database wipe. Cache + seed = enough to rebuild everything. |
| 7 | Trusted SQLite seed, no re-verification | Operator confirmed the data is verified; spending engineering on re-verification doesn't pay off. |
| 8 | Cross-check at raw-bytes level, not derived values | Any derived computation (price math) introduces non-determinism risk; raw-byte cross-check is the integrity floor. |
| 9 | Tiered decode strictness | Strict halt on critical events prevents silent stake-worker drift; permissive on non-critical keeps the firehose flowing. |
| 10 | Stake-worker in v1 (Scope 2) | Principal-only is misleading; full fan-out is expensive; event-triggered + EarningsClaimed reconciliation is the right middle ground. |
| 11 | Telegram, no pager | Operational preference; alerting infrastructure is last priority anyway. |
| 12 | Standalone API service | Keeps the v1 runtime simple and lets the HTTP surface evolve with the replayable data model without depending on a legacy service boundary. |
| 13 | Poll-only API | No external category-5 consumers identified that require push. Ordinary JSON polling is sufficient for v1. |

---

**END OF SPECIFICATION v1.8**
