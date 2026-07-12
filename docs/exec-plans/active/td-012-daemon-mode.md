# TD-012: Daemon mode — keep the pipeline at chain head

**Status:** In progress — Phase 1 done, Phase 2 shipped (all six loops
supervised in-process with restart-with-backoff), Phase 3a metrics + `/health`
shipped; alerting (3b) still underway.
**Severity:** Medium — v1 ships as one-shot CLIs that must be re-invoked to keep
moving forward; production use needs a long-running supervised daemon.
**Last touched:** 2026-07-12.

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

This document started as the plan for fixing that. It is now also the running
execution record for the landed `orchestrator` and `daemon` slices.

## Current architectural direction

As of 2026-05-04, the intended runtime split is now explicit:

- `bootstrap` for empty-DB / large historical catch-up
- `replay` for deterministic rebuild from cached inputs only
- `follow` for steady-state near-head operation

The full target architecture and migration rationale are captured in
[../../design-docs/continuous-catchup-architecture.md](../../design-docs/continuous-catchup-architecture.md).
This plan is the execution artifact for landing that runtime shape.

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
- Strict by default: requires an explicit `--to-block` and fails on missing
  cached RPC inputs instead of falling back to live reads.
- Optional escape hatch: `--allow-live-rpc` for debugging / backfilling cache
  gaps, but that mode is not the determinism contract.
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

#### Concrete extraction targets from the current repo

Current `main.rs` files show the right reuse points already exist, but they are
trapped behind CLI parsing. The first code step is to expose them as crate
library entrypoints.

| Crate | Current bounded function(s) to wrap | Target library module |
|---|---|---|
| `livepeer-indexer` | `backfill::drive_backfill`, `backfill::resume_from` | `crates/livepeer-indexer/src/lib.rs`, `runner.rs` |
| `livepeer-finality-watcher` | `run_iteration` | `crates/livepeer-finality-watcher/src/lib.rs`, `runner.rs` |
| `livepeer-reorg-watcher` | `run_iteration`, `pick_cadence` (scheduler-only helper) | `crates/livepeer-reorg-watcher/src/lib.rs`, `runner.rs` |
| `livepeer-valuator` | `seed::run_seed_pass`, `onchain::run_onchain_pass_eth`, `onchain::run_onchain_pass_lpt`, `multi_asset::run_multi_asset_pass` | `crates/livepeer-valuator/src/lib.rs`, `runner.rs` |
| `livepeer-staker` | `flow::run_flow_backfill`, `pending::refresh_pending` | `crates/livepeer-staker/src/lib.rs`, `runner.rs` |

The CLIs should become thin wrappers:

- parse args
- build config / DB / provider handles
- call the library runner
- log the returned summary

#### Required library extraction boundary

Each batch binary needs a bounded library entrypoint so both the bounded
orchestrator and the future daemon call the same logic:

```rust
pub async fn run_once(ctx: &IterCtx) -> Result<IterSummary>;
```

This is the load-bearing refactor for the whole migration. If the daemon and
the batch binaries do not share the same bounded worker entrypoints, the
determinism story gets weaker and the runtime logic will drift.

#### Proposed shared types

These should live in a small reusable orchestration-facing module, ideally
under `crates/core` once the shape stabilizes.

```rust
pub struct IterCtx<'a> {
    pub pg: &'a PgPool,
    pub cfg: &'a Config,
    pub provider: Option<&'a Provider>,
    pub include_tentative: bool,
    pub valuation_version: Option<&'a str>,
    pub from_block: Option<u64>,
    pub to_block: Option<u64>,
    pub checkpoint_suffix: Option<&'a str>,
    pub cache_only: bool,
}

pub struct IterSummary {
    pub task: &'static str,
    pub units_seen: u64,
    pub units_written: u64,
    pub checkpoint_before: Option<u64>,
    pub checkpoint_after: Option<u64>,
    pub lag_before: Option<u64>,
    pub lag_after: Option<u64>,
}
```

The exact fields can evolve, but the important property is:

- a bounded worker reports enough structured output for both the batch
  orchestrator and the future daemon supervisor
- the caller does not need to scrape human log lines to know what happened

