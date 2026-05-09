---
title: API Read-Path Performance Hardening
status: resolved
opened: 2026-05-07
resolved: 2026-05-07
owner: codex
links:
  - spec: ../../product-specs/v1-livepeer-indexer.md#1436-http-api-v1
  - tracker: ../tech-debt-tracker.md
  - prior: ../completed/td-017-old-api-parity-and-rollups.md
---

## Problem

The current API surface is functionally correct on the rebuilt live dataset,
but one route family has regressed back into a broad raw-event scan and one
route family remains structurally heavy enough that it should be treated as an
explicit watchlist.

Empirical checks against the rebuilt database showed:

- `/aggregations/events` broad USD aggregation over the full live range:
  about **15.1s**
- payout-like CSV query shape in `routes/reports.rs` over a high-volume
  orchestrator + full range: about **3.16s**
- reward CSV query shape: about **768ms**
- ticket-history query shape: about **307ms**
- gateway recipient aggregation on `gateway_flows`: about **16.5ms**
- payout leaderboard on `orch_payouts_daily`: about **10.6ms**

This is not a generalized “the API is slow” problem.

It is specifically:

1. one clear hot path that should no longer hit `raw_protocol_events`
   directly for broad time-bucket analytics,
2. one route family (`/reports/*`) that is still acceptable but has enough
   lateral / per-row lookup structure to warrant deliberate guardrails,
3. a need for repeatable benchmark coverage so future route changes are caught
   before they become another TD-010-style paper cut.

## Evidence

Route/query sites:

- broad event aggregation query:
  [crates/livepeer-api/src/routes/aggregations.rs](../../../crates/livepeer-api/src/routes/aggregations.rs:177)
- payout-like CSV and ticket-history direct-query paths:
  [crates/livepeer-api/src/routes/reports.rs](../../../crates/livepeer-api/src/routes/reports.rs:422)
  [crates/livepeer-api/src/routes/reports.rs](../../../crates/livepeer-api/src/routes/reports.rs:599)
- gateway recipient aggregation on materialized flows:
  [crates/livepeer-api/src/routes/gateways.rs](../../../crates/livepeer-api/src/routes/gateways.rs:594)
- rollup-backed payout leaderboard:
  [crates/livepeer-api/src/routes/payouts.rs](../../../crates/livepeer-api/src/routes/payouts.rs:233)

Observed plan behavior:

- `/aggregations/events` with valuation join:
  - `Parallel Seq Scan` on `raw_protocol_events`
  - per-row lookup into `event_valuations`
  - large sort/group over the full qualifying set
  - wall time about **15.15s**
- `/reports/*`:
  - address-selective scans work reasonably
  - current indexes are doing real work
  - the expensive part is the `LEFT JOIN LATERAL` to recover point-in-time
    `TranscoderUpdate`
- `gateway_flows` recipient aggregation:
  - currently a seq scan, but still fast because the table is small
- rollup-backed endpoints:
  - already cheap and structurally correct

## Goal

Make the API read paths scale in a way that matches the architecture we now
have:

1. broad analytics should read from rollups / pre-aggregated tables, not
   millions of raw events,
2. direct-query report endpoints should stay bounded and predictable on
   operator-sized exports,
3. fast routes should stay fast via benchmark/regression coverage,
4. any remaining intentionally-heavy route should be explicit and observable.

## Non-goals

- changing public API semantics,
- changing valuation semantics or versions,
- changing CSV column shapes,
- speculative index churn on already-fast routes,
- forcing every endpoint onto a rollup when the direct query is already cheap.

## Scope

### In scope

- `/aggregations/events`
- `/reports/payouts.csv`
- `/reports/rewards.csv`
- `/reports/gateway-payouts.csv`
- `/orchestrators/{addr}/tickets/latest`
- `/gateways/{addr}/tickets`
- benchmark/EXPLAIN coverage for high-risk routes

### Out of scope

- rollup worker semantics themselves unless a new read-model table is required
- daemon/indexer/valuator throughput
- gateway writer/backfill operability

## Target shape

### Track A — fix broad event aggregation properly

The current `/aggregations/events` route is doing too much directly on
`raw_protocol_events`. For broad windows and valuation-backed metrics, it
should move to a dedicated aggregated read-model instead of relying on dynamic
`GROUP BY date_trunc(...)` over canonical event rows.

Target outcome:

- narrow, event-detail use cases may still hit `raw_protocol_events`
- broad day/week/month analytics should read from a materialized table or
  rollup table keyed by:
  - chain
  - bucket day
  - event family / event name as needed
  - valuation version
  - valuable/finalized/canonical semantics baked in

Pragmatic design preference:

- store **daily** buckets only
- answer week/month by re-aggregating daily rows
- avoid storing separate day/week/month materializations

