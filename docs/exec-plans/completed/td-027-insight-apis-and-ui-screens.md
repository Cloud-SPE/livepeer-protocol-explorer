# TD-027: Insight APIs + UI Screens

**Status:** Resolved 2026-05-09
**Author:** 2026-05-09
**Severity:** medium
**Source:** Post-TD-025/TD-026 follow-up — exposing the new historical datasets through both API and frontend

## Resolution (2026-05-09)

All eight phases shipped and verified live in a single deploy.

- **Phase 0 — backend extension:** `latest_round_started_block` and `latest_round_started_at` added to `NetworkStatsResponse` in `crates/livepeer-api/src/routes/network.rs`. Pulls from the same `orch_stake_by_round` rows the existing query already touches; ~10 LOC, no new RPC, no schema changes.
- **Phase A — types + client + service wrappers:** TS interfaces in `frontend-ui/src/types/api.ts` mirror the Rust `*Response` structs verbatim (same field names, same order, all numeric strings typed as `string`). Six new `localApi.*` methods in `lib/sources/local-api.ts`. New service files: `services/network.service.ts`, `services/delegators.service.ts`, `services/stake-history.service.ts`. Existing `services/orchestrators.service.ts` extended with `fetchCutsHistory` and `fetchNetEconomics`.
- **Phase B — Network totals card refresh:** `views/dashboard.ts` now consumes `/network/stats` for the hero card with `Round N · Block M · <relative time>` header, 24 h payouts/rewards/gas strip, and matview-age indicator. Other dashboard cards keep their existing fetches per the narrow-scope decision.
- **Phase C — orchestrator detail extensions:** stake-history chart, cuts-history timeline, net-economics card added beneath the existing orch metadata.
- **Phase D — delegator detail page:** `views/delegator-detail.ts` + route `/delegators/:address` shipped; renders portfolio sorted by `bonded_principal DESC` with chip-routed orch links.
- **Phase E — round detail page:** `views/round-detail.ts` + route `/rounds/:round_id` shipped; "started block / time / N active orchs / total LPT" header + top-orchs table + day-rollup totals + round navigator.
- **Phase F — chip kind prop + placeholder index views + nav:** `<address-chip kind="orchestrator|gateway|delegator|unknown">` (default `unknown` → `/delegators/{addr}`); call sites updated to pass `kind` where the entity type is known. Placeholder `/delegators` and `/rounds` index views added with empty-state copy directing to deep-link by address/id. Side-nav gained two flat entries.
- **Phase G — tests + docs:** smoke unit tests for the new services; visual regression baselines (TD-029 harness) cover the new routes; OpenAPI documentation generated automatically from the `ToSchema` derives.

**Live verification:** prod deploy 2026-05-09 serves all six endpoints. `curl /network/stats` returns `latest_round=4193, active_orchestrators=101, total_lpt_staked=28,002,078`, with `latest_round_started_block` and matview-refresh timestamps populated. Frontend separately deployed and consumes the new endpoints.

Real `/delegators` and `/rounds` *index* endpoints + UI (vs. the placeholder views shipped here) remain a future TD; deferred per the narrow-scope decision.

## What landed

The API layer is shipped and live. Six new endpoints, all backed by
already-collected Postgres state (no new RPC, no schema changes):

| Endpoint | Source tables | Powers |
|---|---|---|
| `GET /orchestrators/{addr}/stake-history?from_round&to_round` | `orch_stake_by_round` (TD-026) | per-round stake chart |
| `GET /orchestrators/{addr}/cuts-history` | `raw_protocol_events` (TranscoderUpdate) | cut-change timeline |
| `GET /orchestrators/{addr}/net-economics?period_days=30` | `orch_payouts_daily` + `orch_rewards_daily` + `tx_receipts` | net revenue card |
| `GET /delegators/{addr}` | `stake_balances_by_block` + `delegator_registry` | per-delegator portfolio |
| `GET /network/stats` | matviews + 24h rollup window + tx_receipts gas | dashboard tile pack |
| `GET /rounds/{round_id}` | `orch_stake_by_round` + daily rollups | per-round detail |

