---
title: Gateway TicketBroker Data Model
status: accepted
verified: 2026-05-04
---

# Gateway TicketBroker Data Model

## Decision

Model Livepeer "gateway" data as TicketBroker sender state plus TicketBroker flow history.

Phase 1 ships:
- exact point-in-time balance reads from `TicketBroker.getSenderInfo(sender)` and
  `TicketBroker.isUnlockInProgress(sender)` exposed via the API
- indexed flow history and rolling summaries sourced from `raw_protocol_events`

Phase 2 target schema:
- `gateway_balances_by_block`

Phase 3 target schema:
- `gateway_claimants_by_block`
- `gateway_flows` materialized convenience table

## Why

Gateway economics have two different shapes that should not be collapsed:

1. sender balance state
   - deposit
   - reserve funds remaining
   - reserve claimed in current round
   - withdraw round
   - unlock state

2. economic flow history
   - deposit funding
   - reserve funding
   - ticket redemption / payout
   - reserve transfer
   - reserve claimed
   - withdrawal

The same split already works well elsewhere in the system:
- stake exact state vs stake event history
- transcoder params/lifecycle history vs current point-in-time profile

## TicketBroker mapping

### Exact state getters

- `getSenderInfo(address)` returns:
  - `sender.deposit`
  - `sender.withdrawRound`
  - `reserve.fundsRemaining`
  - `reserve.claimedInCurrentRound`
- `isUnlockInProgress(address)` returns the sender unlock state

### Indexed flow events

- `DepositFunded` -> `deposit_in`
- `ReserveFunded` -> `reserve_in`
- `WinningTicketTransfer` -> `reserve_transfer`
- `WinningTicketRedeemed` -> `ticket_redeemed`
- `ReserveClaimed` -> `reserve_claimed`
- `Withdrawal` -> `withdrawal`
- `Unlock` / `UnlockCancelled` are state transitions, not funding/payout rows

## Phase 1 API surface

- `GET /gateways/{gateway}/balance/latest`
- `GET /gateways/{gateway}/balance/block/{block}`
- `GET /gateways/{gateway}/flows`
- `GET /gateways/{gateway}/summary`

These endpoints are enough to answer:
- what is the sender's deposit/reserve now?
- what was the sender's exact balance at block `N`?
- what were the recent payout/funding events?
- what are the 7-day / 30-day funding and payout totals?

## Phase 2 target schema

### `gateway_balances_by_block`

Key:
- `(chain_id, gateway_address, block_number)`

Columns:
- `chain_id bigint not null`
- `gateway_address text not null`
- `block_number bigint not null`
- `block_timestamp timestamptz not null`
- `block_hash text not null`
- `deposit numeric(78,18) not null`
- `reserve_funds_remaining numeric(78,18) not null`
- `reserve_claimed_in_current_round numeric(78,18) not null`
- `withdraw_round bigint not null`
- `unlock_in_progress boolean not null`
- `source text not null`
- `source_event_id bigint`
- `created_at timestamptz not null default now()`

Indexes:
- primary key `(chain_id, gateway_address, block_number)`
- `(chain_id, gateway_address, block_number desc)`

## Phase 3 target schema

### `gateway_claimants_by_block`

Key:
- `(chain_id, gateway_address, claimant_address, block_number)`

Columns:
- `chain_id bigint not null`
- `gateway_address text not null`
- `claimant_address text not null`
- `block_number bigint not null`
- `block_timestamp timestamptz not null`
- `block_hash text not null`
- `claimable_reserve numeric(38,18) not null`
- `claimed_reserve numeric(38,18) not null`
- `source text not null`
- `triggering_event_id bigint`
- `created_at timestamptz not null default now()`

Primary use:
- claimant-level payout/release state at a historical block
- reserve recipient analytics that cannot be answered from sender-only balance state

### `gateway_flows`

Key:
- `id bigint generated always as identity primary key`

Columns:
- `chain_id bigint not null`
- `event_id bigint not null references raw_protocol_events(id)`
- `gateway_address text not null`
- `claimant_address text`
- `counterparty_address text`
- `block_number bigint not null`
- `block_timestamp timestamptz not null`
- `tx_hash text not null`
- `log_index integer not null`
- `event_name text not null`
- `flow_kind text not null`
- `asset text`
- `amount_native numeric(38,18)`
- `amount_usd numeric(38,18)`
- `valuation_version text`
- `created_at timestamptz not null default now()`

Primary use:
- faster gateway analytics than joining `raw_protocol_events` + `event_valuations`
- preclassified funding/payout rows
- easier recipient/gateway leaderboards and time-bucket summaries

## Important nuance

`WinningTicketTransfer` and `WinningTicketRedeemed` often appear as a paired payout
lifecycle. Consumers should not blindly sum both as independent net payouts unless
they explicitly want "all payout-related event volume" rather than net economic
outflow.

`ReserveClaimed` is tracked as a flow, but valuation semantics should be handled
carefully if it overlaps economically with `WinningTicketRedeemed.faceValue`.
