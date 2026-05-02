# TD-012: Daemon mode — keep the pipeline at chain head

**Status:** Plan only — no implementation yet.
**Severity:** Medium — v1 ships as one-shot CLIs that must be re-invoked to keep
moving forward; production use needs a long-running supervised daemon.
**Last touched:** 2026-04-29.

## Problem statement

Today the system has eight binaries that each do one batch of work and exit:

| Binary | Today's shape | Already long-running? |
|---|---|---|
| `livepeer-indexer` | `backfill --from-block X --to-block Y` then exit | No — batch-only |
| `livepeer-valuator` | `backfill-all` (seed → ETH on-chain → LPT on-chain) then exit | No — batch-only |
| `livepeer-staker` | `flow-backfill` then exit | No — batch-only |
| `livepeer-reorg-watcher` | has `--once` and a built-in poll loop with `--interval` | **Yes** (already a daemon shape) |
| `livepeer-finality-watcher` | has `--once` and a built-in poll loop with `--interval` | **Yes** (already a daemon shape) |
| `livepeer-api` | Axum HTTP server | **Yes** |
| `livepeer-seed-migrator` | one-shot SQLite → PG migration | N/A — bootstrap only |

Operating this against a moving chain currently means a wrapper script that
re-launches the indexer/valuator/staker every N seconds (see
`scripts/full-run-post-indexer.sh` for the catch-up version). That works for
backfill but is the wrong shape for steady-state operation:

1. **No graceful shutdown** — SIGTERM during a batch leaves partial state in
   `event_valuations` / `valuation_attempts` (they're committed per event, so
   no row corruption — but the pass restarts from scratch on next launch).
2. **No backpressure between stages** — indexer can race ahead of valuator;
   valuator can race ahead of staker; nothing coordinates a shared head depth.
3. **No shared rate limiter for RPC** — each launch creates a fresh
   `core::rpc::Provider` pool, so concurrent indexer + valuator + staker can
   exceed Chainstack's per-IP archive-call cap that we tuned for in TD-011.
4. **No metrics surface** — Prometheus endpoint per SPEC §17.2 doesn't exist;
   ops visibility today is grepping `logs/*.log` and `psql` ad-hoc queries.
5. **No alerting** — SPEC §13.5 calls for Telegram alerts on RpcDivergence,
   stuck checkpoints, finality lag — none are wired.

This document is the plan for fixing that. **It is a plan only — no code yet.**

## Goals

- **Steady-state operation**: pipeline tracks chain head with bounded lag
  (target: indexer ≤30s, valuator ≤5min, staker ≤1 round).
- **One supervisor**: single binary the operator runs (`systemctl start
  livepeer-daemon`) instead of seven `cron` entries.
- **Determinism preserved**: daemon output must be byte-equal to batch output
  on the same `rpc_call_cache` snapshot. The whole point of the call-hash
  cache is that the *mode of operation* doesn't bleed into the data.
- **Restartability**: a kill -9 mid-run leaves the system resumable from
  `indexer_checkpoints` + `valuation_attempts` skip filter + staker's
  `pending_*_refresh_cursor`.
- **Observability**: every loop iteration emits structured `tracing` events
  + Prometheus counters/gauges/histograms; alerts on the metrics that matter.

## Operating modes

This repo needs **three distinct operating modes** with different contracts.
Treating them as one mode would blur determinism, steady-state scheduling, and
historical catch-up in ways that are hard to reason about.

### 1. `bootstrap` — first run / historical catch-up

Use for an empty database or a partially-complete historical backfill.

Example:

```bash
livepeer bootstrap \
  --from-block 6072093 \
  --to-block 457212919 \
  --source-sqlite /seed/sqlite-4.0.db \
  --version v1_lpt_weth_twap_30min_x_chainlink_eth
```

Contract:

- Finite job, not a daemon.
- Resumable from checkpoints.
- Allowed to populate `rpc_call_cache` from live RPC on cache miss.
- Runs the full bounded pipeline: migrations/bootstrap checks → seed import →
  indexer backfill → finality promotion → valuator → staker → optional
  cross-check / replay validation.
- Safe to rerun idempotently.