All six are wired into `lib.rs::build_router`, registered in `openapi.rs`,
documented under three new tags ("Orchestrator history", "Delegators",
"Network"), and verified live against production data.

## UI scope (this plan)

Seven screen-level surfaces in `frontend-ui/`:

1. **Network totals card** — refresh the hero card in `views/dashboard.ts`
   to use `/network/stats`. Other dashboard cards keep their existing
   per-service fetches (see Phase B for scope rationale).
2. **Orchestrator detail page** — extend `views/orchestrator-detail.ts` to
   add three new sections: stake-history chart, cuts-history timeline,
   net-economics card.
3. **Delegator detail page** — new `views/delegator-detail.ts` + route
   (`/delegators/:address`). Currently no per-delegator entry point exists.
4. **Delegators index** — new `views/delegators-list.ts` + route
   (`/delegators`). Minimal landing page so the new "Delegators" nav
   entry has a destination.
5. **Round detail page** — new `views/round-detail.ts` + route
   (`/rounds/:round_id`). Linked from the dashboard "latest round" tile.
6. **Rounds index** — new `views/rounds-list.ts` + route (`/rounds`).
   Minimal landing page listing recent rounds.
7. **Address-chip + side-nav polish** — `address-chip` gains a `kind`
   prop so callers declare what the address is; default `unknown`
   routes to `/delegators/{addr}`. Nav gains flat "Delegators" and
   "Rounds" links.

A full entity-search surface (paste-an-address-or-round-id) is **out
of scope**; tracked as a follow-up. The "Delegators" and "Rounds" nav
entries land on intentional placeholder views in v1 — real index
endpoints + UI are also follow-up work (see Phase F).

## Architecture notes

Patterns to reuse:
- **API client layer**: `lib/sources/local-api.ts` is a hand-rolled
  method-per-endpoint façade over `createApi`. Every new endpoint must
  add (a) a typed response interface in `types/api.ts` and (b) a method
  on the `localApi` object. Service wrappers consume `localApi.*` —
  they do not call `createApi` directly.
- **Services**: each new endpoint gets a service wrapper. Three new files
  (`stake-history.service.ts`, `delegators.service.ts`,
  `network.service.ts`). Existing services like `orchestrators.service.ts`
  get a single new method for net-economics + cuts-history.
- **Components**: leverage existing `chart-card.ts`, `time-chart.ts`,
  `bar-chart.ts`, `data-table.ts`. No new generic components needed.
- **Routing**: routes are registered in `components/app-shell.ts`, not
  `main.ts` (`main.ts` only boots the shell). Two edits per new route:
  add a `{ pattern: '...' }` entry in `_registerRoutes()` and a `case`
  in `_renderView()`. Light views are statically imported at the top
  of `app-shell.ts`; only register a lazy import (e.g. via
  `lazyTicketsTimeseries`) if the view pulls in ECharts or another
  heavy dep.

## Phases

### Phase 0 — Backend: extend `/network/stats` with latest-round block (~30 min)

The dashboard header (Phase B) needs `Round N · Block M` and the
matching round timestamp. The data is already at hand inside the
existing `network.rs::stats` SQL — `orch_stake_by_round.block_number`
and `block_timestamp` for `MAX(round)`. Add two fields to
`NetworkStatsResponse`:

```rust
pub latest_round_started_block: Option<String>,
pub latest_round_started_at:    Option<DateTime<Utc>>,
```

Extend the existing `WITH t AS (...)` query in
`crates/livepeer-api/src/routes/network.rs:57` to pick up
`block_number` and `block_timestamp` for the row(s) at `MAX(round)`,
and surface them on the response. Both `Option<...>` so the field
absence (no rounds in the table) doesn't break clients.

OpenAPI auto-regenerates from the `ToSchema` derive — no separate doc
edit. No new SQL tables, no new RPC.

**Acceptance:** `curl /network/stats` returns the two new fields with
non-null values against the live DB; `cargo test -p livepeer-api`
clean; OpenAPI snapshot diff shows the additive change only.

### Phase A — Types, client methods, service wrappers (~2 hours)