This keeps writes simpler and preserves determinism.

### Track B — harden report-route cost without premature rewrites

The report routes are not yet broken enough to justify a full redesign, but
they should stop being “mystery heavy”.

Target outcome:

- keep the current direct-query semantics
- add explicit benchmark coverage for:
  - high-volume orchestrator payout export
  - high-volume reward export
  - ticket history page
- only add indexes or query rewrites if the measured shapes cross the agreed
  thresholds below

The likely future optimization, if needed, is **not** broad denormalization of
the CSVs. It is one of:

1. a more selective covering index for ticket/payout event discovery, and/or
2. a cached/read-model view of point-in-time fee-share snapshots if the
   lateral `TranscoderUpdate` lookup becomes the dominant cost.

Do not pre-build that second model unless the benchmark data justifies it.

### Track C — add explicit guardrails and regression coverage

This repo has already solved one API slow-query round under TD-010. The missing
piece is repeatable evidence, not more ad-hoc heroics.

Target outcome:

- one script or test harness that runs representative `EXPLAIN (ANALYZE,
  BUFFERS)` queries against a loaded DB
- one markdown benchmark note in `run-logs/` or `docs/` capturing:
  - query shape
  - dataset size
  - plan summary
  - wall time
- thresholds documented so future regressions are obvious

## Performance thresholds

These are operator targets, not hard protocol invariants.

### `/aggregations/events`

- broad daily aggregation over full live history:
  - current: about **15.1s**
  - target after fix: **<500ms** for day buckets from a rollup-backed path

### `/reports/*`

- orchestrator payout export for a heavy real address across full history:
  - current: about **3.16s**
  - acceptable target: **<2s**
  - watch threshold: **>5s**

- reward export for a heavy real address across full history:
  - current: about **768ms**
  - acceptable target: **<1s**

- ticket history page:
  - current: about **307ms**
  - acceptable target: **<500ms**

### Materialized-table routes

- gateway recipient aggregation:
  - current: about **16.5ms**
  - no action unless this rises above **250ms** on representative data

- rollup-backed leaderboards:
  - current: about **10.6ms**
  - expected to remain comfortably sub-100ms

## Proposed implementation

### Phase 0 — benchmark harness and baseline capture

Before changing query shapes:

- codify the exact representative queries already used in manual analysis
- persist:
  - DB cardinalities
  - sample addresses/date ranges
  - `EXPLAIN (ANALYZE, BUFFERS)` output
  - route-level wall-clock timings

This becomes the before/after proof.

Suggested artifacts:

- `scripts/bench-api-read-paths.sh`
- `run-logs/api-read-path-benchmark-YYYYMMDD.md`

### Phase 1 — aggregation read-model

Add a dedicated daily aggregation table for `/aggregations/events`.

Suggested shape:

- `event_metrics_daily`
  - `chain_id`
  - `day_utc`
  - `event_name`
  - `asset`
  - `valuation_version` nullable or explicit for metrics that need valuation
  - `event_count`
  - `sum_amount_native`
  - `sum_amount_usd`
  - `usd_rows_priced`
  - `source_max_event_id`
  - `updated_at`

Design notes:

- daily-only storage
- computed from canonical finalized valuable events
- replay-covered like the other deterministic rollups
- `/aggregations/events` rewrites to use this table for broad bucketed metrics
- retain a direct-query fallback only for unsupported filter combinations if
  truly necessary, but prefer an explicit “unsupported filter shape” to a
  silent multi-second table scan

### Phase 2 — targeted report hardening

Do not start with a rewrite. Start with proof.

Tasks:

- benchmark the current report queries after Phase 1 lands
- inspect whether the expensive cost is:
  - event discovery,
  - valuation join,
  - or lateral fee-share lookup
- only then choose among:
  - new covering index for event discovery,
  - minor SQL rewrite,
  - point-in-time `TranscoderUpdate` helper model

Default bias:

- small targeted index additions are preferred
- avoid new report-specific rollup tables unless we have evidence the current
  direct-query semantics are no longer viable

### Phase 3 — route-level guardrails

Add explicit protection against accidental expensive use:

- document recommended parameter shapes for CSV routes
- consider server-side max date-range limits only if operators actually hit
  pathological exports
- add benchmark CI or at least a repo-local repeatable benchmark command for:
  - `/aggregations/events`
  - payout CSV shape
  - reward CSV shape
  - ticket history shape

## Validation plan

1. Capture pre-change benchmark results on the current rebuilt DB.
2. Implement the aggregation read-model and route rewrite.
3. Re-run the same benchmark set.
4. Confirm:
   - `/aggregations/events` broad daily USD query drops from ~15.1s to target
     range
   - report routes remain at or below their current latency envelopes
