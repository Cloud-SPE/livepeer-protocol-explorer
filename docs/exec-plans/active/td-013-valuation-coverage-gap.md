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

Resolve the mismatch between the spec's "value every monetary event" contract and the current valuator implementation, which finishes with zero active candidates but leaves a large residual set of `is_valuable = TRUE` events without `event_valuations` rows.

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
- but the broad residual query still returns `226,256` finalized, canonical, `is_valuable = TRUE` rows with no valuation row at `valuation_version = 'v1_lpt_weth_twap_30min_x_chainlink_eth'`

This is not active backlog. It is a coverage/scope mismatch.

## Why this is a correctness issue

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

The implementation today only has explicit valuation coverage for:

- single-asset seed-hit rows
- ETH on-chain rows
- LPT on-chain rows
- `EarningsClaimed` split rows

That leaves other `is_valuable = TRUE` classes with no valuation path.

## Residual set shape

The residual set observed during the 2026-05-03 investigation is dominated by:

- `Transfer|LPT`
- `Bond|LPT`
- `Unbond|LPT`
- `Mint|LPT`
- `Rebond|LPT`
- `TransferBond|LPT`
- `WithdrawStake|LPT`
- `WithdrawFees|ETH`
- `EarningsClaimed|NULL` was previously present during partial runs, but the completed fresh rerun's active candidate selectors for the multi-asset pass returned `0`

The overwhelming majority is plain LPT transfer/stake-flow classes.

## Decision that must be made first

Before more code is written, we need an explicit product decision on these event classes:

1. `Transfer`
2. `Mint`
3. `Burn`

There are two coherent models:

### Model A — spec literal

`is_valuable = TRUE` means "must produce at least one `event_valuations` row".

Implication:

- add valuation coverage for every residual monetary class
- valuator must eventually drive the broad residual query to 0 for the active version

### Model B — narrower valuation scope

Some events may be economically meaningful but intentionally excluded from `event_valuations` in v1.

Implication:

- narrow spec language
- narrow acceptance criteria
- narrow `is_valuable` tagging semantics or add a second flag separating:
  - "monetary / economically relevant"
  - "must be valued in v1"

Current repo state does not support Model B cleanly. The spec and tags currently encode Model A.

## Recommendation

Treat the spec as authoritative and implement Model A unless the user explicitly approves a spec change.

That means:

1. keep `is_valuable` semantics as "must be valued"
2. add missing valuation coverage for the residual classes
3. only change the spec if we intentionally decide some classes should remain unvalued in v1

## Proposed implementation order

### Phase 1 — clarify intended pricing semantics per class

For each residual class, define the intended pricing source:

- `Transfer` / `Bond` / `Unbond` / `Rebond` / `TransferBond` / `WithdrawStake` / `Mint` / `Burn`
  - LPT/USD at event block
  - should be priceable by the same seed / on-chain LPT paths already used elsewhere
- `WithdrawFees`
  - ETH/USD at event block
  - should be priceable by the same ETH path already used elsewhere

Expected result: these are not new pricing primitives; they are new candidate classes for existing primitives.

### Phase 2 — widen candidate selection

Update valuator candidate fetches so existing pricing paths pick up all eligible event classes rather than an accidental subset.

Likely work:

- audit `seed.rs` candidate shape for which event families are actually entering the single-asset path
- audit `onchain.rs` `fetch_eth_candidates` / `fetch_lpt_candidates`
- verify whether residual rows are being filtered by event-name assumptions, prior-failure assumptions, or asset/null handling

### Phase 3 — deterministic replay validation

After widening coverage:

1. clean rerun from preserved inputs
2. confirm residual broad query drops materially or reaches 0
3. compare final DB against baseline expectations
4. ensure no new retry loops or misclassified permanent failures appear

## Acceptance criteria

- The intended scope decision is explicit and documented.
- If spec remains unchanged:
  - the broad residual query for finalized, canonical, `is_valuable = TRUE` rows without valuations is 0 for the active version, except for explicitly documented permanent-failure statuses that still create valuation-attempt evidence.
- If scope is narrowed:
  - spec language and acceptance criteria are updated
  - `is_valuable` semantics are made internally consistent
  - residual rows outside scope are no longer counted as missing expected valuations

## Progress log

### 2026-05-03

- Confirmed via completed fresh rerun that the valuator finishes with zero active candidates but leaves a large residual set of `is_valuable = TRUE` rows without valuations.
- Confirmed this is not a runtime throughput issue.
- Confirmed the spec currently requires complete coverage for finalized valuable events, so the mismatch is real unless the spec is intentionally changed.