The frontend's API access is a three-layer stack; all three layers must be
extended before Phase B can compile.

**A1 — Response types in `frontend-ui/src/types/api.ts`**

Add interfaces that **mirror the shipped backend response structs
verbatim** — same names, same field names, same field order. The
canonical sources are the Rust `*Response` / `*Row` structs in the
API crate; treat them as the contract. Snake_case wire field names
are kept on the TS side (consistent with existing types like
`OrchestratorProfileRow`). All numeric stake / USD / ETH amounts
arrive as decimal strings — type them as `string`, never `number`.

| TS interface | Backend source | Notes |
|---|---|---|
| `StakeHistoryResponse` | `crates/livepeer-api/src/routes/profiles.rs` (`StakeHistoryResponse`) | `address`, `data: StakeHistoryRow[]`, `meta: ProfileListMeta`. `StakeHistoryRow` mirrors per-round snapshot. |
| `CutsHistoryResponse` | `crates/livepeer-api/src/routes/profiles.rs:622` (`CutsHistoryResponse`) | `address`, `data: CutsHistoryRow[]`, `meta`. Do **not** alias `TranscoderParamsRow`; the cuts-history row is its own type. |
| `NetEconomicsResponse` | `crates/livepeer-api/src/routes/profiles.rs:643` | Fields exactly: `address`, `period_days`, `period_start`, `period_end`, `gross_payouts_usd`, `gross_rewards_usd`, `gas_cost_native_eth`, `gross_total_usd`. |
| `DelegationRow` + `DelegatorResponse` | `crates/livepeer-api/src/routes/delegators.rs:17,34` | `DelegatorResponse` fields: `delegator_address`, `is_active`, `first_bond_block`, `last_seen_block`, `delegations`, `chain_id`. `DelegationRow` fields: `delegate_address`, `bonded_principal`, `pending_stake?`, `pending_fees?`, `pending_round?`, `as_of_block`, `as_of_timestamp`. |
| `NetworkStatsResponse` | `crates/livepeer-api/src/routes/network.rs:20` | Fields exactly: `chain_id`, `latest_round?`, `active_orchestrators`, `total_lpt_staked`, `gateways_known`, `payouts_usd_24h`, `rewards_usd_24h`, `gas_burned_eth_24h`, `orchestrator_profile_refreshed_at?`, `broadcaster_profile_refreshed_at?`. **Plus the two fields added by Phase 0 below: `latest_round_started_block?`, `latest_round_started_at?`.** |
| `RoundOrchSummary` + `RoundSummaryResponse` | `crates/livepeer-api/src/routes/network.rs:106,117` | `RoundSummaryResponse` fields: `round`, `round_started_block`, `round_started_at`, `active_orchestrators`, `total_lpt_staked`, `top_orchs: RoundOrchSummary[]`, `payouts_usd_on_day`, `rewards_usd_on_day`, `new_round_events`. `RoundOrchSummary` fields: `address`, `total_stake`, `fee_cut_percent`, `reward_cut_percent`, `fee_share_percent`, `is_active`. |

If any field on the backend struct gains/loses a property between now
and implementation, update both sides in the same PR. Do not invent or
omit fields on the TS side.

Verification step before declaring A1 done: `curl` each endpoint and
diff the JSON keys against the TS interface (a one-liner like
`curl -s ... | jq 'keys' > actual && diff <expected> actual`).

**A2 — Client methods on `localApi` in `lib/sources/local-api.ts`**

Add six methods, mirroring the existing per-endpoint helpers:

```ts
getStakeHistory(addr: string, params: { fromRound?: number; toRound?: number } = {})
getCutsHistory(addr: string)
getNetEconomics(addr: string, params: { periodDays?: number } = {})
getDelegator(addr: string)
getNetworkStats()
getRound(roundId: number | string)
```

**A3 — Service wrappers**

- `services/stake-history.service.ts` — single `fetchStakeHistory(orch, from, to)`
- `services/delegators.service.ts` — `fetchDelegator(addr)`
- `services/network.service.ts` — `fetchNetworkStats()`, `fetchRound(roundId)`
- Extend `services/orchestrators.service.ts` with `fetchCutsHistory(addr)` and
  `fetchNetEconomics(addr, periodDays)`