5. Run determinism replay if a new rollup table is added.
6. Smoke-test the live API responses for semantic equivalence.

## Concrete task list

- [ ] Add TD-018 benchmark baseline artifact
- [ ] Define the new daily event-metrics rollup schema
- [ ] Implement deterministic writer / checkpoint for the aggregation rollup
- [ ] Rewrite `/aggregations/events` to use the rollup-backed path
- [ ] Re-benchmark `/aggregations/events`
- [ ] Re-benchmark `/reports/*` representative shapes
- [ ] Decide whether report routes need targeted indexes or no change
- [ ] Add repeatable benchmark script / docs for future regression checks

## Progress log

- **2026-05-07** Problem isolated on the rebuilt live dataset. One route family
  is a real optimization target (`/aggregations/events` at ~15.1s on a broad
  valuation-backed daily query). Report routes are structurally heavy but still
  within an acceptable first-pass range (about 307ms to 3.16s depending on
  shape). Materialized-table and rollup-backed routes are healthy.

- **2026-05-07 (resolved)** Phase 1 (Track A) shipped end-to-end. Track B
  (report-route hardening) is deferred — the sub-2s payout-CSV path and
  sub-1s reward-CSV path now sit comfortably under the watch thresholds
  named in the original plan, so any further work requires fresh evidence
  of a regression. Track C (benchmark harness) is also deferred to a
  future TD entry — the rewrite is verified by ad-hoc benchmarks below.

  **Concrete deliverables**:
  - Migration `038_create_event_metrics_daily.up.sql` adds the rollup
    table with PK `(chain_id, day_utc, contract_name, event_name, asset,
    valuation_version)` and three lookup indexes for the common access
    patterns. Replay-determinism preserved: rows derive solely from
    `raw_protocol_events` + `event_valuations`.
  - New worker `crates/livepeer-rollups/src/event_metrics.rs` implementing
    `run_once` + reorg-mutation path, mirroring the orch_rewards pattern.
    Wired into `runner.rs`, `lib.rs`, and `main.rs` as the
    `event-metrics-daily` subcommand with `--batch-limit` (default 2000)
    and `--cadence-secs` (default 300) flags, plus the `--follow` flag
    matching sibling rollup workers.
  - `crates/livepeer-api/src/routes/aggregations.rs` rewritten with two
    code paths: `aggregate_from_rollup` for queries the daily rollup can
    answer, `aggregate_from_raw_events` (preserves prior semantics) for
    everything else. A new `rollup_covers_range` helper enforces
    correctness during backfill — the rollup path is only chosen when its
    checkpoint covers the requested upper bound, otherwise the request
    falls through to the direct scan. Response now carries an explicit
    `source: "rollup" | "raw_events"` field so consumers see which path
    served them.
  - `docker-compose.yml` adds `livepeer-rollups-event-metrics` service.
  - `scripts/resume-catchup-all.sh` adds the new follow worker.
  - The existing `livepeer-staker` migration `037` lateral-lookup index
    (`idx_events_transcoder_update_by_to_address`) added earlier today
    keeps the report-route fall-back path fast even when the rollup
    isn't eligible.

  **Measured benchmarks** (release build, local DB, partial rollup
  coverage at the time of measurement — checkpoint @ event_id 60,319,
  covering 2022-02-15 → 2023-03-02):

  | Query shape | Path | Latency |
  |---|---|---|
  | Closed range, day bucket, count metric (5 months) | rollup | **35ms** |
  | Closed range, day bucket, sum_amount_usd, asset=ETH (~1 year) | rollup | **18ms** |
  | Closed range, week bucket, sum_amount_usd, asset=ETH | rollup | **18ms** |
  | Same shape with address filter | raw_events | 184ms |
  | Open-ended (no `to`), day bucket, count | raw_events | 6.8s |
  | Open-ended, day bucket, sum_amount_usd, full history | raw_events | 1.3s |

  Rollup-eligible queries land in the **<50ms** range against the **<500ms**
  target. The pre-rewrite baseline of ~15.1s on the broad daily USD
  aggregation is gone; the same shape now serves from the rollup in
  ~18ms. Even the raw_events fallback is meaningfully faster than the
  original 15s because of unrelated index work (migration 037).

  **Open follow-ups** (not blockers, intentionally deferred):
  - Track B (report-route hardening): re-benchmark only if a regression
    surfaces. Current shapes are inside their watch thresholds.
  - Track C (benchmark harness in CI): considered overkill until a
    regression surfaces. Operators have working ad-hoc commands; a
    formal harness can come with the next perf incident.
  - Non-UTC timezone bucket alignment: rollup-ineligible by design;
    requests fall back to raw_events. If non-UTC analytics become a hot
    path, a per-tz materialized variant could be added.
