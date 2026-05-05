---
title: Gateway Phase 2
status: done
opened: 2026-05-04
closed: 2026-05-04
owner: codex
links:
  - spec: ../../product-specs/v1-livepeer-indexer.md#64-ticketbroker-events
  - design: ../../design-docs/gateway-ticketbroker-data-model.md
  - tracker: ../tech-debt-tracker.md
---

## Goal

Materialize historical TicketBroker gateway sender balances in Postgres so
gateway balance history no longer depends on live `eth_call` for every queried
block.

## Final summary

Phase 2 is now implemented.

What shipped:

- Migration `022_create_gateway_balances_by_block`
- A bounded gateway backfill worker under `livepeer-staker`
- Daemon integration so follow mode keeps gateway sender snapshots current
- API balance endpoints that prefer materialized rows before falling back to live RPC
- A new materialized history endpoint:
  - `GET /gateways/{gateway}/balance/history`

The implementation uses exact TicketBroker state reads at each gateway-touching
event block:

- `getSenderInfo(address)`
- `isUnlockInProgress(address)`

Those calls flow through `rpc_call_cache` via `cross_check::single_call_cached`,
so the backfill remains deterministic under replay.

## What remains

Phase 2 closure intentionally does **not** include claimant-level reserve state
or a materialized gateway flow ledger. Those are now tracked separately under
[td-015-gateway-phase-3.md](../active/td-015-gateway-phase-3.md).

## Progress log

- 2026-05-04: Phase 1 API shipped in commit `2b4b224` with exact RPC-backed
  balance endpoints plus indexed flow/summary endpoints.
- 2026-05-04: Added migration `022_create_gateway_balances_by_block` and
  opened the phase-2 execution plan.
- 2026-05-04: Implemented gateway backfill worker in `livepeer-staker`.
- 2026-05-04: Wired gateway backfill into daemon follow mode.
- 2026-05-04: Updated gateway balance API routes to prefer materialized rows.