**Acceptance:** `npm run typecheck` clean; `npm run build` clean; each
service's `fetch*` method returns a typed value (no `any`).

### Phase B — Network totals card replacement (2 hours)

**Scope choice (narrow):** `/network/stats` replaces the **Network totals**
hero card and adds the 24 h rollup strip. The existing governance,
top-5 orchestrator, AI capability, and payout-summary cards keep their
own service fetches — they render data that `/network/stats` does not
expose, and pulling them apart is out of scope here.

**Depends on Phase 0** — the round-started-block field on
`NetworkStatsResponse` only exists after that ships.

In `views/dashboard.ts`:
- Add a new `ObservableController` for `networkService.stats$`.
- Replace the body of the **Network totals** card (currently fed by
  `orchs`/`gws`/`summary`) with values from `networkStats.value`.
  Specifically map:
  - `active_orchestrators` → "Active orchs" tile
  - `total_lpt_staked` → "Total LPT" tile
  - `gateways_known` → "Gateways" tile
- Add a **Past 24h** sub-section below the totals using
  `payouts_usd_24h`, `rewards_usd_24h`, `gas_burned_eth_24h`.
- Add a **Round / block** header line above the totals — `Round
  {latest_round} · Block {latest_round_started_block}` (both fields
  from the Phase 0 extension; render `—` when either is null). The
  round number links to `/rounds/{latest_round}`.
- Add a "Last refresh: X sec ago" line driven by
  `orchestrator_profile_refreshed_at` (the more conservative of the
  two matview timestamps), with a tooltip explaining the 30 s
  cadence (TD-025).
- Leave the other cards (Top 5, Recent governance, Activity charts,
  AI status) untouched. Their existing fetches stay.
- `_lastUpdated()` and `_anyLoading()` should now consider
  `networkStats` alongside the existing observables.

Layout sketch (only the top card changes; cards below remain as today):

```
┌─────────────────────────────────────────────────────────────────┐
│  LIVEPEER NETWORK · Round 4192 · Block 460,704,938              │
├─────────────────────────────────────────────────────────────────┤
│  [Active orchs]   [Total LPT]   [Gateways]   [Last refresh]     │
│      101         27.97M LPT       50          4 sec ago         │
│                                                                 │
│  [Past 24h]                                                     │
│  Payouts: $1,120     Rewards: $9,253     Gas burned: 0.005 ETH  │
└─────────────────────────────────────────────────────────────────┘
[Top 5 orchestrators]  [Recent governance]  [Activity charts]  ...
        (unchanged — keep existing per-service fetches)
```

**Acceptance:** the **Network totals** card is fed by a single
`/network/stats` call; the round number in the header navigates to
`/rounds/{N}`; matview-age indicator visible with tooltip; remaining
dashboard cards still render with their existing data sources; no
network-totals fetch fan-out from `dashboard.ts` for those four
fields any more.

**Out of scope (broader option, not chosen):** moving governance, AI,
and payouts off the homepage into dedicated views to make the dashboard
truly single-fetch. If product wants that later, file a follow-up.

### Phase C — Orchestrator detail extensions (3 hours)

Add three new sections to `views/orchestrator-detail.ts`:

```
[Existing: header, total stake, cuts, service URI...]

┌─ Stake history (last 100 rounds) ──────────────────────────────┐
│  [time-chart of total_stake by round, hover shows exact value] │
│  [Round-window selector: 30 / 100 / 365 / All]                 │
└────────────────────────────────────────────────────────────────┘

┌─ Cuts history ─────────────────────────────────────────────────┐
│  [history-list: each TranscoderUpdate as a row]                │
│  YYYY-MM-DD · fee_cut→X% reward_cut→Y% fee_share→Z%            │
└────────────────────────────────────────────────────────────────┘

┌─ Net economics (last 30 days) ─────────────────────────────────┐
│  [chart-card: payouts_usd $776  rewards_usd $18,889]           │
│  [Total: $19,665 · Gas: 0.0014 ETH]                            │
│  [Period selector: 7 / 30 / 90 / 365 days]                     │
└────────────────────────────────────────────────────────────────┘
```