#### Determinism check for Phase 1

After Phase 1 lands, validate that strict `replay` against the same
`rpc_call_cache` + seed reproduces the same `raw_protocol_events`,
`event_valuations`, `token_prices_by_block`, and stake tables as the original
bounded run. Acceptance: `bash scripts/validate-vs-baseline.sh <baseline-dir>`
reports MATCH.

#### First PR slice for Phase 1

The first implementation PR should stay narrow:

1. Add `lib.rs` to:
   - `livepeer-indexer`
   - `livepeer-finality-watcher`
   - `livepeer-valuator`
   - `livepeer-staker`
2. Move existing callable bounded logic behind exported runner functions.
3. Keep existing CLI behavior unchanged by making `main.rs` call the new
   runners.
4. Add a new orchestration binary crate:
   - `crates/livepeer-orchestrator`
5. Implement only:
   - `livepeer-orchestrator bootstrap`
   - `livepeer-orchestrator replay`
6. Do **not** add daemon looping in this PR.

Acceptance for the first PR slice:

- existing binaries still behave the same from the command line
- orchestrator can call the same workers without shelling out
- `replay` can be wired to the same deterministic validation flow already used
  by `scripts/snapshot-baseline.sh` / `scripts/validate-vs-baseline.sh`

#### Suggested crate layout for Phase 1

```
crates/
  livepeer-orchestrator/
    Cargo.toml
    src/
      main.rs
      bootstrap.rs
      replay.rs
      reset.rs          # optional derived-table reset helper
      summary.rs
  livepeer-indexer/
    src/
      lib.rs
      main.rs
      backfill.rs
      runner.rs
  livepeer-finality-watcher/
    src/
      lib.rs
      main.rs
      runner.rs
  livepeer-valuator/
    src/
      lib.rs
      main.rs
      runner.rs
  livepeer-staker/
    src/
      lib.rs
      main.rs
      runner.rs
```

The orchestrator should link directly against these crates, not spawn shell
commands. Shelling out would preserve today's operational shape and lose the
structured summary / shared-resource story we need for Phase 2.

#### What Phase 1 deliberately does NOT do

- No long-running daemon yet.
- No per-binary `--follow` mode as the primary operational interface.
- No multi-process pseudo-daemon with separate RPC pools. That would worsen the
  pressure pattern that TD-011 is trying to stabilize.
- No auto-switch from historical catch-up straight into daemon mode until lag
  thresholds and shared RPC ceilings are codified.

### Phase 2 — single `livepeer-daemon` binary (~3–5 days)

**Goal: introduce `livepeer-daemon follow` for steady-state near-head
processing only, with shared RPC budgets, coordinated checkpoints, and graceful
shutdown.**

Why next: once bounded orchestration is explicit, the daemon can focus on the
single thing batch mode is bad at: near-head steady-state scheduling. This also
avoids entangling first-time backfill with the still-open TD-011 RPC ceiling.

#### Follow-mode startup gate

`livepeer-daemon follow` should refuse to start when lag exceeds a configured
threshold (for example `--max-start-lag-blocks 50_000`). Above that threshold,
the operator must run `bootstrap` instead. This is a core part of the
architecture, not a convenience flag: near-head scheduling and million-block
backfill are intentionally different modes.

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

The first daemon scope was intentionally narrow:

- Required in daemon v1: indexer, finality watcher, valuator
- Defer or keep separate initially: reorg watcher, staker

Reasoning: keeping the initial daemon focused on ingestion + finalization +
valuation minimizes failure-surface while solving the highest-value
steady-state problem first.

> **Superseded (as shipped).** The narrow cut is history. The daemon now
> supervises **all six loops in-process** — `SUPERVISED_TASKS = [indexer,
> finality, reorg, valuator, staker, matview]` — each under its own
> `supervise()` restart-with-backoff wrapper. Folding reorg and staker in
> became safe once per-task supervision meant a flapping loop restarts itself
> instead of taking the daemon down, so the "keep them standalone" hedge is no
> longer needed.

#### First daemon PR slice

The first Phase 2 PR should also stay narrow:

1. Add `crates/livepeer-daemon`
2. Implement only:
   - startup lag inspection
   - `follow` mode
   - shared shutdown signal
   - shared RPC manager wrapper
   - process-wide RPC concurrency ceiling in `core::rpc::Provider`
   - task loops for:
     - indexer
     - finality watcher
     - valuator
3. Keep:
   - reorg watcher standalone for the first daemon cut, or run it on a very
     conservative cadence with no mutation-policy changes
   - staker standalone until the follow loop is proven stable

> **Done / superseded.** Both `reorg` and `staker` now run as first-class
> supervised loops inside the daemon (reorg every 60s, staker every 300s) — see
> the *Async task topology* and *Supervision + graceful shutdown* sections. The
> standalone binaries are still shipped for cold-start/decomposed operation but
> are no longer the steady-state path.

Acceptance for the first daemon PR slice:

- daemon refuses to start when lag is above threshold
- daemon keeps near-head data moving without wrapper scripts
- stopping the daemon cleanly leaves resumable checkpoints
- bounded worker output remains byte-identical to batch-mode output on the same
  cached inputs

#### Async task topology

> **As shipped:** six loops, each wrapped in its own `supervise()` combinator
> (see *Graceful shutdown* below). The list, order, and cadences are the
> `SUPERVISED_TASKS` constant in `crates/livepeer-daemon/src/supervisor.rs`.

```
                     ┌──────────────────────┐
                     │  Arc<Provider>       │  shared rate limiter
                     │  (rpc pool, N=24)    │  + cross-task semaphore
                     └────────┬─────────────┘
                              │
   ┌──────────┬───────────────┼────────────┬──────────────┬────────────┐
   │          │               │            │              │            │
┌──┴──────┐ ┌─┴─────────┐ ┌───┴───────┐ ┌──┴──────────┐ ┌─┴───────────┐ ┌┴─────────┐
│indexer  │ │reorg      │ │valuator   │ │staker       │ │finality     │ │matview   │
│ every   │ │ every 60s │ │ every 60s │ │ every 300s  │ │ every 60s   │ │ every 30s│
│  12s    │ │           │ │           │ │             │ │             │ │ (default)│
└─────────┘ └───────────┘ └───────────┘ └─────────────┘ └─────────────┘ └──────────┘
   each loop is wrapped in supervise(): catch Err+panic → back off → respawn
       │           │            │              │             │           │
       └───────────┴────────────┴──────────────┴─────────────┴───────────┘
                              │
                     ┌────────┴─────────────┐
                     │ shutdown             │  watch::Receiver<bool>
                     │ (latched; one for    │  set by SIGINT (ctrl_c)
                     │  the whole daemon)   │  *and* SIGTERM
                     └──────────────────────┘
```

The `matview` loop (6th task) refreshes the `orchestrator_profile` /
`broadcaster_profile` materialized views `CONCURRENTLY` every
`DAEMON_MATVIEW_REFRESH_SECS` (default 30s). It is a full supervised loop like
the others but is intentionally **excluded from `/health` gating** — a stale
profile view is cosmetic and must not restart the whole container.

A latched `watch::channel(false)` (not a `tokio::sync::Notify`) is used so a
signal delivered during a supervisor's backoff/respawn gap is not lost — a
`Notify` only wakes current waiters, so a signal arriving while a loop is
sleeping between restarts could be missed. Both SIGINT (`ctrl_c()`) and SIGTERM
(`signal(SignalKind::terminate())`, the signal `docker stop` / compose sends)
set the latch.

#### Shared RPC manager

- Single shared RPC manager constructed at boot and given to every task.
- One `tokio::sync::Semaphore` with N permits gates the **total** in-flight
  cross-checked RPC calls across all tasks. N defaults to 24 (well below
  TD-011's empirical 50 ceiling, leaves headroom for the API server's
  on-demand calls).
- Per-task soft caps now ship via task-scoped semaphores in `core::rpc` so
  one runaway task can't starve the others while still sharing the same
  process-wide ceiling (`indexer ≤ 8`, `finality ≤ 2`, `reorg ≤ 2`,
  `valuator ≤ 16`, `staker ≤ 6`; sum intentionally exceeds the global cap
  so idle tasks do not reserve capacity).
