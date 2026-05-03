---
title: valuation coverage gap
status: in_progress
opened: 2026-05-03
owner: codex+mazup
links:
  - spec: ../../product-specs/v1-livepeer-indexer.md
  - tracker: ../tech-debt-tracker.md
---

## Goal

Resolve the mismatch between the spec's "value every monetary event" contract and the current end-state accounting. Initial investigation suggested a large residual set of unvalued `is_valuable = TRUE` events; deeper analysis showed most of that set is actually valued under the degraded LPT version. The real remaining gap is the handling/acceptance semantics for terminal valuation failures.

## Problem statement

On a full fresh rerun against committed code through `1cf0cb8`, the pipeline reaches this state:

- `raw_protocol_events = 2,650,777`
- `event_valuations = 2,369,873`
- `valuation_attempts = 2,373,774`
- `token_prices_by_block = 1,660,949`
- valuator candidate selectors return `0` for:
  - seed pass
  - ETH on-chain pass
  - LPT on-chain pass
  - multi-asset pass
- the broad residual query still returns `226,256` finalized, canonical, `is_valuable = TRUE` rows with no valuation row at `valuation_version = 'v1_lpt_weth_twap_30min_x_chainlink_eth'`

That `226,256` figure is misleading by itself because it ignores degraded-version valuations.

## Corrected diagnosis

`event_valuations` by version on the fresh rerun:

- `v1_lpt_weth_twap_30min_x_chainlink_eth`: `2,143,953`
- `v1_degraded_spot_pre_cardinality`: `225,920`

When the residual query is widened to count either valuation version, the apparent gap collapses from `226,256` to `3,572`.

Those `3,572` rows are not unattempted backlog. They are all explained by terminal `valuation_attempts`:

- `failed_missing_oracle`: `3,518`
- `failed_missing_pool`: `36`
- `failed_sequencer_outage`: `18`

There are `0` residual rows with no attempt record under either version.

So the main issue is not broad missing coverage for `Transfer` / `Bond` / etc. The real issue is that the current spec / acceptance wording expects a valuation row for every valuable event, while the implementation intentionally models some terminal outcomes as failed attempts without an `event_valuations` row.

## Why this is still a correctness issue

The spec says:

- §1.1: "Compute USD valuations for every monetary event at block-level precision"
- §24.1: "`event_valuations` rows for every finalized, valuable event"

The event catalog marks these as monetary / `is_valuable = TRUE`:

- `Transfer`
- `Bond`
- `Unbond`
- `Rebond`
- `WithdrawStake`
- `TransferBond`
- `Reward`
- `WithdrawFees`
- `WinningTicketRedeemed`
- `WinningTicketTransfer`
- `DepositFunded`
- `ReserveFunded`
- `Withdrawal`
- `Mint`
- `Burn`
- `EarningsClaimed` (multi-asset)

The implementation today does have coverage for the monetary classes under the two-version model:

- single-asset seed-hit rows
- ETH on-chain rows
- LPT on-chain rows
- `EarningsClaimed` split rows

The remaining mismatch is that terminal failures currently count as "missing valuations" if you only inspect `event_valuations`, even though the system has fully processed them and recorded the outcome in `valuation_attempts`.

## Residual set shape

If you ignore degraded-version valuations, the apparent residual set is dominated by:

- `Transfer|LPT`
- `Bond|LPT`
- `Unbond|LPT`
- `Mint|LPT`
- `Rebond|LPT`
- `TransferBond|LPT`
- `WithdrawStake|LPT`
- `WithdrawFees|ETH`

But those rows are mostly explained by the degraded-version path, not missing implementation coverage.

The true residual set after counting both versions is only `3,572` rows, all of which already have terminal attempt statuses.

## Decision

Before more code is written, we need an explicit product/spec decision on terminal failures:

### Model A — strict row completeness

Every `is_valuable = TRUE` event must produce at least one `event_valuations` row, even for failure outcomes.

Implication:

- introduce a failure-valued row shape or placeholder-valued row shape
- redefine how failed oracle / failed pool / sequencer outage outcomes are stored in `event_valuations`

### Model B — attempt completeness

Every `is_valuable = TRUE` event must produce either:

- an `event_valuations` row, or
- a terminal `valuation_attempts` outcome explaining why no valuation row exists

Implication:

- keep the current storage model
- adjust spec wording and acceptance criteria to count terminal attempts as complete processing

Current implementation originally behaved like Model B.

## Chosen direction

Operator decision on 2026-05-03: **Model A**.

That means:

1. every terminal valuation failure must also emit an immutable `event_valuations` row
2. `valuation_attempts` remains the append-only audit log
3. failure outcome rows carry nullable USD price fields and terminal `status`

## Proposed implementation order

### Phase 1 — schema + write-path changes

Update `event_valuations` to support terminal failure rows:

- widen `status`
- allow nullable `native_usd_price` / `amount_usd`
- write failure outcomes from the on-chain valuator passes
- backfill existing terminal failures from `valuation_attempts`

### Phase 2 — update residual/backlog queries

Replace the current broad "unvalued valuable events" query in runbooks/debugging with:

- active backlog query = no valuation row under either version AND no terminal attempt
- completed-with-terminal-failure query = no valuation row under either version BUT has terminal failure attempt

### Phase 3 — deterministic replay validation

After semantics/query updates:

1. clean rerun from preserved inputs
2. confirm true active backlog query reaches 0
3. compare final DB against baseline expectations
4. ensure no new retry loops or misclassified permanent failures appear

## Acceptance criteria

- Every terminal valuation failure also has an `event_valuations` row.
- A true active-backlog query returns 0 after a completed rerun.
- Terminal-failure rows are separately queryable and reported by status.
- Acceptance criteria no longer misclassify degraded-version rows or terminal-failure rows as unfinished backlog.

## Progress log

### 2026-05-03

- Confirmed via completed fresh rerun that the broad one-version residual query overstated the problem.
- Measured `225,920` rows under the degraded valuation version.
- Measured true residual after counting both versions: `3,572`.
- Measured terminal attempt breakdown for those residual rows: `3,518 missing_oracle`, `36 missing_pool`, `18 sequencer_outage`, `0 no-attempt`.
- Chose Model A: every valuable event must have an `event_valuations` row.
- Implemented migration `017_event_valuations_terminal_failures` to widen `event_valuations.status`, make USD price fields nullable, and backfill existing terminal failures from `valuation_attempts`.
- Updated valuator bulk/on-chain write paths so `failed_missing_oracle`, `failed_missing_pool`, and `failed_sequencer_outage` now persist immutable `event_valuations` outcome rows in addition to `valuation_attempts`.
