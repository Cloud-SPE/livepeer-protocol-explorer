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

## Three-phase rollout

### Phase 1 — minimum viable `--follow` mode (~1 day)

**Goal: each existing binary grows a `--follow` flag that loops internally
instead of exiting. No new binaries, no shared state between them. Operator
runs each as a separate systemd service.**

Why this first: it's the smallest diff that gets us off cron, preserves the
existing CLIs unchanged for backfill use, and reuses the loop shape that
`reorg-watcher` and `finality-watcher` already ship with.

#### Per-binary spec

**`livepeer-indexer`**
```
livepeer-indexer follow [--interval 12s] [--head-depth 10]
```
- Loops: read `indexer_checkpoints`, fetch `eth_blockNumber`, advance up to
  `head − head_depth` blocks, commit checkpoint, sleep `interval`.
- `head_depth=10` (~2min on Arbitrum) keeps indexer behind the reorg horizon
  so we don't index blocks that vanish in the next minute. Reorg-watcher
  handles deeper reorgs.
- Reuses existing `backfill::drive_backfill(... from, to ...)` per iteration.
  Internal range chunking (1000-block windows) is unchanged.
- On RPC divergence: emit metric, log, **stop** (don't auto-skip — TD-011
  posture).

**`livepeer-valuator`**
```
livepeer-valuator follow [--interval 60s] [--lag-blocks 1]
```
- Loops: pick up unvalued events from `raw_protocol_events` whose
  `block_number ≤ indexer_checkpoint − lag_blocks`, run seed → ETH → LPT
  passes (each is already idempotent per `valuation_attempts` skip filter).
- `lag_blocks=1` is the soft contract with the indexer: never try to value
  the very latest indexed block, in case the indexer is mid-commit.
- Each pass uses the bulk shape from TD-009 (when shipped) so a 60s loop
  doing ~50 events worth of work stays under 1s of compute.
- On HTTP wedge symptoms (TD-011): single retry, then back off and emit
  alert; don't auto-recreate the provider pool inside the loop.

**`livepeer-staker`**
```
livepeer-staker follow [--interval 300s] [--include-tentative=false]
```
- Loops: run `flow::run_flow_backfill` (already idempotent — uses
  `pending_stake_refresh_cursor` + dedupe). Runs less often because
  per-round refresh is the natural cadence (rounds are ~1 day on Livepeer
  Arbitrum).
- Flag default `--include-tentative=false` matches steady-state safety; flip
  to true only during live-edge debugging.

**`livepeer-reorg-watcher`**, **`livepeer-finality-watcher`**: already
follow-shape; no changes needed for Phase 1.

#### systemd unit examples (operator-facing)

Drop these as templates; we don't ship distro packaging in Phase 1. Living
location: `docs/operator/systemd-units/` — to be created when Phase 1 lands.

```ini
# livepeer-indexer-follow.service
[Unit]
Description=Livepeer indexer (follow mode)
After=network.target postgresql.service

[Service]
Type=simple
User=livepeer
EnvironmentFile=/etc/livepeer/daemon.env
ExecStart=/usr/local/bin/livepeer-indexer follow --interval 12s --head-depth 10
Restart=on-failure
RestartSec=10s
# Send SIGTERM, give 30s grace before SIGKILL — indexer commits per chunk
# so 30s is enough to finish the current 1000-block window.
KillSignal=SIGTERM
TimeoutStopSec=30s

[Install]
WantedBy=multi-user.target
```

Five units total: indexer, valuator, staker, reorg-watcher, finality-watcher.
The API is already a long-running process and ships independently.

#### Determinism check for Phase 1

After Phase 1 lands, validate that 24h of daemon-mode operation produces the
same `event_valuations` / `token_prices_by_block` rows as a 24h batch
re-run from the same indexer checkpoint range, on the same
`rpc_call_cache`. Acceptance: `bash scripts/validate-vs-baseline.sh
<daemon-baseline>` reports MATCH.

#### What Phase 1 deliberately does NOT do

- No shared RPC pool between processes — each binary keeps its own
  `core::rpc::Provider`. Means total in-flight RPC = sum of per-process
  budgets. We pick conservative per-process concurrency (`indexer=8`,
  `valuator=14`, `staker=4`) so the sum stays under TD-011's empirical
  ceiling of ~50.
- No cross-process backpressure — valuator can lag, staker can lag,
  alerting catches it. No need for IPC plumbing yet.
- No new metrics surface beyond what `tracing` already emits as JSON to
  stdout. Operator parses logs (or pipes to Loki/Vector) until Phase 3.

### Phase 2 — single `livepeer-daemon` binary (~3–5 days)

**Goal: collapse the five follow-loops into one process so we can share the
RPC pool, coordinate checkpoints, and make graceful shutdown trivial.**

Why next: Phase 1 leaves five independent processes each holding 8–16 TCP
connections to Chainstack. That's 40–80 sockets total against a per-IP
archive limit we know is around 50 (TD-011). Sharing one provider pool
lets us right-size at the daemon level.

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

Each existing crate must expose its current per-iteration logic as a
**library function** taking `(pg: &PgPool, provider: Arc<Provider>, ctx:
&IterCtx) -> Result<IterSummary>`. The current `main.rs` wrappers stay,
parsing CLI and calling that same library function once per `--follow`
loop iteration — so the binaries continue to work standalone.

The daemon wraps each library function inside its own
`tokio::spawn(async move { loop { ... } })` task.

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

#### Shared RPC pool

- Single `Arc<Provider>` constructed at boot, given to every task.
- One `tokio::sync::Semaphore` with N permits gates the **total** in-flight
  cross-checked RPC calls across all tasks. N defaults to 24 (well below
  TD-011's empirical 50 ceiling, leaves headroom for the API server's
  on-demand calls).
- Per-task soft caps via separate semaphores so one runaway task can't
  starve the others (`indexer ≤ 8`, `valuator ≤ 14`, `staker ≤ 4`,
  watchers ≤ 1 each).
- Pool refresh (TD-011 next-experiment list, item 1): keep the existing
  `Arc<RwLock<reqwest::Client>>` background refresh, hidden inside
  `Provider`. Tasks see it as a stable handle.

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