- Client refresh / rotation policy (TD-011 mitigation) lives inside this
  manager, not in daemon task code. Tasks should depend on a stable handle and
  remain unaware of connection-pool refreshes.

#### Supervision + graceful shutdown

> **As shipped** — the original design (below the fold in git history) had each
> task call `shutdown.notify_waiters(); break;` on its *first* fatal error,
> which tore down the whole daemon whenever any one loop hit an unrecoverable
> error. That was replaced by a per-task `supervise()` combinator: a broken
> loop restarts itself with backoff instead of killing its siblings, and only a
> *supervisor* panic (or a failed `/health` probe → container restart) is fatal.

Each loop is spawned inside a `supervise()` combinator that owns the
restart-with-backoff policy. `supervise()`:

```rust
// One supervisor per loop. Never breaks except on shutdown.
async fn supervise(task, metrics, mut shutdown: watch::Receiver<bool>, policy, mut make_fut) {
    let mut consecutive = 0u32;
    loop {
        if *shutdown.borrow() { break; }

        let hb_before = metrics.heartbeat(task);
        // Run the loop on its own task so a panic is *caught*, not fatal.
        let outcome = tokio::spawn(make_fut()).await;
        if *shutdown.borrow() { break; }   // clean stop, not a restart

        let reason = match outcome {
            Ok(Ok(()))   => "error",   // a loop only returns Ok on shutdown; anomalous → restart
            Ok(Err(_))   => "error",
            Err(j) if j.is_cancelled() => break,  // runtime tearing down
            Err(_)       => "panic",
        };
        metrics.record_restart(task, reason);        // livepeer_task_restarts_total{task,reason}

        // Progress-based reset: heartbeat advanced ⇒ the loop made progress.
        if metrics.heartbeat(task) > hb_before { consecutive = 0; }
        else { consecutive += 1; }

        // Escalate after repeated deaths-without-progress → /health turns it into a restart.
        metrics.set_task_up(task, consecutive <= policy.max_consecutive);  // livepeer_task_up{task}

        // Exponential backoff (1s base × 2^(n-1), capped at 60s), interruptible by shutdown.
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = sleep(backoff) => {}
        }
    }
}
```

The shutdown latch and signal handler:

```rust
// Latched: a signal during a backoff/respawn gap is not lost (unlike Notify).
let (shutdown_tx, shutdown_rx) = watch::channel(false);
tokio::spawn(async move {
    let mut term = signal(SignalKind::terminate())?;      // SIGTERM (docker stop / compose)
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},                // SIGINT
        _ = term.recv() => {},
    }
    let _ = shutdown_tx.send(true);
});

// Supervisors only return on shutdown. A supervisor *panic* leaves a loop
// unsupervised → treat as fatal so the process exits and Docker restarts it.
while let Some(res) = set.join_next().await {
    if let Err(j) = res {
        if j.is_panic() { return Err(anyhow!("supervisor task panicked: {j}")); }
    }
}
```

Each inner loop still processes **small resumable bounded units** and beats its
liveness heartbeat (`livepeer_task_last_success_timestamp{task}`) after each
successful iteration; scheduling, restart, and cancellation belong at the
supervisor layer.

Key invariants:
- **A single broken loop never kills the daemon.** `supervise()` catches the
  loop's `Err` *and* any panic, backs off, and respawns just that loop. Only a
  supervisor-level panic is fatal (it exits the process → Docker restarts the
  container), and a persistently-wedged/escalated loop is caught by the
  `/health` probe (below), which is what turns a stuck task into a restart.
- An iteration **never spans a shutdown** — each loop `select!`s the shutdown
  latch against its cadence sleep and returns `Ok(())` when the latch is set;
  bounded iterations (e.g. one indexer chunk = ≤1000 blocks) run to completion.
- On shutdown, the supervisor `JoinSet::join_next()`s each supervisor to
  completion; the Docker `stop_grace_period` is sized to fit the longest
  iteration plus a safety margin.
- Database commits already happen per chunk / per event, so an interrupted
  iteration leaves the cursor advanced for the work it actually finished.

#### Config schema — proposed `/etc/livepeer/daemon.yaml` vs what shipped

