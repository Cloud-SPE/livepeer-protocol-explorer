---
title: Gateway Phase 2
status: in_progress
opened: 2026-05-04
owner: codex
links:
  - spec: ../../product-specs/v1-livepeer-indexer.md#64-ticketbroker-events
  - design: ../../design-docs/gateway-ticketbroker-data-model.md
---

## Goal

Materialize historical TicketBroker gateway sender balances in Postgres so
gateway balance history no longer depends on live `eth_call` for every queried
block.

## Scope

- Add `gateway_balances_by_block` schema
- Add a bounded worker/backfill path that:
  - scans TicketBroker gateway-touching events
  - writes event-block snapshots keyed by `(chain_id, gateway_address, block_number)`
  - reconciles exact state from `getSenderInfo()` and `isUnlockInProgress()`
- Switch API historical balance lookups to prefer the materialized table and use
  live RPC only as a fallback or debugging path

## Non-goals

- Claimant-level state materialization
- Separate `gateway_flows` table unless API pressure proves raw-event queries are too heavy
- Revaluing `ReserveClaimed` semantics beyond current event tracking

## Approach

1. Land the schema migration for `gateway_balances_by_block`.
2. Add a `livepeer-gateway` worker module or extend the staker-style post-indexer
   path with bounded gateway snapshot writes.
3. Reuse `rpc_call_cache` + `cross_check::single_call_cached` so historical gateway
   state remains deterministic under replay.
4. Backfill gateway balances from existing TicketBroker sender-side events:
   - `DepositFunded`
   - `ReserveFunded`
   - `WinningTicketTransfer`
   - `WinningTicketRedeemed`
   - `ReserveClaimed`
   - `Withdrawal`
   - `Unlock`
   - `UnlockCancelled`
5. Update API routes:
   - `/gateways/{gateway}/balance/block/{block}`
   - `/gateways/{gateway}/balance/latest`
   to read the materialized table first.

## Remaining implementation work

- Identify the minimal gateway address universe to backfill efficiently
  from `raw_protocol_events`
- Decide whether the worker should live under `livepeer-staker` or a new
  gateway-focused crate/module
- Implement snapshot write semantics for repeated same-block updates
- Add acceptance validation:
  - compare sampled `gateway_balances_by_block` rows against on-chain
    `getSenderInfo()` at the same block

## Progress log

- 2026-05-04: Phase 1 API shipped in commit `2b4b224` with exact RPC-backed
  balance endpoints plus indexed flow/summary endpoints.
- 2026-05-04: Added migration `022_create_gateway_balances_by_block` to make the
  phase-2 target schema explicit before worker implementation.
