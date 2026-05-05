---
title: Gateway Phase 3
status: in_progress
opened: 2026-05-04
owner: codex
links:
  - spec: ../../product-specs/v1-livepeer-indexer.md#64-ticketbroker-events
  - design: ../../design-docs/gateway-ticketbroker-data-model.md
  - phase2: ./td-014-gateway-phase-2.md
---

## Goal

Add claimant-level TicketBroker reserve state and a materialized gateway flow
ledger so gateway analytics no longer depend on expensive raw-event joins and
can answer recipient-level payout questions directly.

## Scope

- Add `gateway_claimants_by_block`
- Add `gateway_flows`
- Define claimant discovery and bounded reconciliation strategy
- Define API target surface for claimant and flow queries

## Non-goals

- Changing valuation semantics for existing TicketBroker event types
- Replacing `raw_protocol_events` as the canonical event ledger
- Full UI/reporting layer

## Approach

1. Materialize claimant-level state from:
   - `claimableReserve(reserveHolder, claimant)`
   - `claimedReserve(reserveHolder, claimant)`
2. Build a bounded claimant universe from gateway-touching events:
   - `WinningTicketRedeemed`
   - `WinningTicketTransfer`
   - `ReserveClaimed`
3. Materialize `gateway_flows` from canonical TicketBroker events plus priced
   valuation rows where available.
4. Add API read paths that prefer materialized flow rows for analytics-heavy
   queries.

## Candidate API surface

- `GET /gateways/{gateway}/claimants/block/{block}`
- `GET /gateways/{gateway}/claimants/history`
- `GET /gateways/{gateway}/payouts`
- `GET /gateways/{gateway}/recipients`
- `GET /gateways/{gateway}/analytics/summary`

## Remaining implementation work

- Define the claimant backfill universe and its pruning rules
- Decide whether phase 3 lives in the phase-2 gateway worker or a second pass
- Implement paired-event handling for:
  - `WinningTicketTransfer`
  - `WinningTicketRedeemed`
- Define whether `gateway_flows` should store:
  - both gross paired-event volume
  - and a net-payout interpretation field
- Add validation:
  - sampled claimant rows vs on-chain `claimableReserve` / `claimedReserve`
  - sampled materialized flow rows vs `raw_protocol_events` + `event_valuations`

## Progress log

- 2026-05-04: Added target migrations:
  - `023_create_gateway_claimants_by_block`
  - `024_create_gateway_flows`
- 2026-05-04: Split claimant/materialized-flow work out of phase 2 so sender
  balance snapshots can ship first without scope creep.