> **What shipped is NOT a `daemon.yaml`.** There is no dedicated daemon config
> file. The daemon reuses the same config surface as every other binary — the
> static + env YAML pair (`--static-config`/`STATIC_CONFIG`, default
> `config/arbitrum.yaml`; `--env-config`/`ENV_CONFIG`, default
> `config/env/dev.yaml`) — plus a handful of CLI flags / env vars on
> `livepeer-daemon follow`:
>
> - `--max-start-lag-blocks` (default `50_000`) — startup lag gate
> - `--version` — valuation version (defaults to the static config's
>   `pricing.default_valuation_version`)
> - `--include-tentative` (default `false`)
> - `--matview-refresh-secs` / `DAEMON_MATVIEW_REFRESH_SECS` (default `30`)
> - `--metrics-bind` / `DAEMON_METRICS_BIND` (default **`0.0.0.0:9107`**, not
>   `127.0.0.1:9090`) — serves both `/metrics` and `/health`
>
> Per-task cadences, head depth, per-chunk size, and RPC caps are compile-time
> constants in `supervisor.rs` / `core::rpc`, not YAML keys. The block below is
> the **original proposal**, kept for design context — it is not the shipped
> schema.

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
  matview:                         # SHIPPED as the 6th supervised loop
    enabled: true
    interval_seconds: 30           # DAEMON_MATVIEW_REFRESH_SECS
    views: [orchestrator_profile, broadcaster_profile]
    health_gated: false            # stale profile view must not restart the daemon
metrics:
  bind: "127.0.0.1:9090"           # SHIPPED default is 0.0.0.0:9107 (DAEMON_METRICS_BIND)
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

> **As shipped.** The early catalog below the fold used per-worker metric names
> (`livepeer_indexer_lag_blocks`, …) and richer labels than landed. The shipped
> daemon (`crates/livepeer-daemon/src/metrics.rs`) instead uses a **generic
> `{task}` label** across all per-loop metrics, so one metric covers all six
> supervised loops. The reconciled catalog is below; everything is prefixed
> `livepeer_` and served on `/metrics` (`DAEMON_METRICS_BIND`, default
> `0.0.0.0:9107`).

**Per-task loop metrics (label: `{task}` ∈ indexer|finality|reorg|valuator|staker|matview)**
- `livepeer_iterations_total{task}` — successful iterations (counter)
- `livepeer_iteration_failures_total{task,error_kind}` — `error_kind` ∈
  `{rpc, db, internal}` (from `classify_error`; counter)
- `livepeer_iteration_duration_seconds{task}` — histogram
- `livepeer_task_last_success_timestamp{task}` — **liveness heartbeat**, unix
  seconds of the last successful iteration; advances every healthy cadence,
  stalls when a loop is wedged/erroring. Read by `supervise()` (progress-based
  backoff reset) and by `/health` (gauge)
- `livepeer_task_restarts_total{task,reason}` — `reason` ∈ `{error, panic}`;
  incremented by `supervise()` on each restart (counter)
- `livepeer_task_up{task}` — 1 healthy, 0 after a loop exceeds its restart
  budget (escalated); fed into the `/health` decision (gauge)
- `livepeer_task_lag_blocks{task}` — per-task lag in blocks (gauge; today only
  `indexer` is populated)
- `livepeer_task_checkpoint_block{task}` — latest checkpoint-like block per task
  (gauge; today only `indexer`)
- `livepeer_task_rpc_limit{task}` / `livepeer_task_rpc_in_flight{task}` —
  per-task soft RPC concurrency cap and current in-flight permits (gauges)

**Ingestion / valuation / reorg counters**
- `livepeer_events_indexed_total{contract}` — rows committed to
  `raw_protocol_events`
- `livepeer_decode_failures_total{contract}` — decode failures written
- `livepeer_events_valued_total{status}` — valuation outcomes by status;
  `status` ∈ `{priced, failed_missing_oracle, failed_sequencer_outage,
  failed_missing_pool, failed_other}` (note: **only a `status` label** — the
  earlier `{asset,version,status}` shape was not shipped)
- `livepeer_reorgs_detected_total{severity}` — reorg divergences detected

