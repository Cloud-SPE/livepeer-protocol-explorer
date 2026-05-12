---
title: Old API Parity + Rollup / Dimension Layer
status: planned
opened: 2026-05-05
revised: 2026-05-05
owner: codex
links:
  - source-old-api: ../../../../livepeer-backend-rs
  - tracker: ../tech-debt-tracker.md
  - related: td-016-gateway-backfill-operability.md
  - spec: ../../product-specs/v1-livepeer-indexer.md
---

## Problem

The legacy `livepeer-backend-rs` HTTP API (Axum 0.7, SQLite, port 4000)
exposes 17 endpoints across orchestrators, broadcasters/gateways, payouts,
rewards, and treasury. Several of those endpoints have no equivalent in
Livepeer Protocol Explorer, and a few that look equivalent return semantically
different values because the legacy API stores Livepeer's `feeShare` in
operator perspective (orch's keep) while the new API exposes it raw
(delegators' share).

Porting the missing surface naively would either leave query-time scans of
millions of rows in the hot path or duplicate the legacy SQLite shape inside
Postgres. Neither is acceptable.

This plan ports the missing endpoints onto the new event + valuation pipeline
by adding (a) a small dimension layer for entity-centric reads, split by
determinism boundary so on-chain-derived state stays under the existing
replay contract while ENS / operator-curated state sits explicitly outside
it; and (b) a small rollup layer for analytics-heavy queries. It also
patches the existing transcoder endpoints to expose both fee perspectives
so dashboards can migrate without silent semantic drift.

## Goal

1. Provide every legacy endpoint either as-is on the new pipeline or with a
   documented superset, with zero loss of compute capability.
2. Eliminate the fee-share semantic ambiguity by exposing both
   `fee_share_percent` (protocol) and `fee_cut_percent` (operator) everywhere
   transcoder cuts surface.
3. Add rollup tables only where measured query cost demands it
   (per-orchestrator daily totals, leaderboards, ticket timeseries).
4. Add dimension tables only where data must merge external state with
   on-chain state — and partition them so the deterministic and
   non-deterministic columns live in **separate tables**.
5. Keep the determinism contract intact for the on-chain portion: deterministic
   tables derive entirely from `raw_protocol_events` + `event_valuations` +
   `rpc_call_cache` and respect `reorg_mutations`.

## Non-goals

- Replacing or rewriting the existing low-level endpoints (`/events`,
  `/valuations`, `/prices/*`, `/stake/*`, `/transcoders/.../params`,
  `/gateways/.../balance`). Those keep their current shapes and only gain
  additive fields.
- Building a generic "medallion" framework or rollup engine. Each rollup
  table is a focused worker.
- Per-delegator current-stake computation by full fan-out
  (`pendingStake(delegator, currentRound)` over all 5,863 known delegators).
  Stake totals come from per-orch `transcoderTotalStake(orch)` instead.
- Auth at the application layer. Admin endpoints are protected by network
  binding to an internal port and the production reverse proxy.
- Bug-for-bug parity with the legacy fee semantics. The legacy API's
  `commission` field is the orch's keep but its underlying derivation
  inverted the on-chain `feeShare` value at storage time. The new API
  exposes both perspectives explicitly and labels them honestly.
- Bringing ENS resolution and operator-curated overrides under the
  determinism contract. They are explicitly external state — see
  Determinism contract section.

## Determinism contract

The repository's load-bearing replay contract is: drop everything except
`rpc_call_cache` and the seed, run replay, get byte-identical output. This
plan preserves that contract for on-chain-derived state and explicitly
excludes external (ENS, operator) state, with the boundary enforced by
table separation.

### Table classification

| Table | Status | Writer | Replay-verified? |
|---|---|---|---|
| `orchestrator_profile` (on-chain columns only) | **deterministic** | `livepeer-staker` (extended) | yes |
| `broadcaster_profile` (on-chain columns only) | **deterministic** | `livepeer-staker` (extended) | yes |
| `orch_payouts_daily` | **deterministic** | `livepeer-rollups` | yes |
| `orch_rewards_daily` | **deterministic** | `livepeer-rollups` | yes |
| `tickets_daily` | **deterministic** | `livepeer-rollups` | yes |
| `orchestrator_ens` | **external** | `livepeer-enricher` (new) | no |
| `broadcaster_ens` | **external** | `livepeer-enricher` (new) | no |
| `name_avatar_overrides` | **external** | admin endpoints | no |
| `broadcaster_classifications` | **external** | admin endpoints + seed | no |

API read paths join the deterministic profile table with its `_ens` sibling
and the override table at request time, applying
`COALESCE(override.display_name, ens.ens_name)` and equivalent for avatars.

### Acceptance criteria for deterministic writers

These fall on the `livepeer-staker` extension and the `livepeer-rollups`
workers. They are explicit acceptance criteria for those PRs:

1. **All RPC calls go through the existing `rpc_call_cache` path.** New code
   must use the same cached `eth_call` plumbing the staker already uses for
   `pendingStake` and `getSenderInfo`. A parallel uncached path silently
   breaks replay determinism even though the table looks well-shaped.
2. **`as_of_block` and `as_of_round` derive from the triggering event's
   block, never wall-clock or "current head."** A `NewRound` event at block
   `B` triggers a refresh whose snapshot is stamped `as_of_block = B`. The
   value the worker happened to wake up at is irrelevant.
3. **Profile and rollup rows are monotonic by `last_event_id`.** Each
   deterministic writer carries a `last_event_id BIGINT NOT NULL` column on
   its target table (named `last_event_id` for profile rows;
   `source_max_event_id` for the existing rollup convention). Upserts use
   `ON CONFLICT … DO UPDATE … WHERE excluded.last_event_id >
   target.last_event_id`. Because `raw_protocol_events.id` is monotonic by
   `(block_number, log_index)`, this enforces deterministic ordering even
   when a single profile row is touched by multiple events in the same batch.

The replay-determinism CI (`scripts/run-determinism-replay.sh`) extends to
cover the new deterministic tables. External tables are explicitly excluded
from that comparison.

### Persistence requirements (external tables)

External tables are not part of the determinism contract and are **not
verified by replay**. Operationally, deployments must preserve them through
their own backup/restore lifecycle:

| Table | Initial state on fresh deploy | Ongoing persistence |
|---|---|---|
| `broadcaster_classifications` | Populated by the seed migration (10 known AI addresses with `source='seed'`) | Operator changes via admin endpoints must be backed up out-of-band (Postgres dumps or equivalent) |
| `name_avatar_overrides` | Empty | Operator changes via admin endpoints must be backed up |
| `orchestrator_ens`, `broadcaster_ens` | Empty; refilled lazily by `livepeer-enricher` against L1 mainnet | Optional to back up — the enricher will rebuild from L1, but a backup avoids a cold-start lookup wave on a fresh deployment |

A fresh-DB rerun produces empty external tables until the enricher catches
up and operators re-apply overrides / classifications from backup. This is
intentional: the boundary between "rebuilt from on-chain truth" and
"depends on operator state" is the same boundary as deterministic vs.
external tables.

## Spec impact

This plan reintroduces three items the v1 spec consciously dropped from
scope (`docs/product-specs/v1-livepeer-indexer.md:49`). Treating that
honestly requires three localized edits to the spec, landed in the Phase 0
PR alongside the foundational field addition. **No separate amendment doc;
no external gating.** The spec is a living document with a changelog
header, and TD-017 is the change.

Current spec version is **v1.8** (`Document version: 1.8`,
`Changes since v1.7 (2026-05-05)` is the most recent header entry). The
edits below land as v1.9.

### Edits owned by this plan

1. **§14 dropped-items line** (`v1-livepeer-indexer.md:49`): flip the three
   "consciously dropped" items to "in scope as of v1.9 — see TD-017":
   - CSV report endpoints
   - Orchestrator / gateway metadata endpoints
   - `job_type` (ai/transcoding) filter
2. **New §14.3.x entry** documenting the reintroduced endpoint families:
   leaderboards (`/payouts/leaderboard`, `/rewards/leaderboard`), per-period
   summaries (`/payouts/summary/{daily|weekly|monthly}/{date}`), ticket
   timeseries (`/tickets/timeseries/daily`), CSV reports
   (`/reports/{payouts,rewards,gateway-payouts}.csv`), entity profile
   endpoints (`/orchestrators`, `/gateways`), and the votes list
   (`/governance/votes`).
3. **Changelog header**: bump `Document version` to `1.9` and add a
   `Changes since v1.8 (YYYY-MM-DD)` entry summarizing the scope
   reinstatement and pointing at TD-017.

## Background

### The fee-share inversion (critical context)

`BondingManager.sol:368` distributes ticket fees as:

```solidity
uint256 delegatorsFees = MathUtils.percOf(_fees, earningsPool.transcoderFeeShare);
uint256 transcoderCommissionFees = _fees.sub(delegatorsFees);
```

So on-chain `feeShare` (raw, scaled by `PERC_DIVISOR = 1_000_000`) is the
**fraction routed to delegators**, and the orchestrator keeps `1 − feeShare`.

The legacy API's `cut_calculator.rs` inverts this on the way into storage:

```rust
CutType::Fee(cut) => {
    let cut_percent_display = 100.00 - (cut / 10000_f64);
    let cut_percent = (100.00 - (cut / 10000_f64)) * 0.01;
    ...
}
```

So legacy `orchestrator.fee_cut = 0.8` corresponds to on-chain raw
`feeShare = 200_000` (i.e. 20% to delegators, 80% to orch). Legacy
`commission = face_value × fee_cut` is correct *because* of the inversion
at storage time.

Reward semantics are not inverted: on-chain `rewardCut` already represents
the fraction the orch keeps, and both the legacy and new APIs treat it that
way.

The new API currently exposes only the protocol-perspective
`fee_share_percent` from `crates/livepeer-api/src/routes/transcoders.rs:316`.
A consumer porting from old → new would silently see "fee cut" change from
80 to 20 for the same orch with no schema change, no error, no warning.
Phase 0 of this plan adds the operator-perspective `fee_cut_percent` field
alongside the protocol-perspective one to remove that footgun.

### Pipeline state at plan time

Live counts on the local replica, refreshed 2026-05-05 (chain `42161`,
range 2022-02-15 → 2026-05-04):

| Source | Rows |
|---|---|
| `raw_protocol_events.WinningTicketRedeemed` | 298,178 |
| `raw_protocol_events.Reward` | 157,061 |
| `raw_protocol_events.TranscoderUpdate` | 1,057 |
| `raw_protocol_events.NewRound` | 1,699 |
| `raw_protocol_events` (Governor) | **2,975** (essentially complete vs. legacy ≈ 2,908) |
| `event_valuations` | 2,374,842 |
| `stake_balances_by_block` | 81,976 (5,863 delegators across 256 orchs) |
| `gateway_flows` | **4,000 and climbing** (TD-016 fix actively running in runtime) |
| `gateway_claimants_by_block` | **1,000 and climbing** |
| `gateway_balances_by_block` | **598 and climbing** |

Backfill cost estimate for the rollup workers (single pass, all-history):

| Worker | Source rows | Estimated runtime |
|---|---|---|
| `rollup_orch_payouts_daily` | ~298K events | 2–10 min |
| `rollup_orch_rewards_daily` | ~157K events | 1–5 min |
| `rollup_tickets_daily` | ~298K events | 1–3 min |

Total rollup backfill: < 20 min. Safe to do live with the same code path as
follow mode (checkpoint reset = backfill, no separate batch binary).

## Locked decisions

| Topic | Decision |
|---|---|
| Naming | Functional table names, no `gold_` / `silver_` / `bronze_` prefix. |
| Migration numbering | Forward-only, sequential. Plan starts at `028_*`. **Re-check current head before any PR is opened**; if other PRs land first, renumber to slot in cleanly. |
| Migration discipline | Forward-only per `migrations/README.md` and `RUNBOOK.md`. `.down.sql` files exist for local development only and are never run in production. |
| Worker pattern | Same shape as existing services — checkpoint row in `indexer_checkpoints`, idempotent `INSERT … ON CONFLICT DO UPDATE`, single code path for backfill and live. |
| Determinism boundary | Enforced at the table level (see Determinism contract). Deterministic columns and external columns never share a table. |
| Home for the on-chain dimension writer | **Extend `livepeer-staker`.** It already has the RPC handles, the cached `eth_call` path, the checkpoint pattern, and the reorg subscription. Adding `transcoderTotalStake` reads is structurally identical to its existing `pendingStake` work. No new crate. |
| Home for the ENS / external writer | **New `livepeer-enricher` crate, standalone binary only.** Not embedded in the daemon supervisor. Isolates the L1 mainnet RPC dependency from the rest of the L2 pipeline. Writes only to `*_ens` tables. |
| Worker deployment shape | **Standalone binaries by default for new crates.** `livepeer-enricher` and the three `livepeer-rollups` workers ship as their own binaries with their own process boundaries. Daemon embedding is optional only for the rollup workers (matching the existing `livepeer-staker` dual-pattern); the enricher is never daemon-embedded. Rationale: TD-016 demonstrated that piling additional work onto the daemon supervisor has real operability consequences, and isolation of L1-dependent work is non-negotiable. |
| Rollup data source | `raw_protocol_events` directly. **Not** `gateway_flows` — that table is materialized by `livepeer-staker` (now actively backfilling under TD-016's bounded-phase refactor) and is the wrong coupling for a deterministic rollup. Reading source events directly removes the cross-worker dependency: rollup correctness no longer depends on the staker's flow phase having reached the same block. |
| Reorg correctness | Rollup workers subscribe to `reorg_mutations`. Affected `(day, orch, version, broadcaster_kind)` cells are recomputed in full on mutation. |
| Valuation versions | `valuation_version` is part of every rollup table's primary key. New versions materialize alongside existing rows. No auto-rebuild on version bump. |
| Total stake | `BondingManager.transcoderTotalStake(orch)` RPC call per orch via the cached path, refreshed event-driven on `NewRound` + `Bond` + `Unbond` + `Rebond` + `TransferBond`. No subgraph. No per-delegator fan-out. |
| Override semantics | `name_avatar_overrides` is a separate table. Read path is `COALESCE(override.display_name, ens.ens_name)`. ENS truth and overrides are tracked independently so divergence is queryable. |
| Broadcaster classification | DB-driven via `broadcaster_classifications`. Seeded with the 10 known AI gateway addresses. Missing rows default to `'transcoding'`. Operator-editable through admin endpoints. |
| Fee semantics in API | Both `fee_share_percent` (protocol) and `fee_cut_percent` (operator) exposed everywhere transcoder cuts appear. They sum to 100 by construction. |
| Commission math | `commission_native = face_value × (1 − fee_share_raw / 1_000_000)` |
| Reward math | `orch_tokens = total_tokens × (reward_cut_raw / 1_000_000)` (no inversion) |
| Job-type schema | `broadcaster_kind` is part of the rollup PK: `(chain_id, day_utc, orchestrator_address, valuation_version, broadcaster_kind)`. No column duplication. |
| Auth | None at app layer. Admin endpoints bind to an internal port (e.g. `127.0.0.1:9091`); production reverse proxy enforces auth. Bind address pinned in config; runbook calls out the boundary. |
| CSV format | Strict column order preserved. Two renames: `txn_fee` → `transaction_fee`, `txn_fee_usd` → `transaction_fee_usd`. New columns appended. |
| Pagination | Cursor only across all list endpoints (opaque base64-encoded sort-key tuple). No offset, no page numbers. `limit` is a single page-size knob (default 100, max 1000). |
| Response envelope | Optional `meta` block on backfill-affected and rollup endpoints only. Fields: `chain_id`, `valuation_version`, `coverage`, `as_of_block` / `as_of_round`, `usd_coverage`, optional `stale_data_warning`. Existing low-level endpoints unchanged. |
| HTTP method for downloads | `GET /reports/*.csv` with query params (not POST + form). Streaming response. |

## Cross-cutting design

### Response envelope (`meta`)

Attached only where it adds information. Shape:

```jsonc
{
  "data": [ /* … */ ],
  "meta": {
    "chain_id": 42161,
    "valuation_version": "v1_lpt_weth_twap_30min_x_chainlink_eth",
    "coverage": {
      "backfill_complete": false,
      "last_processed_block": 269298263,
      "last_processed_at": "2026-05-05T08:13:40Z",
      "domain": "governor"
    },
    "as_of_block": 459170000,
    "as_of_round": 4321,
    "usd_coverage": { "rows_priced": 94, "rows_total": 100 },
    "stale_data_warning": "Governor history backfill ~38% complete; vote totals are partial."
  }
}
```

All `meta.*` fields are optional. Endpoints attach only what's relevant.

| Endpoint group | Fields |
|---|---|
| `/governance/proposals`, `/governance/votes` | `coverage` (domain=`"governor"`) |
| `/orchestrators` (list + single) | `as_of_block`, `as_of_round`, `coverage` if enricher behind |
| `/gateways` (list + single profile) | `as_of_block` |
| `/payouts/leaderboard`, `/payouts/summary/*`, `/rewards/leaderboard`, `/rewards/summary/*`, `/tickets/timeseries/daily` | `valuation_version`, `usd_coverage`, `coverage` if rollup worker behind |
| `/backfills/status` (existing) | unchanged — already the global view |
| `/events`, `/valuations`, `/prices/*`, `/stake/*`, `/transcoders/.../params`, `/gateways/.../balance` | unchanged — needed info already inline on row shapes |
| CSV download endpoints | response headers (`X-Valuation-Version`, `X-Backfill-Complete`) instead of JSON envelope |

### Cursor pagination

Cursor is opaque base64 of the sort-key tuple, identical convention to the
existing `/events` endpoint. Sort keys:

| Endpoint | Sort key |
|---|---|
| `GET /orchestrators` | `(total_stake DESC, address ASC)` |
| `GET /gateways` | `(latest_deposit DESC, address ASC)` |
| `GET /payouts/leaderboard` | `(sum_commission_usd DESC, orchestrator_address ASC)` |
| `GET /rewards/leaderboard` | `(sum_orch_tokens_usd DESC, orchestrator_address ASC)` |
| `GET /governance/votes` | `(block_number DESC, log_index DESC)` |
| `GET /orchestrators/{addr}/tickets/latest` | `(block_number DESC, log_index DESC)` |
| `GET /gateways/{addr}/tickets` | `(block_number DESC, log_index DESC)` |

Summary endpoints (`/payouts/summary/{daily,weekly,monthly}/{date}`) and the
ticket timeseries are date-bounded singletons — no pagination.

### Worker pattern recap

Every rollup and dimension writer follows the identical shape used by
`livepeer-indexer`, `livepeer-valuator`, `livepeer-staker`:

1. Look up `last_processed_*` from `indexer_checkpoints` keyed by worker name
2. Fetch a bounded batch of source rows after the checkpoint, ordered ASC
3. Process each row; write target rows with `INSERT … ON CONFLICT DO UPDATE`
4. Advance checkpoint with `GREATEST(existing, new)` (monotonic)
5. Sleep, then loop

Reorg path: subscribe (poll) `reorg_mutations` for new rows since the
worker's last checked mutation id. For each mutation, recompute the affected
`(day_utc, orchestrator_address, valuation_version, broadcaster_kind)` cells
in full. Cheap because reorg depth is bounded.

## Phases

Each phase is a standalone PR. Cross-phase dependencies are noted explicitly.

### Phase 0 — Foundations (small, low risk, unblocks everything)

**Migrations** (verify head before merge; renumber if needed)

- `028_create_broadcaster_classifications.up.sql`
  - Columns: `chain_id BIGINT NOT NULL`, `address TEXT NOT NULL`,
    `kind TEXT NOT NULL CHECK(kind IN ('ai','transcoding'))`,
    `source TEXT NOT NULL`, `notes TEXT`, `updated_at TIMESTAMPTZ`
  - Primary key: `(chain_id, address)`
  - Seed (in same migration or `028_seed_classifications.sql`): the 10
    legacy AI broadcaster addresses with `source='seed'`
  - Read convention everywhere: `COALESCE((SELECT kind FROM
    broadcaster_classifications WHERE chain_id=$1 AND address=$2),
    'transcoding')`
- `029_create_name_avatar_overrides.up.sql`
  - Columns: `chain_id`, `address`, `display_name TEXT`, `avatar_url TEXT`,
    `notes TEXT`, `updated_at TIMESTAMPTZ`, `updated_by TEXT`,
    `ens_name_at_override_time TEXT`
  - Primary key: `(chain_id, address)`

**API patch (additive only)**

- `crates/livepeer-api/src/routes/transcoders.rs`: add
  `fee_cut_percent: String` to `TranscoderParamsRow`
  (= `100 - fee_share_percent`)
- Surfaces on: `/transcoders/{addr}/params/latest`,
  `…/params/block/{block}`, `…/params/history`,
  `…/profile/block/{block}`
- Update OpenAPI spec; document both fields with explicit semantics
- Reward fields unchanged

**Spec edits (land in this PR)**

- `docs/product-specs/v1-livepeer-indexer.md:49` — flip the three
  "consciously dropped" items as described in the Spec impact section
- Add new §14.3.x entry listing the reintroduced endpoint families
- Bump `Document version` to `1.9` and add a `Changes since v1.8 (YYYY-MM-DD)` changelog header entry

**Definition of done**

- Migrations apply cleanly forward
- `/transcoders/.../params/latest` returns `fee_cut_percent` with the
  correct inversion against the on-chain raw value (verify against
  `0xd00354656922168815fcd1e51cbddb9e359e3c7f` → expect 80)
- `fee_cut_percent + fee_share_percent` rounds to 100.0 on every row
- OpenAPI regenerated and committed
- Spec edits merged in same commit as the field addition
- No existing fields or values changed

**Risks** — None significant. Additive only.

### Phase 1 — Dimension layer (split by determinism boundary)

**Migrations**

- `030_create_orchestrator_profile.up.sql` *(deterministic columns only)*
  - `chain_id`, `address`, `total_stake NUMERIC(38,18)`,
    `latest_fee_cut_percent NUMERIC(10,4)`,
    `latest_reward_cut_percent NUMERIC(10,4)`,
    `latest_fee_share_percent NUMERIC(10,4)`,
    `is_active BOOLEAN`, `last_lifecycle_event_at TIMESTAMPTZ`,
    `as_of_block BIGINT`, `as_of_round BIGINT`,
    `last_event_id BIGINT NOT NULL`,
    `service_uri TEXT`, `updated_at TIMESTAMPTZ`
  - PK: `(chain_id, address)`
  - Upsert rule: `ON CONFLICT (chain_id, address) DO UPDATE … WHERE
    excluded.last_event_id > orchestrator_profile.last_event_id` (monotonic
    by triggering event ID; see Determinism contract acceptance criterion 3)
- `031_create_broadcaster_profile.up.sql` *(deterministic columns only)*
  - `chain_id`, `address`, `latest_deposit NUMERIC(38,18)`,
    `latest_reserve NUMERIC(38,18)`, `unlock_in_progress BOOLEAN`,
    `as_of_block BIGINT`, `last_event_id BIGINT NOT NULL`,
    `updated_at TIMESTAMPTZ`
  - PK: `(chain_id, address)`
  - Upsert rule: same monotonic guard as `orchestrator_profile`
- `032_create_orchestrator_ens.up.sql` *(external)*
  - `chain_id`, `address`, `ens_name TEXT`, `ens_avatar_url TEXT`,
    `ens_last_resolved_at TIMESTAMPTZ`
  - PK: `(chain_id, address)`
- `033_create_broadcaster_ens.up.sql` *(external)*
  - `chain_id`, `address`, `ens_name TEXT`, `ens_avatar_url TEXT`,
    `ens_last_resolved_at TIMESTAMPTZ`
  - PK: `(chain_id, address)`

**Worker A — extend `livepeer-staker` (deterministic writer)**

Adds two new responsibilities to the existing staker, alongside its current
delegator-stake and gateway-balance work:

- New checkpoint entries: `staker_orch_profile`, `staker_gateway_profile`
- Source: `raw_protocol_events` filtered to:
  - For `orchestrator_profile`: `(NewRound, Bond, Unbond, Rebond,
    TransferBond, TranscoderUpdate, TranscoderActivated,
    TranscoderDeactivated, ServiceURIUpdate)`
  - For `broadcaster_profile`: `(DepositFunded, ReserveFunded, Withdrawal,
    Unlock, ReserveClaimed, UnlockCancelled)`
- For each event:
  - **Orch profile**: call `BondingManager.transcoderTotalStake(orch)`
    through the existing cached `eth_call` path; merge with the latest
    `TranscoderUpdate` decode for `latest_*_percent`; merge with latest
    lifecycle event for `is_active` and `last_lifecycle_event_at`.
    Stamp `as_of_block` and `as_of_round` from the **triggering event's
    block**.
  - **Gateway profile**: call `TicketBroker.getSenderInfo(gateway)`
    through the cached path; stamp `as_of_block` from the triggering event.
- On `NewRound`, refresh all known orchs (≈ 256 cached RPC calls per round
  boundary, ≈ once per ~22h)
- Reorg path: subscribe `reorg_mutations`; on mutation affecting any
  triggering event, recompute the affected profile row(s)

**Acceptance criteria for the staker extension PR** (from Determinism
contract):

1. Every new RPC call routes through the same cached `eth_call` path the
   staker already uses
2. `as_of_block` / `as_of_round` come from the triggering event's
   `block_number` and `round` derivation, never `chain.latest_block()` or
   wall-clock

**Worker B — new `livepeer-enricher` crate (external writer, standalone binary)**

Writes only to `orchestrator_ens` and `broadcaster_ens`. Never touches
`*_profile` or override tables. **Ships as its own binary; never embedded
in the daemon supervisor** (per the Locked decisions row on worker
deployment shape).

- Two checkpoint entries: `enricher_orchestrator_ens`,
  `enricher_broadcaster_ens`
- Three input sources:
  1. **Lazy resolve on first sighting** — when a new address appears in
     `orchestrator_profile` or `broadcaster_profile` and has no row in the
     corresponding `_ens` table, resolve `[address].addr.reverse` on L1
     mainnet
  2. **L1 ENS event watcher** — subscribe to `NameChanged` and
     `TextChanged(node, "avatar", _)` on the public ENS resolver(s),
     refresh affected rows immediately
  3. **TTL refresh sweep** — every 30 days, re-resolve rows whose
     `ens_last_resolved_at` is older than the threshold
- L1 RPC failures are isolated here. If the L1 provider is down, this
  worker degrades; the rest of the pipeline (L2-only) is unaffected.
- Add a circuit breaker that halts ENS reads after N consecutive failures
  and exposes a metric.
- Outside the determinism contract. Replay verification skips
  `*_ens` tables.

**API endpoints (new)**

- `GET /orchestrators?cursor=…&limit=…&active_only=…`
  - Returns list sorted by `(total_stake DESC, address ASC)`
  - Each row: `address`, `display_name` (`COALESCE(override, ens)`),
    `avatar_url` (same), `total_stake`, `fee_cut_percent`,
    `fee_share_percent`, `reward_cut_percent`, `is_active`, `service_uri`,
    `as_of_block`, `as_of_round`
  - `meta.chain_id`, `meta.as_of_block`, `meta.coverage` if writer behind
- `GET /orchestrators/{address}` — same row shape, single record
- `GET /gateways?cursor=…&limit=…`
- `GET /gateways/{address}/profile` — new sub-resource (existing
  `/gateways/{addr}/balance/*` keeps current shape)

Read-path joins:

```
SELECT p.*, e.ens_name, e.ens_avatar_url, o.display_name, o.avatar_url
  FROM orchestrator_profile p
  LEFT JOIN orchestrator_ens   e USING (chain_id, address)
  LEFT JOIN name_avatar_overrides o USING (chain_id, address)
 WHERE p.chain_id = $1
 ORDER BY p.total_stake DESC, p.address ASC
```

**Stake-approximation note**

`transcoderTotalStake` is exact at the call's block. It does not provide
historical "total stake at block X 6 months ago." No legacy endpoint needs
historical totals; if a future endpoint does, that requires either per-orch
periodic snapshotting or the deferred per-delegator fan-out worker.

**Definition of done**

- All four tables populated for current head (256 orchs, 51 gateways)
- ENS resolver verified end-to-end for at least one orch and one gateway
- `total_stake` for the top 10 orchs within ±0.5% of on-chain
  `transcoderTotalStake` at the same block
- Override + ENS COALESCE behaviour verified via test fixture
- All four list/single endpoints return data with cursor pagination working
- Replay determinism CI passes on `orchestrator_profile` and
  `broadcaster_profile`; explicitly skips `*_ens` and override tables

**Risks**

- L1 mainnet RPC dependency lives only in `livepeer-enricher`. The L2
  pipeline must remain healthy when L1 is degraded. Verify with a fault
  injection test or documented runbook procedure.
- `transcoderTotalStake` semantics at round boundaries: the value can shift
  meaningfully on `NewRound` as compounded rewards register. The
  `as_of_block` / `as_of_round` columns make this transparent to consumers.

**Dependencies** — Phase 0 (`name_avatar_overrides` exists for the
COALESCE read path).

### Phase 2 — Payout rollup

**Migration**

- `034_create_orch_payouts_daily.up.sql`
  - `chain_id`, `day_utc DATE`, `orchestrator_address TEXT`,
    `valuation_version TEXT`,
    `broadcaster_kind TEXT NOT NULL CHECK(broadcaster_kind IN ('ai','transcoding'))`,
    `ticket_count BIGINT`,
    `sum_face_value_native NUMERIC(38,18)`,
    `sum_face_value_usd NUMERIC(38,18)`,
    `sum_commission_native NUMERIC(38,18)`,    -- orch keep
    `sum_commission_usd NUMERIC(38,18)`,
    `sum_delegators_share_native NUMERIC(38,18)`,
    `sum_delegators_share_usd NUMERIC(38,18)`,
    `distinct_gateways INT`,
    `usd_rows_priced BIGINT`,
    `source_max_event_id BIGINT`,
    `updated_at TIMESTAMPTZ`
  - PK: `(chain_id, day_utc, orchestrator_address, valuation_version,
    broadcaster_kind)`
  - Indexes:
    - `(orchestrator_address, day_utc DESC)`
    - `(day_utc DESC, sum_commission_usd DESC NULLS LAST)` — leaderboard
- Verify covering index exists for `TranscoderUpdate` lookup by
  `(contract_name, event_name, to_address, block_number DESC)`; add if not
  present.

**New crate `livepeer-rollups` (standalone binaries; daemon embedding optional)**

Each rollup worker (`rollup_orch_payouts_daily`, and the Phase 3 workers
`rollup_orch_rewards_daily` + `rollup_tickets_daily`) ships as its own
binary. Daemon embedding is optional only for these workers (matches the
existing `livepeer-staker` dual-pattern); default deployment runs them as
separate processes. They're DB-only — no RPC budget impact on the daemon —
but operability isolation per TD-016's lesson is preferred.

`rollup_orch_payouts_daily` worker:

1. Read `raw_protocol_events` rows where `id > source_max_event_id`,
   `event_name = 'WinningTicketRedeemed'`, `is_canonical = TRUE`,
   `finality = 'finalized'` (configurable to include tentative)
2. For each row:
   - Look up `event_valuations` for `(event_id, valuation_version)`
   - Look up most recent `TranscoderUpdate` for `to_address` at-or-before
     `block_number` to get `fee_share_raw`. Cache last-seen value per orch
     in-process for the batch.
   - Look up `broadcaster_kind` via
     `COALESCE(broadcaster_classifications.kind, 'transcoding')`
   - Compute `fee_cut_fraction = 1 − (fee_share_raw / 1_000_000)`
   - `commission_native = amount_normalized × fee_cut_fraction`
   - `delegators_share_native = amount_normalized × (1 − fee_cut_fraction)`
   - USD values via `event_valuations.amount_usd × fee_cut_fraction` and
     the inverse
3. Group accumulated values by `(day_utc, to_address, valuation_version,
   broadcaster_kind)`, `INSERT … ON CONFLICT DO UPDATE` accumulating sums
   and incrementing counters
4. Advance checkpoint to highest `event_id` in the batch
5. On `reorg_mutations` for any consumed event, recompute the affected
   `(day_utc, orchestrator_address, valuation_version, broadcaster_kind)`
   cells fully

**Edge cases**

- Ticket redeemed before the orch's first `TranscoderUpdate` event →
  `fee_cut_fraction = 1.0` (orch keeps 100%, matching old API default)
- Ticket without `event_valuations` row → USD columns left NULL; row
  counted in `ticket_count` but excluded from `usd_rows_priced`
- `to_address` NULL on a ticket event → skip and log; should not occur

**API endpoints (new)**

- `GET /payouts/leaderboard?from=YYYY-MM-DD&to=YYYY-MM-DD&job_type=ai|transcoding|both&sort=commission_usd|ticket_count|face_value_usd&cursor=…&limit=…`
  - Replaces old `POST /api/payout/report`
  - `job_type` filter is now a clean WHERE clause against the rollup PK
  - `meta.valuation_version`, `meta.usd_coverage`, `meta.coverage` if behind
- `GET /payouts/summary/daily/{YYYY-MM-DD}?job_type=…`
- `GET /payouts/summary/weekly/{YYYY-MM-DD}?job_type=…`
  (Mon–Sun containing date)
- `GET /payouts/summary/monthly/{YYYY-MM-DD}?job_type=…`
  (full calendar month)

**Definition of done**

- Migration applied; worker running; checkpoint advances to head
- Spot-check: 5 random orchs on 3 random days — sum from rollup matches sum
  from `raw_protocol_events + event_valuations` direct query
- Spot-check: legacy `payout` totals for
  `0xd00354656922168815fcd1e51cbddb9e359e3c7f` for 2025-09 vs new
  `orch_payouts_daily.sum_commission_native` for same orch/month — must
  agree (allowing for legacy float drift)
- All four endpoints documented in OpenAPI; all return data
- Reorg recomputation tested with a synthesized `reorg_mutations` row
- Replay determinism CI passes: drop `orch_payouts_daily`, reset
  checkpoint, rerun → byte-identical output

**Risks**

- Backfill scan over ~298K events with per-row `TranscoderUpdate` lookup
  could be slow without the covering index. Verify before promoting.
- Reorg recompute correctness — needs an integration test that simulates
  a ticket event moving between days

**Dependencies** — Phase 0 (`broadcaster_classifications`).

### Phase 3 — Reward rollup + ticket timeseries

**Migrations**

- `035_create_orch_rewards_daily.up.sql`
  - `chain_id`, `day_utc`, `orchestrator_address`, `valuation_version`,
    `reward_event_count BIGINT`,
    `sum_total_tokens NUMERIC(38,18)`,
    `sum_total_tokens_usd NUMERIC(38,18)`,
    `sum_orch_tokens NUMERIC(38,18)`,         -- orch keep
    `sum_orch_tokens_usd NUMERIC(38,18)`,
    `sum_delegators_tokens NUMERIC(38,18)`,
    `sum_delegators_tokens_usd NUMERIC(38,18)`,
    `usd_rows_priced BIGINT`,
    `source_max_event_id BIGINT`,
    `updated_at TIMESTAMPTZ`
  - PK: `(chain_id, day_utc, orchestrator_address, valuation_version)`
    (no `broadcaster_kind` — rewards are inflation, not gateway-driven)
  - Indexes match Phase 2 shape minus the `broadcaster_kind` column
- `036_create_tickets_daily.up.sql`
  - `chain_id`, `day_utc`,
    `broadcaster_kind TEXT NOT NULL CHECK(broadcaster_kind IN ('ai','transcoding'))`,
    `ticket_count BIGINT`, `distinct_orchestrators INT`,
    `distinct_gateways INT`, `source_max_event_id BIGINT`,
    `updated_at TIMESTAMPTZ`
  - PK: `(chain_id, day_utc, broadcaster_kind)`

**Workers (in `livepeer-rollups`)**

- `rollup_orch_rewards_daily` — same shape as payouts but reads
  `event_name = 'Reward'`, joins `TranscoderUpdate` for `reward_cut_raw`,
  no inversion (`orch_tokens = total_tokens × (reward_cut_raw / 1_000_000)`)
- `rollup_tickets_daily` — reads `WinningTicketRedeemed`, joins
  `broadcaster_classifications` on `from_address` (with `'transcoding'`
  default), groups by `(day_utc, broadcaster_kind)`

Both inherit the deterministic-writer acceptance criteria from Phase 0.

**API endpoints**

- `GET /rewards/leaderboard?from=…&to=…&sort=…&cursor=…&limit=…`
- `GET /rewards/summary/{daily|weekly|monthly}/{date}`
- `GET /tickets/timeseries/daily?start=YYYY-MM-DD&end=YYYY-MM-DD&job_type=…`
  - Validate `(end − start) ≤ 730` days (matches legacy)
  - Returns `{ ai: [{date, count}, …], transcoding: [{date, count}, …] }`

**Definition of done**

- All three new tables populated to head
- Reward formula spot-check against any orch's actual on-chain
  `WithdrawFees` over a known period
- Timeseries split for `0xd00354656922168815fcd1e51cbddb9e359e3c7f`
  matches legacy ratios for 2025-09
- Replay determinism CI extended to cover both new tables

**Risks** — same as Phase 2; no new ones.

**Dependencies** — Phase 2 worker pattern proven.

### Phase 4 — Direct-query endpoints (no new tables)

**Endpoints**

- `GET /reports/payouts.csv?orchestrator=0x…&start=YYYY-MM-DD&end=YYYY-MM-DD[&valuation_version=…][&chain_id=42161]`
  - Streams CSV from `raw_protocol_events`
    (filter `WinningTicketRedeemed`, `to_address = orchestrator`) joined to
    `event_valuations` and most recent `TranscoderUpdate` at-or-before each
    event
  - Columns (strict order, only renames vs legacy):
    ```
    timestamp, transaction_id, face_value, face_value_usd,
    orch_commission, orch_commission_usd, eth_price, fee_cut,
    transaction_fee, transaction_fee_usd, total_value_usd, total_value,
    block_number, chain_id, valuation_version, from_address,
    fee_share_percent, fee_cut_percent
    ```
  - Response headers: `X-Valuation-Version`, `X-Backfill-Complete`
- `GET /reports/rewards.csv?orchestrator=0x…&start=…&end=…[&valuation_version=…][&chain_id=42161]`
  - Columns:
    ```
    timestamp, transaction_id, lpt_price, eth_price, orch_tokens, total_tokens,
    reward_cut, transaction_fee, transaction_fee_usd, total_value_usd,
    block_number, chain_id, valuation_version, reward_cut_percent
    ```
- `GET /reports/gateway-payouts.csv?gateway=0x…&start=…&end=…[&valuation_version=…][&chain_id=42161]`
  - Same columns as `payouts.csv` but filtered by `from_address = gateway`
    instead of `to_address = orchestrator`
- `GET /orchestrators/{address}/tickets/latest?cursor=…&limit=…`
  - `raw_protocol_events.WinningTicketRedeemed` filtered by `to_address`,
    ordered `(block_number DESC, log_index DESC)`
- `GET /gateways/{address}/tickets?start=…&end=…&cursor=…&limit=…`
  - Same source, filtered by `from_address`
- `GET /governance/votes?proposal_id=…&voter=…&cursor=…&limit=…`
  - `raw_protocol_events.VoteCast`, filterable
  - Shippable now; the corrected Governor emitter address (commit `718e084`)
    has driven event ingest essentially to completion (2,975 events vs.
    legacy's ≈ 2,908). `meta.coverage` continues to reflect any residual
    indexing lag.

No migrations. No new workers.

**Definition of done**

- CSV column-by-column comparison between new and legacy API for
  `0xd00354656922168815fcd1e51cbddb9e359e3c7f` over one full week — must
  match (allowing for the documented renames)
- All endpoints documented in OpenAPI
- Streaming verified for date ranges large enough to exceed buffer (year+)

**Risks** — Low. CSV streaming for very long date ranges may need
response chunking; benchmark before promoting.

**Dependencies** — Phase 0 only. Could ship in parallel with Phase 1.

### Phase 5 — Operations, deprecation, documentation

**Admin endpoints (separate internal port)**

- `PUT /admin/overrides/{address}` — body
  `{ display_name?, avatar_url?, notes? }`
- `DELETE /admin/overrides/{address}`
- `PUT /admin/broadcaster-classification/{address}` — body
  `{ kind: 'ai'|'transcoding' }`
- `DELETE /admin/broadcaster-classification/{address}` — resets to default
- `GET /admin/divergences` — lists overrides where
  `ens_name != ens_name_at_override_time`

Bind address for admin listener pinned in config (default
`127.0.0.1:9091`). Public listener stays on its current port. Runbook
entry explicitly calls out that the admin port must not be exposed past
the production proxy.

**Operational additions**

- Prometheus metrics per new worker:
  - `staker_profile_rows_written_total{kind=…}`
  - `staker_profile_last_processed_event_id{kind=…}`
  - `enricher_ens_failures_total`, `enricher_rpc_calls_total{kind=…}`
  - `rollup_rows_written_total{worker=…}`
  - `rollup_last_processed_event_id{worker=…}`
  - `rollup_reorg_recompute_total{worker=…}`
- Alerts:
  - Any rollup or staker-profile checkpoint stalled > N minutes
  - ENS failure rate exceeds threshold for K consecutive minutes
  - `reorg_recompute_total` jumps disproportionate to `reorg_events`
- Runbook updates: enricher L1 dependency, override management, admin port
  boundary, rollup backfill procedure, replay-determinism verification
  for new deterministic tables

**Documentation**

- Migration guide doc: legacy endpoint → new endpoint mapping table with
  semantic notes (esp. the fee inversion: legacy `fee_cut` = new
  `fee_cut_percent / 100`, NOT `fee_share_percent / 100`)
- Add a "fee semantics" section to API docs that walks through the
  inversion with `0xd00354656922168815fcd1e51cbddb9e359e3c7f` as the
  worked example

**Optional compatibility shim**

If legacy clients are still in flight, ship a thin compatibility layer at
the legacy `/api/...` paths that 308-redirects to the new endpoints (or
proxies them with field renames). Decide based on inventory of external
consumers.

## Validation plan

End-to-end checks before declaring the sequence complete:

1. **Fee semantics invariant**: for every orch in `orchestrator_profile`,
   `fee_cut_percent + fee_share_percent` rounds to 100.0. Run as an
   acceptance test.
2. **Commission parity**: for
   `0xd00354656922168815fcd1e51cbddb9e359e3c7f`,
   `SUM(orch_payouts_daily.sum_commission_native)` over month M equals
   legacy `SUM(payout.orch_commission)` over month M (allowing legacy
   float drift).
3. **Reward parity**: same orch,
   `SUM(orch_rewards_daily.sum_orch_tokens)` equals legacy
   `SUM(reward.orch_tokens)`.
4. **Replay determinism (deterministic tables)**: drop
   `orchestrator_profile`, `broadcaster_profile`, all `orch_*_daily`,
   `tickets_daily`; reset their checkpoints; restart the workers — tables
   rebuild byte-for-byte identical. Wired into the existing
   `scripts/run-determinism-replay.sh`.
5. **Replay isolation (external tables)**: same drop/reset cycle leaves
   `*_ens` and override tables empty until the enricher / operators
   repopulate them. Replay verification explicitly does not compare
   these tables.
6. **Reorg-correctness simulation**: insert a synthetic `reorg_mutations`
   row that moves a ticket event between calendar days — observe both
   affected daily rollup cells recompute correctly.
7. **CSV column parity**: download legacy `/api/report/ticket/download`
   for one orch+week; download new `/reports/payouts.csv` for the same;
   diff. Only the renames + appended columns should differ.
8. **Cursor pagination correctness**: page through `/payouts/leaderboard`
   with `limit=10` until cursor exhausted. Compare to a single-shot
   `limit=1000` query — same rows, same order, no duplicates, no gaps.
9. **`meta.coverage` accuracy**: while Governor backfill is incomplete,
   `/governance/votes` returns `meta.coverage.backfill_complete = false`
   and a `last_processed_block` consistent with `indexer_checkpoints`.
10. **L1 isolation**: simulate L1 mainnet RPC outage. The L2 pipeline,
    deterministic writers, and rollup workers continue. Only
    `livepeer-enricher` degrades, surfaces an alert, and `*_ens` tables
    go stale.

## Concrete task list

- [x] **Phase 0**
  - [x] Migration `028_create_broadcaster_classifications` with seed
  - [x] Migration `029_create_name_avatar_overrides`
  - [x] Add `fee_cut_percent` to `TranscoderParamsRow` and update
    `scaled_percent` callsites
  - [x] OpenAPI regen + field documentation
  - [x] **Spec edits in same PR**: flip `v1-livepeer-indexer.md:49` items;
    add §14.3.x entry; bump `Document version` to `1.9`; add
    `Changes since v1.8 (YYYY-MM-DD)` changelog entry
  - [x] Acceptance: spot-check on
    `0xd00354656922168815fcd1e51cbddb9e359e3c7f`

- [x] **Phase 1**
  - [x] Migrations `030_create_orchestrator_profile` and
    `031_create_broadcaster_profile` (deterministic columns only)
  - [x] Migrations `032_create_orchestrator_ens` and
    `033_create_broadcaster_ens` (external)
  - [x] Extend `livepeer-staker` with `staker_orch_profile` and
    `staker_gateway_profile` checkpoints + writers
  - [x] Wire all new RPC calls through the existing cached `eth_call` path
  - [x] Verify `as_of_block` / `as_of_round` derive from triggering events
  - [x] New crate `livepeer-enricher` as a standalone binary (NOT daemon-supervised)
  - [x] ENS lazy resolve + L1 watcher + TTL refresh sweep
  - [x] L1 RPC circuit breaker + metrics
  - [x] API endpoints `/orchestrators` (list + single)
  - [x] API endpoints `/gateways` (list + single profile)
  - [x] Read-path JOIN logic with COALESCE override → ens
  - [x] Replay determinism CI extension for `orchestrator_profile`,
    `broadcaster_profile`
  - [x] OpenAPI
  - [x] Tests

- [x] **Phase 2**
  - [x] Migration `034_create_orch_payouts_daily`
  - [x] Verify / add `TranscoderUpdate` covering index
  - [x] New crate `livepeer-rollups` skeleton; ships standalone binaries (daemon embedding optional, off by default)
  - [x] `rollup_orch_payouts_daily` worker
  - [x] Reorg recomputation path via `reorg_mutations` polling
  - [x] API: `/payouts/leaderboard`, `/payouts/summary/{daily,weekly,monthly}/{date}`
  - [x] Backfill verification
  - [x] Replay determinism CI extension for `orch_payouts_daily`

- [x] **Phase 3**
  - [x] Migration `035_create_orch_rewards_daily`
  - [x] Migration `036_create_tickets_daily`
  - [x] `rollup_orch_rewards_daily` worker
  - [x] `rollup_tickets_daily` worker
  - [x] API: `/rewards/leaderboard`,
    `/rewards/summary/{daily,weekly,monthly}/{date}`,
    `/tickets/timeseries/daily`
  - [x] Replay determinism CI extension for both new tables

- [x] **Phase 4**
  - [x] `GET /reports/payouts.csv`
  - [x] `GET /reports/rewards.csv`
  - [x] `GET /reports/gateway-payouts.csv`
  - [x] `GET /orchestrators/{addr}/tickets/latest`
  - [x] `GET /gateways/{addr}/tickets`
  - [x] `GET /governance/votes`
  - [x] CSV column-parity comparison against legacy: payout / gateway-payout /
    reward exports now use legacy transaction URL + gas-fee semantics, with
    reward `eth_price` fixed to read the stored `pricing_chain.steps` shape

- [ ] **Phase 5**
  - [ ] Admin port + binding config
  - [ ] All admin endpoints
  - [ ] Prometheus metrics for new workers
  - [ ] Alert rules
  - [ ] Runbook updates (incl. replay-determinism procedure)
  - [ ] Migration guide doc (legacy → new)
  - [ ] Optional: legacy-path compatibility shim

## Suggested sequencing

| Sprint | Work |
|---|---|
| 1 | Phase 0 |
| 2 | Phase 1 ‖ Phase 4 (independent) |
| 3 | Phase 2 |
| 4 | Phase 3 |
| 5 | Phase 5 |

## Tracked-but-out-of-scope items

These don't block this plan but should be visible:

- **Governor backfill completion** (separate work, see commit `718e084`):
  essentially complete — 2,975 events ingested under the corrected emitter
  address vs. legacy DB's ≈ 2,908. `/governance/votes` is shippable now.
  Any residual indexing lag surfaces via `meta.coverage`.
- **TD-016 metrics + restart-test completion**: the gateway flow refactor
  in commit `7192dbf` is now actively running in the local runtime
  (4,000 / 1,000 / 598 rows in flows / claimants / balances respectively
  and climbing). The operability metrics from the Phase E task list and
  the restart-test validation remain pending per their own task list.
  Independent of this plan.
- **Exact historical total-stake-at-block-X**: not needed for any current
  endpoint. If a future endpoint needs it, the implementation requires
  either per-orch periodic snapshotting of `transcoderTotalStake` or full
  per-delegator `pendingStake` fan-out. Track separately if the
  requirement arises.

## Progress log

- 2026-05-05: Plan drafted.
- 2026-05-05 (revised): Incorporated review feedback. Five substantive
  changes:
  (1) Determinism contract section added; deterministic state and external
  state separated into different tables to make the boundary enforceable
  at the schema level.
  (2) Migration numbering moved to start at `028_*` (current head is
  `027_stake_delegate_lookup_index`); forward-only language matched to
  repo discipline.
  (3) Phase 1 split: deterministic on-chain writer extends the existing
  `livepeer-staker`; non-deterministic ENS writer lives in a new
  `livepeer-enricher` crate. Disjoint target tables prevent collision.
  (4) Phase 2 schema locked with `broadcaster_kind` in PK; "Open
  implementation note" removed.
  (5) Spec impact addressed in-band: three localized edits to
  `v1-livepeer-indexer.md` ship in the Phase 0 PR; no separate amendment
  doc, no external gating. Status remains `planned`.
  Two implementation requirements made explicit as acceptance criteria
  for the staker extension and rollup workers: all RPC calls go through
  `rpc_call_cache`; `as_of_block` / `as_of_round` derive from triggering
  events.
- 2026-05-05 (revised, third pass): Cleanup-only edits per team review.
  Two stale references corrected: the Phase 0 spec-edit task list now
  reads "Bump `Document version` to `1.9`" (was a leftover "v1.2 → v1.3"
  reference); the "Rollup data source" locked-decision row now reflects
  the current state of `gateway_flows` (actively backfilling under
  TD-016) while preserving the design rationale that the rollup reads
  `raw_protocol_events` directly to remove the cross-worker dependency.
- 2026-05-05 (revised, second pass): Incorporated team's remaining
  pre-implementation feedback:
  (a) Worker deployment shape made explicit — `livepeer-enricher` ships as
  a standalone binary only (never daemon-supervised); `livepeer-rollups`
  workers ship as standalone binaries with optional daemon embedding,
  matching `livepeer-staker`'s dual-pattern. Rationale documented per
  TD-016's lesson.
  (b) "Persistence requirements" subsection added under the determinism
  contract spelling out the replay/backup contract for external tables.
  (c) Profile-writer monotonicity rule added as acceptance criterion #3:
  `last_event_id BIGINT NOT NULL` columns on `orchestrator_profile` and
  `broadcaster_profile`, with `WHERE excluded.last_event_id > target.last_event_id`
  upsert guard. Rollup tables already use the equivalent
  `source_max_event_id`.
  (d) Spec version bump corrected from "v1.2 → v1.3" to "v1.8 → v1.9" to
  match actual current spec lineage.
  (e) Background pipeline counts refreshed: `gateway_flows` is no longer 0
  (4,000 and climbing under TD-016's runtime); Governor events are
  essentially complete (2,975 vs. legacy ≈ 2,908). Tracked-out-of-scope
  items and Phase 4 votes endpoint framing updated accordingly.
- 2026-05-05 (implementation): Phase 0 partial landed in code. Added
  migrations `028_create_broadcaster_classifications` and
  `029_create_name_avatar_overrides`, added `fee_cut_percent` to the
  transcoder API responses, and updated the spec/OpenAPI to v1.9 semantics.
- 2026-05-05 (implementation, follow-up): Closed the remaining Phase 0 seed
  gap by sourcing the 10 AI broadcaster addresses from the legacy
  `livepeer-backend-rs` repo: hardcoded list in `src/lib.rs` plus matching
  named entries in `config.toml`.
- 2026-05-05 (implementation, Phase 1 foundation): Added migrations
  `030_create_orchestrator_profile` and `031_create_broadcaster_profile`,
  plus a new `livepeer-staker profile-backfill` command that writes
  deterministic on-chain profile rows through cached `eth_call`s and
  monotonic `last_event_id` upserts. First local smoke pass succeeded:
  256 `orchestrator_profile` rows, 2 `broadcaster_profile` rows, checkpoints
  advanced to blocks 6,203,936 / 7,203,883.
- 2026-05-05 (implementation, Phase 1 deterministic follow-up): closed the
  two biggest deterministic-profile gaps. `service_uri` is now populated via
  cached on-chain reads through
  `Controller.getContract(keccak256("ServiceRegistry"))` +
  `ServiceRegistry.getServiceURI()` at the trigger block; live spot-check
  confirmed non-null URIs landing in `orchestrator_profile` (27 rows on the
  local dataset during verification). `TransferBond` is now handled by
  decoding `newDelegator` from the stored raw log and resolving its delegate
  with `BondingManager.getDelegator(newDelegator)` at the event block. The
  remaining Phase 1 gaps are ENS/external tables, API read endpoints, and the
  slow known-orchestrator bootstrap query, which still wants a tighter index
  or alternate seed source before continuous use.
- 2026-05-05 (implementation, Phase 1 read layer): Added migrations
  `032_create_orchestrator_ens` and `033_create_broadcaster_ens`, then
  landed the first profile endpoints in `livepeer-api`:
  `GET /orchestrators`, `GET /orchestrators/{address}`, `GET /gateways`,
  and `GET /gateways/{address}/profile`. The read path now joins
  deterministic profile tables with `name_avatar_overrides`,
  `orchestrator_ens` / `broadcaster_ens`, and
  `broadcaster_classifications`, defaulting gateway kind to `transcoding`
  when no classification exists. Local runtime smoke tests against Postgres
  returned real rows from both list endpoints. Remaining Phase 1 work is the
  standalone `livepeer-enricher`, end-to-end ENS population/precedence
  verification, replay-CI coverage for the deterministic profile tables,
  endpoint tests, and tightening the known-orchestrator bootstrap query
  before continuous use.
- 2026-05-05 (implementation, Phase 1 external writer): Added a new
  standalone `livepeer-enricher` crate and wired it into the workspace,
  `Dockerfile`, and `docker-compose`. The initial implementation ships the
  useful external half of the plan: one-shot and follow-mode sweeps over
  unresolved or stale `orchestrator_profile` / `broadcaster_profile`
  addresses, reverse ENS resolution on L1 mainnet, forward-resolution
  verification, avatar text lookup, and upserts into
  `orchestrator_ens` / `broadcaster_ens`. A small consecutive-failure
  circuit breaker now cools the worker down on repeated L1 RPC errors.
  Local proof against the current Postgres dataset succeeded:
  `backfill --batch-limit 50` wrote 50 orchestrator ENS rows and 2
  broadcaster ENS rows, with 24 names and 21 avatars resolved during the
  run. The full L1 ENS event watcher, dedicated metrics surface, replay-CI
  notes for the deterministic tables, and endpoint tests remain open.
- 2026-05-05 (implementation, Phase 1 tests + acceptance follow-up): Added
  route-level integration tests for the new profile endpoints. The tests hit
  a real Postgres schema through the shared Axum router and verify three
  load-bearing behaviors: orchestrator override precedence over ENS,
  orchestrator cursor pagination + `active_only`, and gateway
  classification/default semantics plus override precedence. `cargo test -p
  livepeer-api profiles::tests -- --nocapture` now passes with 2 green tests.
  Acceptance spot-check status is mixed but informative: the requested
  orchestrator `0xd00354656922168815fcd1e51cbddb9e359e3c7f` exists locally in
  `orchestrator_profile` with a resolved `service_uri`
  (`https://lp-orch.svr.run:8935` at block `6407785` / round `2472`), but the
  two seeded AI broadcaster addresses checked during this pass do not yet
  have `broadcaster_profile` rows on the local dataset, so the AI-gateway
  acceptance item remains open as a data-coverage issue rather than an API
  or schema bug.
- 2026-05-05 (implementation, Phase 1 replay-CI extension): Wired strict
  replay to rebuild `orchestrator_profile` and `broadcaster_profile`,
  updated replay reset/truncation to include both deterministic profile
  tables, and extended `scripts/compute-determinism-hashes.sh` to hash them
  in stable PK order. To keep the pre-existing compact fixtures viable, replay
  now skips contract backfills whose fixture cache contains no `eth_getLogs`
  rows for that address instead of forcing an immediate cache-only failure.
  Both committed determinism fixtures were refreshed with the newly required
  cached profile RPC inputs and new baseline hashes. Verified end-to-end:
  `bash scripts/run-determinism-replay.sh` now passes for both `case-a` and
  `case-b`, while still excluding `orchestrator_ens` / `broadcaster_ens` from
  the replay contract.
- 2026-05-05 (implementation, Phase 1 enricher metrics): Added a dedicated
  `livepeer-enricher` `/metrics` + `/health` HTTP surface plus basic
  Prometheus instrumentation for sweep outcomes, updated rows, resolved
  names/avatars, resolve failures, and breaker-open state/transitions. Local
  proof used `follow --batch-limit 5 --cadence-secs 60` bound to
  `127.0.0.1:19112`; `/health` returned `ok` and `/metrics` exposed live
  counters including `livepeer_enricher_sweeps_total{result="ok"} 1`,
  `livepeer_enricher_rows_updated_total{entity="orchestrator"} 2`, and
  `livepeer_enricher_breaker_open 0`.
- 2026-05-05 (implementation, Phase 1 close-out): Finished the remaining
  Phase 1 engineering work in two places. First, `livepeer-enricher follow`
  now runs a checkpointed L1 ENS watcher before each TTL sweep: it polls
  `NameChanged` plus both known `TextChanged` topic signatures, maps reverse
  resolver changes back to tracked addresses via reverse-node derivation,
  maps avatar text changes back to tracked rows via forward `namehash`, and
  immediately refreshes the affected `orchestrator_ens` /
  `broadcaster_ens` rows. The watcher uses a dedicated
  `enricher_ens_l1_logs` checkpoint and was smoke-tested live against local
  Postgres + `L1_RPC_URL` with the host-side `DATABASE_URL` override. Second,
  the slow steady-state known-orchestrator bootstrap path in
  `livepeer-staker profile-backfill` no longer rescans historical
  `raw_protocol_events` on every resumed pass: when an orchestrator profile
  checkpoint exists, it seeds the in-memory known set directly from
  `orchestrator_profile` rows before falling back to the old raw-event scan
  only for true cold-start bootstrap. At that moment the only remaining
  Phase 1 acceptance gap was data coverage for the seeded AI gateways on the
  current local dataset.
- 2026-05-05 (implementation, Phase 2 core slice): Landed the deterministic
  payout-rollup foundation. Migration `034_create_orch_payouts_daily`
  creates the materialized table plus a dedicated
  `TranscoderUpdate(chain_id, contract_name, event_name, to_address,
  block_number DESC, log_index DESC)` covering index for point-in-time fee
  share lookups. A new standalone `livepeer-rollups` crate now ships
  `orch-payouts-daily`, with one-shot and follow-mode execution, its own
  `rollup_orch_payouts_daily` checkpoint, and deterministic accumulation of
  `(day_utc, orchestrator_address, valuation_version, broadcaster_kind)`
  cells from finalized canonical `WinningTicketRedeemed` rows joined to
  `event_valuations` and `broadcaster_classifications`. On the current local
  dataset the first pass wrote 2 aggregate rows through source event id `16`,
  and a second pass wrote `0` rows while leaving both the checkpoint and row
  count unchanged, proving checkpoint idempotency. Direct SQL spot-checks on
  those two local ticket rows matched both `ticket_count` and
  `sum_face_value_native` exactly. Remaining Phase 2 work is the reorg
  recompute path, payout leaderboard/summary API endpoints, and replay-CI
  coverage for `orch_payouts_daily`.
- 2026-05-05 (implementation, Phase 2 payout API): Added the first public
  read layer on top of `orch_payouts_daily` in `livepeer-api`:
  `GET /payouts/leaderboard` with cursor pagination over
  `commission_usd | ticket_count | face_value_usd`, plus
  `GET /payouts/summary/daily/{date}`,
  `GET /payouts/summary/weekly/{date}`, and
  `GET /payouts/summary/monthly/{date}` with `job_type` and
  `valuation_version` filtering. The leaderboard joins ENS and override
  overlays for orchestrator identity fields just like the Phase 1 profile
  endpoints. Route-level integration coverage now exercises leaderboard
  pagination plus daily/weekly/monthly summaries from fixture rollup rows,
  and local runtime smoke tests against `127.0.0.1:8080` returned real
  payloads for October 2022 from the current `orch_payouts_daily` rows.
  Remaining Phase 2 work is now narrowed to reorg recomputation and
  replay-determinism coverage for the payout rollup table.
- 2026-05-05 (implementation, Phase 2 reorg + replay close-out): Finished the
  remaining deterministic rollup hardening. `livepeer-rollups
  orch-payouts-daily` now polls `reorg_mutations` via a dedicated
  `rollup_orch_payouts_daily_reorg` checkpoint, derives the affected
  `(day_utc, orchestrator_address, valuation_version, broadcaster_kind)` cells
  from both current canonical ticket state and any previously materialized
  aggregate rows touched by the mutated event ids, and rebuilds those cells in
  full from canonical source rows. Local proof used a deliberately corrupted
  `orch_payouts_daily` row plus a synthetic `reorg_mutations` entry for a real
  `WinningTicketRedeemed` event id; one worker pass rewrote the row back to the
  exact canonical `ticket_count`, `sum_commission_native`, and
  `sum_commission_usd` values. Strict replay now also rebuilds
  `orch_payouts_daily`: `livepeer-orchestrator replay` invokes the rollup
  worker, replay reset truncates the table, `scripts/compute-determinism-hashes.sh`
  hashes it in stable PK order, and both committed fixture baselines now carry
  `orch_payouts_daily` counts + md5s. End-to-end verification:
  `bash scripts/run-determinism-replay.sh` passes for both `case-a` and
  `case-b`. The remaining reorg-production nuance is upstream of TD-017:
  `livepeer-reorg-watcher` still does not emit full `reorg_mutations` coverage
  in live mode, which remains tracked separately under TD-005.
- 2026-05-05 (implementation, Phase 3 close-out): Landed the reward and
  ticket rollup layer end to end. Migrations `035_create_orch_rewards_daily`
  and `036_create_tickets_daily` add the two deterministic Phase 3 tables.
  `livepeer-rollups` now ships `orch-rewards-daily` and `tickets-daily`, each
  with its own checkpoint plus `reorg_mutations` recomputation path. The
  reward worker groups finalized canonical `Reward` rows by
  `(day_utc, orchestrator_address, valuation_version)`, applies point-in-time
  `rewardCut` from the latest canonical `TranscoderUpdate`, and materializes
  total / orch / delegator token splits in both native and USD columns. The
  ticket worker groups finalized canonical `WinningTicketRedeemed` rows by
  `(day_utc, broadcaster_kind)` and tracks both ticket counts and distinct
  orchestrator / gateway counts. The API now exposes
  `GET /rewards/leaderboard`,
  `GET /rewards/summary/{daily,weekly,monthly}/{date}`, and
  `GET /tickets/timeseries/daily`, with route-level tests covering both the
  reward leaderboard/summary reads and the zero-filled daily ticket
  timeseries. Local runtime verification on the current Postgres dataset shows
  migrations `35` and `36` recorded in `_sqlx_migrations`, `tickets_daily`
  materializing `1` real row for `2022-10-31 / transcoding` with
  `ticket_count = 2`, and a second `tickets-daily` run writing `0` rows while
  leaving the checkpoint unchanged. The current local dataset still contains
  `0` canonical `Reward` rows, so `orch_rewards_daily` remains empty there,
  but strict replay now fully exercises that path: both committed fixture
  cases rebuild `orch_rewards_daily` and `tickets_daily`, and
  `bash scripts/run-determinism-replay.sh` passes with the new hashes
  committed.
- 2026-05-05 (implementation, Phase 4 direct-query endpoints): Added the
  non-materialized export and history surface in `livepeer-api`. New report
  routes are `GET /reports/payouts.csv`,
  `GET /reports/rewards.csv`, and
  `GET /reports/gateway-payouts.csv`, all backed directly by canonical
  `raw_protocol_events` plus `event_valuations` and point-in-time
  `TranscoderUpdate` lookups, with response headers
  `X-Valuation-Version` and `X-Backfill-Complete`. New ticket-history reads
  are `GET /orchestrators/{address}/tickets/latest` and
  `GET /gateways/{address}/tickets`, both paginated over canonical
  `WinningTicketRedeemed` rows and enriched with point-in-time fee-share /
  fee-cut percentages. Governance now also exposes
  `GET /governance/votes`, filtering canonical `VoteCast` /
  `VoteCastWithParams` rows by `proposal_id` and/or `voter` and surfacing a
  simple governor-domain coverage block from `indexer_checkpoints`. Local
  smoke verification on the current replay fixture dataset hit the new payout
  CSV plus both ticket-history endpoints successfully, and the votes endpoint
  returned an empty-but-valid payload with `meta.domain = "governor"` because
  the current fixture has no Governor rows. Remaining Phase 4 work is only
  the explicit legacy column-parity comparison for the CSV downloads.
- 2026-05-05 (implementation, Phase 4 parity follow-up): Corrected the CSV
  export formulas in `livepeer-api` to match legacy `livepeer-backend-rs`
  semantics. `transaction_id` is now an Arbiscan URL, and
  `transaction_fee` / `transaction_fee_usd` are derived from cached
  `eth_getTransactionReceipt` gas usage instead of being mis-modeled as
  delegator share. `total_value` and `total_value_usd` now subtract actual gas
  cost for payout and gateway-payout exports, and reward `total_value_usd`
  now reflects orchestrator-side LPT value minus gas. Local smoke on
  `/reports/payouts.csv` against the current DB confirmed the corrected URL,
  gas-fee, and total-value columns. Final legacy parity proof remains
  partially data-limited because the current local dataset still has no
  canonical `Reward` rows to compare.
- 2026-05-05 (implementation, final acceptance close-out): Reloaded a bounded
  live window (`459622094..459722134`) into the local Postgres replica via
  `livepeer-indexer` backfills for `TicketBroker` and `BondingManager`,
  followed by a one-shot finality pass, `livepeer-valuator backfill-all`, and
  `livepeer-staker profile-backfill`. That window produced `2` finalized
  canonical `Reward` rows with valuations and materialized two seeded AI
  gateways in `broadcaster_profile`:
  `0xca3331d67e87816adb30d9562a6e8c0623fb7fef` and
  `0x5ae4e42db3671370a0c25aff451e7482aaec3d0b`. Live API proofs then
  confirmed `GET /gateways`, `GET /gateways/{address}/profile`, and
  `GET /reports/rewards.csv` against those rows. During that proof pass a real
  reward-export bug surfaced: `eth_price_from_chain()` only handled a
  top-level array even though stored `pricing_chain` is an object with
  `steps`. Fixing that parser restored nonzero reward `eth_price`,
  `transaction_fee_usd`, and `total_value_usd`, and a regression test was
  added in `routes::reports::tests`. With that correction, the legacy CSV
  parity comparison and AI-gateway acceptance proof are both closed.