Loading states use existing `empty-state` component. Charts use the
existing `time-chart` (line) and `chart-card` (KPI) components.

**Acceptance:** the three sections render below the existing orch
metadata; all three handle period changes via a query-param refresh;
empty states show "no data yet" rather than crash on missing rounds.

### Phase D — Delegator detail page (2 hours)

New route `/delegators/:address` → new view `views/delegator-detail.ts`.

```
┌─ Delegator 0x58b9...0716 ──────────────────────────────────────┐
│  [Active] · First bonded block 21,088,318                      │
├────────────────────────────────────────────────────────────────┤
│  Delegations (sorted by bonded principal DESC)                 │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │ Orch                  Bonded     Pending Stake   Fees    │  │
│  │ 0x5d11...b9fb         2.000 LPT  -                 -     │  │
│  │ 0x4bd8...d69b         1.000 LPT  1.000 LPT       0 ETH   │  │
│  └──────────────────────────────────────────────────────────┘  │
│  [Each orch row links to /orchestrators/{addr}]                │
└────────────────────────────────────────────────────────────────┘
```

Uses existing `data-table` + `address-chip` + `money-cell`.

**Acceptance:** `/delegators/0x58b9...0716` renders the portfolio; each
orch row navigates to the existing orchestrator detail page; 404 state
clean for unknown addresses.

### Phase E — Round detail page (2 hours)

New route `/rounds/:round_id` → new view `views/round-detail.ts`.

```
┌─ Round 4192 ───────────────────────────────────────────────────┐
│  Started block 460,704,938 · 2026-05-08 14:52 UTC              │
│  101 active orchestrators · 27.97M LPT total                   │
├────────────────────────────────────────────────────────────────┤
│  Top orchestrators by stake                                    │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │ Orch         Stake          Cut(F/R)        Active       │  │
│  │ 0x5254...    3,903,122 LPT  49% / 4%        ✓            │  │
│  │ ...                                                      │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                │
│  Day rollup totals                                             │
│  Payouts: $X · Rewards: $Y                                     │
└────────────────────────────────────────────────────────────────┘
```

Bonus: a small left/right round navigator (`/rounds/{round-1}` /
`/rounds/{round+1}`) for browsing.

**Acceptance:** loads under 500 ms; orch rows link to detail pages;
round navigator works.

### Phase F — Chip kind prop + placeholder index views + nav (~1.5 hours)

`address-chip` hard-codes `#/orchestrators/${address}` at
`components/ui/address-chip.ts:42`; `side-nav` is a flat array of
`NavItem`s. The work below is the minimum to make the new pages
reachable without introducing global resolvers or under-designed
index views.

**F1 — `kind` prop on `address-chip` (~30 min)**

Add an explicit, caller-supplied `kind` prop to the chip:

```ts
@property() kind: 'orchestrator' | 'gateway' | 'delegator' | 'unknown' = 'unknown';
```

In `render()`, replace the hardcoded `#/orchestrators/${this.address}`
href with a deterministic switch:
- `'orchestrator'` → `#/orchestrators/{addr}`
- `'gateway'`      → `#/gateways/{addr}`
- `'delegator'`    → `#/delegators/{addr}`
- `'unknown'`      → `#/delegators/{addr}` (default)

Update existing call sites that already know the entity type to pass
`kind` explicitly. This is mechanical: orchestrators-list, orch
detail header, gateway-list, gateway-detail header, delegator-detail
delegation rows, etc. Call sites that legitimately don't know
(governance vote lists, performance views) keep the default
`'unknown'` and route to `/delegators/{addr}` — the delegator page
404s gracefully for non-delegator addresses.

No global resolver, no service-cache dependency, no warm-up
contract. Behavior is determined entirely by the caller's prop.

**F2 — Placeholder index views for `/delegators` and `/rounds` (~30 min)**

Both routes are intentional placeholders for v1; real index pages
need real index endpoints, which are out of scope here.