**Matview refresh metrics (label: `{view}`)**
- `livepeer_matview_refresh_total{view,result}` — refresh attempts; `result` ∈
  `{success, error}` (counter)
- `livepeer_matview_refresh_seconds{view}` — wall-clock seconds of the most
  recent refresh (gauge, last-sample)

**Process-wide gauge**
- `livepeer_chain_head_block` — most recent `eth_blockNumber` observed

Provider-level metrics (`rpc_calls_total`, `rpc_call_duration_seconds`,
divergence counters, etc.) are **not** owned by `metrics.rs`; `/metrics` also
folds in `livepeer_core::rpc::metrics::gather()` and
`livepeer_staker::metrics::gather()` at scrape time.

The metric set is **observability surface, not the alert surface** —
alerts are derived from these via Prometheus rules.

#### 3a′ — `/health` staleness contract (shipped)

> This resolved open question 5 below: `/health` is served by the daemon itself
> (not proxied through the API) on `DAEMON_METRICS_BIND` and is the Docker
> healthcheck target.

`/health` reads the in-process heartbeat/up gauges (no DB query) and returns:

- **`200 OK`** when every *gated* task is fresh and up.
- **`503 Service Unavailable`** when any gated task's heartbeat age exceeds its
  threshold **or** its `livepeer_task_up == 0` (escalated). A 503 drives a
  whole-container restart, turning a wedged/permanently-broken loop into a
  restart instead of a silent partial stall.

Per-task staleness thresholds (`HEALTH_THRESHOLDS` in `http.rs`), roughly
`k × cadence` so fast loops surface a stall in minutes while the slow staker
keeps margin:

| task | threshold |
|---|---|
| indexer | 300s |
| finality | 300s |
| reorg | 300s |
| valuator | 300s |
| staker | 900s |
| **matview** | **excluded — not health-gated** |

- **Startup grace:** `HEALTH_START_GRACE_SECS = 120` — `/health` always
  reports OK for the first 120s after boot so a slow first iteration (or the
  Docker `start_period`) doesn't trigger a restart loop before the loops have
  run once.
- **matview is intentionally excluded** from gating — a stale profile matview
  is cosmetic and must not restart the daemon. It is still a fully supervised
  loop (restarts/backoff, `task_up`, matview metrics) — it just doesn't feed
  the health decision.

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

Automatic failover is now explicitly **deferred** until we have a real
second archive provider.

What ships today:
- one primary archive provider
- the existing cross-check path where configured
- a process-wide RPC concurrency ceiling
- provider-level metrics and distress visibility

What remains for a later Phase 3 slice:
- operator-approved alternate archive URL / env contract
- unhealthy-provider promotion policy
- manual-vs-automatic failback decision

Reason: without a real archive backup, "failover" would just add state
machine complexity without improving availability. The current operator
contract is manual: if the archive provider degrades badly, acquire or
switch to a replacement provider and restart the affected process.

#### 3d — Determinism replay CI

This has now landed for the orchestrated replay path.

What ships today:
1. Committed multi-case fixtures under `tests/fixtures/<case>/`
2. Strict replay via `livepeer-orchestrator replay`
3. `scripts/run-determinism-replay.sh` loads fixture cache + replay
   checkpoints, runs replay on a clean DB, and diffs stable table hashes
4. `.github/workflows/determinism.yml` runs that gate in CI

Current scope:
- validates strict offline replay semantics
- validates indexer/finality/valuator/staker orchestration against
  committed fixture windows
- does **not** yet simulate a long-running daemon follow session over a
  recorded head stream

That future "daemonized determinism soak" can still be added later, but
it is no longer a blocker for calling replay determinism CI real.

#### 3e — Operator runbook outline

Lives at `docs/RUNBOOK.md`. Sections:

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

   > **Partially resolved.** The daemon now serves its own `/health` and
   > `/metrics` directly on `DAEMON_METRICS_BIND` (default `0.0.0.0:9107`); the
   > `/health` staleness contract is specified in §3a′ above and is the Docker
   > healthcheck target. Whether the API server also proxies it as
   > `/operational/daemon-health` is still open.

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