This is the correct mode for the **first production backfill** and for
operator-driven catch-up after long downtime. It is **not** the daemon.

### 2. `replay` — deterministic rerun from cached inputs

Use for byte-deterministic replay from `rpc_call_cache` + seeded SQLite.

Example:

```bash
livepeer replay \
  --from-block 6072093 \
  --to-block 32000000 \
  --source-sqlite /seed/sqlite-4.0.db \
  --version v1_lpt_weth_twap_30min_x_chainlink_eth \
  --cache-only
```

Contract:

- Finite job, not a daemon.
- Must **not** use live RPC as a fallback.
- Any missing cached RPC call is a hard failure.
- Runs against a fresh DB or freshly-reset derived state.
- Emits or validates expected table-content hashes.

This is the mode used by determinism CI and by operators when validating that a
cached replay reproduces the original database exactly.

### 3. `follow` — steady-state near-head processing

Use only after the system has already caught up enough that it should keep pace
with the moving chain head.

Example:

```bash
livepeer-daemon follow \
  --max-start-lag-blocks 50000 \
  --version v1_lpt_weth_twap_30min_x_chainlink_eth
```

Contract:

- Infinite long-running service.
- Uses shared RPC budgets, coordinated shutdown, metrics, and alerts.
- Refuses to start when current lag exceeds a configured threshold.
- Optimized for bounded incremental work, not million-block historical backfill.

`follow` is the daemon mode covered by this plan. It should never be the
primary engine for first-time backfill or determinism replay.

## Non-goals

- **HA/multi-instance**: v1 daemon is single-instance. Multiple daemons against
  the same Postgres would race on `indexer_checkpoints` row updates. HA needs
  advisory locks (`pg_try_advisory_lock(...)`) — deferred to v2.
- **Live reorg deeper than the indexer's lookback window** (currently 256
  blocks). Multi-hour reorgs require operator intervention; daemon's job is
  to detect and stop, not auto-heal.
- **Catch-up parallelism beyond what's already in
  `full-run-parallel.sh`**. Backfill stays a separate code path; daemon mode
  is for incremental work after the initial backfill is caught up.
- **Using daemon mode as the first historical backfill path.** First run and
  deterministic replay stay batch-oriented (`bootstrap` / `replay`), even
  after daemon mode exists.

## Three-phase rollout

### Phase 1 — orchestration modes for bounded runs (~1-2 days)

**Goal: introduce explicit `bootstrap` and `replay` orchestration modes for
bounded runs, while keeping daemon work out of the first historical backfill
path.**

Why this first: the repo already has the right shape for batch work, and
deterministic replay is a core invariant. The first implementation step should
clarify the operator contract for finite runs before adding a scheduler for
steady-state mode.

#### New top-level CLI contract

Introduce a small orchestration binary (name TBD; examples below use
`livepeer`) with these subcommands:

```
livepeer bootstrap ...
livepeer replay ...
```

`bootstrap` responsibilities:

- Runs migrations and boot checks.
- Imports the seed if requested.
- Runs bounded indexer/finality/valuator/staker work over the requested range.
- May populate `rpc_call_cache` from live RPC on cache miss.
- Resumes from checkpoints by default.

`replay` responsibilities:

- Recreates derived state from a known input set.
- Requires `--cache-only` / `--fail-on-missing-cache`.
- Fails immediately on missing cached RPC inputs.
- Emits or verifies deterministic output hashes.

#### Existing binaries stay bounded

The current binaries remain the building blocks for historical work:

- `livepeer-indexer`: bounded block-range run
- `livepeer-finality-watcher`: bounded promotion pass
- `livepeer-valuator`: bounded valuation pass
- `livepeer-staker`: bounded stake refresh/backfill pass

The new orchestration CLI should call into shared library functions from these
crates rather than shelling out to subprocesses.

#### Determinism check for Phase 1

After Phase 1 lands, validate that `replay` against the same
`rpc_call_cache` + seed reproduces the same `raw_protocol_events`,
`event_valuations`, `token_prices_by_block`, and stake tables as the original
bounded run. Acceptance: `bash scripts/validate-vs-baseline.sh <baseline-dir>`
reports MATCH.

