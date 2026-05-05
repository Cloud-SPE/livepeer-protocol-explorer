---
title: Gateway Backfill Operability
status: in_progress
opened: 2026-05-05
owner: codex
links:
  - spec: ../../product-specs/v1-livepeer-indexer.md#64-ticketbroker-events
  - design: ../../design-docs/gateway-ticketbroker-data-model.md
  - tracker: ../tech-debt-tracker.md
---

## Problem

Gateway phase 2 and phase 3 are implemented, but the historical backfill shape
is operationally weak.

The current `livepeer-staker` gateway worker:

1. computes the full historical balance candidate set,
2. computes the full historical flow candidate set,
3. processes **all** balance candidates first with per-candidate RPC
   reconciliation,
4. only after that begins writing `gateway_flows`,
5. only after flow rows begin writing claimant rows.

That means a large historical run can show:

- `gateway_balances_by_block` slowly increasing,
- `gateway_flows = 0`,
- `gateway_claimants_by_block = 0`,

even though the code paths for flow and claimant materialization exist and are
wired into daemon follow mode.

This is not a missing-implementation bug. It is a sequencing / checkpointing /
operability problem.

## Evidence

The current code confirms the behavior:

- daemon follow mode runs `run_gateway_backfill(...)` inside the staker loop:
  [crates/livepeer-daemon/src/supervisor.rs](../../../crates/livepeer-daemon/src/supervisor.rs:365)
- `run_gateway_backfill(...)` materializes:
  - balances first,
  - then flows,
  - then claimant rows:
  [crates/livepeer-staker/src/gateway.rs](../../../crates/livepeer-staker/src/gateway.rs:67)
- flow candidates are implemented and `upsert_gateway_flow(...)` is called:
  [crates/livepeer-staker/src/gateway.rs](../../../crates/livepeer-staker/src/gateway.rs:205)

Observed local historical candidate sizes during investigation:

- balance candidates: about `298,820`
- flow candidates: about `597,903`

With the current ordering, flow and claimant tables can remain empty for a long
time simply because the balance pass has not finished yet.

## Goal

Make gateway historical materialization incremental and observable enough that:

1. gateway flows begin materializing without waiting for a full sender-balance
   historical replay,
2. claimant rows begin materializing without waiting for the entire historical
   balance domain,
3. progress survives process restarts,
4. operators can see which gateway phase is advancing or stalled.

## Non-goals

- changing gateway API semantics,
- changing the underlying TicketBroker state model,
- removing exact RPC reconciliation,
- introducing multi-instance concurrency or cross-process locking.

## Target shape

Split gateway historical work into independently resumable phases:

1. **Balance phase**
   - materialize `gateway_balances_by_block`
   - checkpoint independently

2. **Flow phase**
   - materialize `gateway_flows`
   - checkpoint independently
   - must not wait for full balance completion

3. **Claimant phase**
   - materialize `gateway_claimants_by_block`
   - checkpoint independently
   - may depend on flow rows, but not on total balance completion

4. **Follow-mode incremental phase**
   - small near-head bounded iterations for all 3 phases
   - never attempt a full-history replay in a single daemon tick

## Proposed implementation

### Phase A — independent phase checkpoints

Add explicit gateway checkpoints rather than relying on one monolithic worker
call to “finish everything”.

Suggested checkpoint keys:

- `gateway_balance_backfill`
- `gateway_flow_backfill`
- `gateway_claimant_backfill`

Each should track at least:

- last processed block,
- last processed event/log ordering cursor if needed,
- updated timestamp.

This can reuse `indexer_checkpoints` if desired, or use a dedicated table if
that produces a cleaner schema.

### Phase B — bounded batch reads

Refactor candidate fetches so each phase operates on a bounded slice:

- `fetch_balance_candidates_after(cursor, limit)`
- `fetch_flow_candidates_after(cursor, limit)`
- `fetch_claimant_candidates_after(cursor, limit)`

The worker should return after one bounded batch, not after exhausting history.

This makes daemon follow mode safe and resumable.

### Phase C — phase decoupling

Stop forcing:

- all balances first,
- all flows second,
- all claimants third.

Instead:

- each daemon tick runs a bounded balance batch,
- each daemon tick runs a bounded flow batch,
- each daemon tick runs a bounded claimant batch.

That lets `gateway_flows` become useful quickly even if historical sender
balance replay is still catching up.

### Phase D — flow-first operator usefulness

For user-facing usefulness, flows should become queryable before full balance
history completes.

Practical priority:

1. bounded flow batches
2. bounded claimant batches
3. bounded balance batches

or at minimum:

- execute flow batches before large balance batches in follow mode

if exact per-block gateway balance history is less urgent than payout/funding
visibility.

### Phase E — metrics and logs

Add explicit gateway metrics:

- `gateway_balance_candidates_remaining`
- `gateway_flow_candidates_remaining`
- `gateway_claimant_candidates_remaining`
- `gateway_balance_rows_written_total`
- `gateway_flow_rows_written_total`
- `gateway_claimant_rows_written_total`
- `gateway_backfill_last_processed_block{phase=...}`

And structured logs per bounded phase iteration:

- candidate count,
- rows written,
- current cursor,
- elapsed time.

### Phase F — API source transparency

Keep or extend the existing `source` semantics on gateway balance rows so
operators and callers can tell whether a response came from:

- historical materialization,
- on-demand RPC hydration,
- or both.

## Validation plan

1. Run a bounded gateway backfill from a DB with empty gateway tables.
2. Confirm:
   - `gateway_flows` starts filling before total balance completion,
   - `gateway_claimants_by_block` starts filling before total balance completion.
3. Kill the worker mid-run and restart.
4. Confirm phase checkpoints resume correctly.
5. In daemon follow mode, verify that one tick does bounded work only.
6. Benchmark representative API routes after partial historical completion:
   - `/gateways/{gateway}/flows`
   - `/gateways/{gateway}/payouts`
   - `/gateways/{gateway}/claimants/history`
   - `/gateways/{gateway}/balance/history`

## Concrete task list

- [ ] Add independent gateway phase checkpoints
- [ ] Refactor gateway candidate fetches into bounded batch functions
- [ ] Refactor `run_gateway_backfill()` into phase-specific bounded runners
- [ ] Update daemon staker loop to call the bounded phase runners
- [ ] Add gateway phase metrics
- [ ] Add progress-oriented structured logging
- [ ] Validate restart/resume semantics
- [ ] Rebenchmark gateway endpoints after partial and full backfill

## Progress log

- 2026-05-05: Investigation confirmed this is not a missing code path.
  `gateway_flows` and claimant writes are implemented, but the monolithic
  balance-first replay shape starves them operationally during long historical
  runs.
- 2026-05-05: Code-prep landed locally for bounded gateway phases with
  independent checkpoint keys under `indexer_checkpoints`
  (`gateway_flow_backfill`, `gateway_claimant_backfill`,
  `gateway_balance_backfill`). The refactor compiles, but has not been applied
  to the running local runtime while the separate Governor repair backfill is in
  progress.
