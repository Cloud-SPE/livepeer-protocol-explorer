---
title: valuation coverage gap
status: done
opened: 2026-05-03
closed: 2026-05-03
owner: codex+mazup
links:
  - spec: ../../product-specs/v1-livepeer-indexer.md
  - tracker: ../tech-debt-tracker.md
---

## Goal

Resolve the mismatch between the spec's "value every monetary event" contract and the
implementation's end-state accounting, especially around degraded-version valuations
and terminal failure outcomes.

## Final summary

The apparent gap was initially overstated by a one-version residual query. Once
degraded-version rows were counted, the true residual collapsed to a small set of
terminal failures. The operator chose **Model A** on 2026-05-03: every valuable event
must also have an immutable `event_valuations` row, including terminal failures.

That choice is now implemented and validated:

- Migration `017_event_valuations_terminal_failures` widened the outcome table so
  terminal failures persist there with nullable USD fields.
- Valuator write paths now emit immutable outcome rows for:
  - `failed_missing_oracle`
  - `failed_missing_pool`
  - `failed_sequencer_outage`
- Replay-DB validation confirmed:
  - latest terminal attempts missing a matching `event_valuations` row: `0`
  - true active backlog under the two-version model: `0`
- Spec and acceptance wording were aligned to the shipped outcome-row contract.

## Evidence captured during closure

- `v1_lpt_weth_twap_30min_x_chainlink_eth`: `2,143,953` rows
- `v1_degraded_spot_pre_cardinality`: `225,920` rows
- True residual after counting both versions: `3,572`
- Terminal attempt breakdown for the true residual:
  - `failed_missing_oracle`: `3,518`
  - `failed_missing_pool`: `36`
  - `failed_sequencer_outage`: `18`
  - unattempted rows: `0`

## Follow-up

No semantics gap remains. Future work is ordinary reporting and operator
ergonomics, not valuation-completeness design.

## Progress log

- 2026-05-03: Verified the broad one-version residual query overstated the problem.
- 2026-05-03: Chose Model A — every valuable event must have an `event_valuations` row.
- 2026-05-03: Implemented migration `017_event_valuations_terminal_failures`.
- 2026-05-03: Updated valuator write paths to persist terminal failure outcome rows.
- 2026-05-03: Applied migration `017` to the replay DB and verified closure conditions.