#### What Phase 1 deliberately does NOT do

- No long-running daemon yet.
- No per-binary `--follow` mode as the primary operational interface.
- No multi-process pseudo-daemon with separate RPC pools. That would worsen the
  pressure pattern that TD-011 is trying to stabilize.

### Phase 2 — single `livepeer-daemon` binary (~3–5 days)

**Goal: introduce `livepeer-daemon follow` for steady-state near-head
processing only, with shared RPC budgets, coordinated checkpoints, and graceful
shutdown.**

Why next: once bounded orchestration is explicit, the daemon can focus on the
single thing batch mode is bad at: near-head steady-state scheduling. This also
avoids entangling first-time backfill with the still-open TD-011 RPC ceiling.

#### New crate: `crates/livepeer-daemon/`

```
livepeer-daemon/
  Cargo.toml                    # depends on every other crate's lib
  src/
    main.rs                     # arg parsing, config load, supervisor::run
    supervisor.rs               # task topology + graceful shutdown
    config.rs                   # daemon.yaml schema
    tasks/
      indexer_task.rs           # wraps livepeer_indexer::backfill::drive_backfill
      valuator_task.rs          # wraps livepeer_valuator::{seed,onchain,multi_asset}
      staker_task.rs            # wraps livepeer_staker::flow::run_flow_backfill
      reorg_task.rs             # wraps livepeer_reorg_watcher::watcher::run_once
      finality_task.rs          # wraps livepeer_finality_watcher::watcher::run_once
```

Each existing crate must expose its current bounded-work logic as a
**library function** taking `(pg: &PgPool, provider: Arc<Provider>, ctx:
&IterCtx) -> Result<IterSummary>`. The current `main.rs` wrappers stay,
parsing CLI and calling that same library function for one bounded run.
The daemon will call those same library functions repeatedly under a scheduler.

The daemon wraps each library function inside its own
`tokio::spawn(async move { loop { ... } })` task.

The first daemon scope should be intentionally narrow:

- Required in daemon v1: indexer, finality watcher, valuator
- Defer or keep separate initially: reorg watcher, staker

Reasoning: keeping the initial daemon focused on ingestion + finalization +
valuation minimizes failure-surface while solving the highest-value
steady-state problem first.

#### Async task topology

```
                     ┌──────────────────────┐
                     │  Arc<Provider>       │  shared rate limiter
                     │  (rpc pool, N=24)    │  + cross-task semaphore
                     └────────┬─────────────┘
                              │
   ┌──────────┬───────────────┼────────────┬──────────────┐
   │          │               │            │              │
┌──┴──────┐ ┌─┴─────────┐ ┌───┴───────┐ ┌──┴──────────┐ ┌─┴───────────┐
│indexer  │ │reorg-wat. │ │valuator   │ │staker       │ │finality-wat.│
│ every   │ │ every 60s │ │ every 60s │ │ every 300s  │ │ every 60s   │
│  12s    │ │           │ │           │ │             │ │             │
└─────────┘ └───────────┘ └───────────┘ └─────────────┘ └─────────────┘
       │           │            │              │             │
       └───────────┴────────────┴──────────────┴─────────────┘
                              │
                     ┌────────┴─────────────┐
                     │ shutdown_signal      │  tokio::sync::Notify
                     │ (one for the whole   │  fired by Ctrl-C
                     │  daemon)             │  / SIGTERM
                     └──────────────────────┘
```

#### Shared RPC manager

- Single shared RPC manager constructed at boot and given to every task.
- One `tokio::sync::Semaphore` with N permits gates the **total** in-flight
  cross-checked RPC calls across all tasks. N defaults to 24 (well below
  TD-011's empirical 50 ceiling, leaves headroom for the API server's
  on-demand calls).
- Per-task soft caps via separate semaphores so one runaway task can't
  starve the others (`indexer ≤ 8`, `valuator ≤ 14`, `staker ≤ 4`,
  watchers ≤ 1 each).
- Client refresh / rotation policy (TD-011 mitigation) lives inside this
  manager, not in daemon task code. Tasks should depend on a stable handle and
  remain unaware of connection-pool refreshes.