- `views/delegators-list.ts` — single `empty-state` card:
  > **Open a delegator by address.** Paste a delegator address into
  > the URL bar, e.g. `#/delegators/0x58b9...0716`, or click any
  > delegator address chip elsewhere in the app. A searchable index
  > is tracked as a follow-up.
- `views/rounds-list.ts` — single `empty-state` card:
  > **Open a round by id.** Paste a round number into the URL bar,
  > e.g. `#/rounds/4192`, or click the current round on the
  > dashboard. A real index is tracked as a follow-up.

These views exist so the new nav entries don't 404. Each is one
component, no data fetches.

**F3 — Side-nav additions (~30 min)**

In `components/side-nav.ts:11`, append two flat `NavItem`s:

```ts
{ label: 'Delegators', href: '#/delegators', match: /^\/delegators/ },
{ label: 'Rounds', href: '#/rounds', match: /^\/rounds/ },
```

No grouping, no search input, no collapsible sections — those need
new component scaffolding and are deferred. Match the existing flat
style.

**Acceptance:** every existing call site of `<address-chip>` that
knows the entity type passes a `kind` prop; chips for unknown
addresses route to `/delegators/{addr}`; the two new nav entries
each load a non-crashing placeholder view; deep links to
`/delegators/{addr}` and `/rounds/{n}` work from anywhere.

**Out of scope:** entity-search input, side-nav grouping, fuzzy match
across ENS/display name, real `/delegators` and `/rounds` index
endpoints + UI. Tracked as a follow-up TD.

### Phase G — Tests + docs (1 hour)

- Smoke unit tests for the three new services.
- Update `frontend-ui/README.md` with the new routes.
- Add screenshots to `docs/design-docs/ui-screenshots/` (optional).

**Acceptance:** `npm test` clean; README mentions the new routes.

## Risks

| Risk | Mitigation |
|---|---|
| `stake-history` returns up to 100 rounds × 1 row per request — with chart libraries that's negligible, but `time-chart` may need a tweak if the existing component assumes ≤30 points. | Chart-card has been used elsewhere with hundreds of points (TD-017's per-day rollups). Should be fine. |
| `delegator-detail` may show very long delegation lists for delegators bonded to many orchs (rare in practice — most delegators bond to 1). | Sort by `bonded_principal DESC` and let the user scroll; data-table already handles arbitrary-length lists. |
| `/rounds/{round_id}` for very early rounds (e.g. round 0) might return empty `top_orchs` because no orch was ever active. | Handler already returns 404 for empty `orch_stake_by_round` lookup; UI handles 404 gracefully. |
| `network/stats` matview-refresh timestamps may confuse users | Display as "X seconds ago" relative time, with a tooltip explaining the 30 s cadence from TD-025. |

## Estimated effort

- Phase 0: 0.5 h (backend — extend `/network/stats` with two fields)
- Phase A: 2 h (types mirror backend exactly + `localApi` methods + services)
- Phase B: 2 h (narrow scope — Network totals card only; depends on Phase 0)
- Phase C: 3 h
- Phase D: 2 h
- Phase E: 2 h
- Phase F: 1.5 h (chip `kind` prop + placeholder index views + nav)
- Phase G: 1 h
- **Total: ~14 hours** (~2 days)

## Dependencies

- API layer (this plan, already shipped)
- TD-025 (broadcaster matview refresh) — Resolved
- TD-026 (orch_stake_by_round historical data) — Resolved
- No upstream blockers

## Future-proofing

The API surface is intentionally additive — every new endpoint is
backward-compatible with all existing consumers. The UI work is
likewise modular: each phase is shippable independently. If frontend
work is paused, the APIs remain useful for any third-party consumer
of the OpenAPI surface (already published at `/openapi.json`).

Three potential follow-ups, not in scope here:
- **Stake leaderboard time-series** — a sibling to `payouts/leaderboard`
  that takes `?at_round=N` to show "who was in the top 10 at round N."
- **Orchestrator scorecard** — composite metric combining cut stability,
  stake trend, gas efficiency, delegator retention. Needs design input.
- **Search endpoint** — fuzzy match across address, ENS, display_name,
  service_uri using `pg_trgm` or external index.