#### Graceful shutdown

```rust
let shutdown = Arc::new(Notify::new());

// signal handler
tokio::spawn({
    let shutdown = shutdown.clone();
    async move {
        tokio::signal::ctrl_c().await.ok();
        shutdown.notify_waiters();
    }
});

// each task loop
loop {
    tokio::select! {
        _ = shutdown.notified() => break,
        _ = tokio::time::sleep(interval) => {}
    }
    let result = run_one_iteration(...).await;
    match result {
        Ok(_) => {},
        Err(e) if e.is_recoverable() => warn!(err=%e, "iteration failed; retrying next tick"),
        Err(e) => {
            error!(err=%e, "fatal iteration failure; daemon shutting down");
            shutdown.notify_waiters();
            break;
        }
    }
}
```

Each task must process **small resumable bounded units** and return an
`IterSummary`. The daemon should not hide long opaque loops inside worker
tasks; scheduling and cancellation belong at the supervisor layer.

Key invariants:
- An iteration **never spans a shutdown** — `run_one_iteration` runs to
  completion (it's bounded; e.g. one indexer chunk = ≤1000 blocks ≈ 5–15s).
- On shutdown, the supervisor `JoinHandle::await`s each task; SIGTERM
  grace period (systemd `TimeoutStopSec=60s`) is sized to fit the longest
  iteration plus a safety margin.
- Database commits already happen per chunk / per event, so an interrupted
  iteration leaves the cursor advanced for the work it actually finished.

#### Config schema (`/etc/livepeer/daemon.yaml`)

```yaml
chain_id: 42161
postgres_url: ${POSTGRES_URL}     # env interp
rpc:
  primary_url:   ${CHAINSTACK_PRIMARY_URL}
  secondary_url: ${CHAINSTACK_SECONDARY_URL}
  total_concurrency: 24
  per_call_timeout_seconds: 60
  hard_timeout_seconds: 90
  pool_refresh_interval_seconds: 600
  cross_check_required: true       # always on for chain reads (SPEC §7.6)
tasks:
  indexer:
    enabled: true
    interval_seconds: 12
    head_depth_blocks: 10
    per_chunk_blocks: 1000
    concurrency: 8
  valuator:
    enabled: true
    interval_seconds: 60
    lag_blocks: 1
    concurrency: 14
    passes: [seed, onchain_eth, onchain_lpt, multi_asset]
  staker:
    enabled: true
    interval_seconds: 300
    include_tentative: false
    concurrency: 4
  reorg_watcher:
    enabled: true
    interval_seconds: 60
    lookback_blocks: 256
  finality_watcher:
    enabled: true
    interval_seconds: 60
metrics:
  bind: "127.0.0.1:9090"           # Prometheus scrape target
alerting:
  telegram:                        # Phase 3 wiring
    enabled: false
    bot_token_env: TELEGRAM_BOT_TOKEN
    chat_id_env:   TELEGRAM_CHAT_ID
```

#### What still ships standalone

- `livepeer-api` — separate process, separate systemd unit. The daemon
  doesn't host HTTP; that's a different lifecycle (zero-downtime restarts
  via reverse proxy etc.).
- `livepeer-seed-migrator` — one-shot. Stays as-is.
- All the existing `--backfill` / `--from-block` CLIs — preserved for
  cold-start operations and replay-validation runs. Phase 2's library
  refactor doesn't remove them, just adds a new caller.

### Phase 3 — production hardening (~2–3 days on top of Phase 2)

#### 3a — Prometheus metrics (per SPEC §17.2)

Catalog (final names TBD; prefix `livepeer_`):

**Counters**
- `livepeer_iterations_total{task}` — successful iterations
- `livepeer_iteration_failures_total{task,error_kind}` — split by error
  classification (rpc_http, rpc_divergence, db, decoder, internal)
- `livepeer_events_indexed_total{contract}` — rows committed to
  `raw_protocol_events`
- `livepeer_events_valued_total{asset,version,status}` — by
  `event_valuations.status` (priced / failed_*)
- `livepeer_rpc_calls_total{provider,method,result}` — provider-level
- `livepeer_rpc_cache_hits_total{method}` — cache effectiveness
- `livepeer_rpc_divergence_total{method}` — TD-011-related; should stay 0

**Gauges**
- `livepeer_chain_head_block` — most recent `eth_blockNumber` we observed
- `livepeer_indexer_checkpoint_block` — latest committed indexer cursor
- `livepeer_indexer_lag_blocks` = head − checkpoint
- `livepeer_valuator_lag_blocks` — head − latest valued block per asset
- `livepeer_finality_pending_count{kind}` — rows still tentative/l1_posted
- `livepeer_rpc_pool_in_flight` — semaphore acquired count
- `livepeer_db_pool_in_flight`

**Histograms**
- `livepeer_iteration_duration_seconds{task}`
- `livepeer_rpc_call_duration_seconds{provider,method}` — from
  `core::rpc::Provider`
- `livepeer_indexer_chunk_size_blocks`

The metric set is **observability surface, not the alert surface** —
alerts are derived from these via Prometheus rules.

#### 3b — Alerting (Telegram, per SPEC §13.5)

Three ways an alert fires; all go to the same `core::alert::send_alert`
sink so we can test the wire without firing real alerts.

| Trigger | Detection | Severity |
|---|---|---|
| `RpcDivergence` written | `core::rpc::cross_check` emits, daemon hooks the error path | **page** |
| Indexer lag > 1000 blocks for >5min | Prometheus rule on `livepeer_indexer_lag_blocks` | warn |
| Valuator lag > 5000 blocks for >15min | Prometheus rule on `livepeer_valuator_lag_blocks` | warn |
| Iteration failure rate > 5% over 10min | rule on `iteration_failures_total / iterations_total` | warn |
| Pool refresh failed 3 consecutive times | counter on Provider's pool refresh task | warn |
| L2 sequencer down (per Chainlink uptime feed) | finality watcher detects, propagates | **page** |
| Daemon process restarted | systemd `OnFailure=` hook to `alert-bot.service` | info |

We use a **Prometheus Alertmanager → Telegram** topology rather than
embedding Telegram calls in the daemon — keeps the daemon free of
alerting state and lets us reuse alert rules across staging/prod.
The `livepeer-alert-bot` is a tiny separate binary in
`crates/livepeer-alert-bot/` (Phase 3 deliverable; not in Phase 2 scope).

#### 3c — RPC failover semantics

Today, `Provider` is constructed with a single primary URL; cross-check
adds a secondary. In daemon mode we want the primary to fail over to a
**third archive endpoint** if it stays unhealthy for >60s.

Failover rules:
- Health check = last-N-call success rate < 50% over a 30s window OR
  pool-refresh-failed counter ≥ 3.
- Failover swaps the URL inside `Arc<RwLock<reqwest::Client>>`; existing
  in-flight calls are not aborted.
- Cross-check still runs every chain read — failover doesn't make
  single-provider reads acceptable; it just changes which providers
  pair up for the cross-check.
- `liveinfraspe` (the secondary, non-archive) is never promoted to
  primary because it doesn't have archive depth.

Out of scope for Phase 3: automatic failback. Once we fail over, manual
ops re-points to primary. Reason: thrashing under flapping conditions
is worse than running on the secondary for a few hours.

#### 3d — Determinism replay CI

Add a CI job that runs nightly:

1. Snapshot prod's `rpc_call_cache` to a file.
2. Stand up an empty PG, restore that cache.
3. Run the daemon for 24 hours of simulated time (replaying chain head
   from a recorded sequence).
4. Diff `event_valuations`, `token_prices_by_block`,
   `delegator_pending_state` row hashes against the prod baseline.
5. Pass = byte-identical; fail = page on-call.

This is the operational cousin of `scripts/validate-vs-baseline.sh` and
is the contract that lets us refactor daemon internals without fearing
silent determinism drift.

#### 3e — Operator runbook outline

Lives at `docs/operator/runbook.md` (to be created). Sections:

- **Startup**: env vars, config validation, "is this catching up?" check
- **Healthy indicators**: lag gauges expected ranges, expected per-task
  loop cadence in logs
- **Common alerts and what to do**:
  - RpcDivergence → don't auto-resolve; inspect `rpc_divergence_failures`
  - Indexer lag growing → check Chainstack status page, then Provider logs
  - Valuator lag growing → check `valuation_attempts` failure mix
- **Planned restarts**: order matters (api → daemon → postgres maintenance)
- **Database maintenance**: `VACUUM` cadence; `valuation_attempts` and
  `rpc_call_cache` are the two hot tables (TD-011 noted attempts bloating
  to 5.69M rows)
- **Disaster recovery**: PITR target, how to drop derived tables and
  re-run from `rpc_call_cache` to get back to byte-identical state

## How this intersects with existing TDs

| TD | Intersection |
|---|---|
| **TD-005** (reorg watcher v1 limited) | Reorg watcher already loops; daemon hosts it but doesn't change its semantics. Full §9 compliance still tracked under TD-005. |
| **TD-008** (finality watcher precise vs heuristic) | Finality watcher already loops; daemon hosts it. Precise SequencerInbox tracking is independent. |
| **TD-009** (bulk SQL refactors) | **Hard prerequisite for Phase 1.** A 60s valuator loop doing per-event SQL won't keep up. Phase 1 should not start until the on-chain bulk refactor is shipped — otherwise the follow loop falls behind on day one. |
| **TD-010** (slow API endpoints) | Independent — API endpoints are read-only over the same tables. Daemon doesn't change query plans. Address TD-010 separately. |
| **TD-011** (LPT throughput collapse) | Phase 2's shared rate-limited pool is a forcing function for picking the right ceiling. The investigations under TD-011 (pool refresh, HTTP/1.1 fallback, third provider) feed directly into Phase 2's `Provider` design. **Phase 2 should not start until TD-011 has a stable production ceiling we can encode as a default.** |

## Open questions to answer before Phase 1

1. **Head-depth choice**: 10 blocks is ~2 minutes on Arbitrum. Is that the
   right floor? Reorg-watcher's lookback is 256 blocks. We can afford
   indexer at 10 because reorg-watcher will mark anything that vanishes.
   But: what's the longest reorg we've actually observed on Arbitrum One
   in the seed window? Probably near zero — Arbitrum's sequencer rarely
   produces L2 reorgs in practice. Confirm before encoding.
2. **Valuator interval cadence**: 60s makes lag bounded by ~50 events.
   Could be 30s if bulk pass is fast enough. Decide after TD-009 LPT bulk
   pass perf data.
3. **Staker interval cadence**: rounds tick ~daily, but `pendingStake`
   monotonically accrues per-block. Is 5min right or should staker only
   wake on `NewRound` events? Operator-tunable; default 300s is safe.
4. **Single daemon vs five units in Phase 1**: Phase 2's plan is to
   collapse them. Some operators prefer one-unit-per-task. Keep both
   binaries shipped (per-task and `livepeer-daemon`) so it's an operator
   choice. Documenting this here so we don't accidentally remove the
   per-task `--follow` shape when Phase 2 lands.
5. **API ↔ daemon coordination**: should the API server expose
   `/operational/daemon-health` that proxies daemon's metrics endpoint?
   Today `/backfills/status` exists for this purpose (TD-010 item 4 —
   it's slow because of `COUNT(*)`). Consider rebuilding it from
   daemon's gauge values once Phase 3 metrics ship.

## Acceptance for closing TD-012

- Phase 1 merged and running in staging for ≥7 days without manual
  intervention.
- Phase 2 daemon binary running in prod with the per-task fallback
  binaries available but unused.
- Phase 3 metrics + alerts catch a real incident before logs do (we'll
  know it's working when an alert beats us to noticing a problem).

## Schedule (rough)

This is plan only — no commitment to dates. Sketch:

- Phase 1 — pick up after TD-009 LPT-pass bulk refactor lands (TD-009
  blocks Phase 1 viability).
- Phase 2 — pick up after TD-011 has a stable provider ceiling.
- Phase 3 — incremental on top of Phase 2; each subsection (3a–3e) is a
  separable PR.
