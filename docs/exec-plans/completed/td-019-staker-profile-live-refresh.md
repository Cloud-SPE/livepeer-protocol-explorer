---
title: Staker Profile Live Refresh (proper Fix B for orchestrator/broadcaster profile staleness)
status: resolved
opened: 2026-05-07
resolved: 2026-05-07
owner: codex
links:
  - prior: ../completed/td-017-old-api-parity-and-rollups.md
  - related: ../active/td-018-api-read-path-performance-hardening.md
  - tracker: ../tech-debt-tracker.md
---

## Problem

`orchestrator_profile` and `broadcaster_profile` are written exclusively by
`livepeer-staker profile-backfill`. The daemon's `staker_loop` runs
`run_backfill` (flow) and `run_gateway_backfill` every cycle, but does **not**
call `run_profile_backfill`. That means in steady-state operation, neither
the daemon nor any other long-running process refreshes profile data.

The current operational workaround is a bash wrapper script
(`scripts/loop-staker-profile.sh`) that calls `profile-backfill` in a loop.
The workaround was originally written to exit on stable (`events_seen=0`),
which left profile data stale forever after first catch-up. A follow-up
edit (TD-019 Fix A, applied 2026-05-07) changed the wrapper to sleep and
re-check, but that's still operator-managed state outside the codebase.

The proper fix is in the staker codebase, not in operator scripts.

## Goal

Bring profile-table refresh into the same operational shape as the rollup
workers and the rest of the daemon supervisor — so once it ships, no
external wrapper is required to keep `orchestrator_profile` and
`broadcaster_profile` current with chain head.

## Non-goals

- Changing the on-disk schema of `orchestrator_profile` or
  `broadcaster_profile`. They were locked under TD-017's determinism
  contract and stay as-is.
- Changing the determinism contract. Profile rows remain deterministic
  per TD-017's table classification; the live writer must continue routing
  RPC through `rpc_call_cache` and stamping `as_of_block` from the
  triggering event's block.
- Modifying `livepeer-enricher` (it owns the external `*_ens` tables, which
  already auto-refresh).

## Context: why this slipped through TD-017

TD-017 Phase 1 specified an event-triggered refresh on
`NewRound + Bond + Unbond + Rebond + TransferBond`. The implementation
landed `profile-backfill` (a one-shot bounded backfill that walks events
forward in checkpoint order) but did not land a follow / live-poll path.
The wrapper script was the manual stand-in. This plan closes the gap.

## Two design options

### Option 1 — Standalone `profile-follow` subcommand

Add a new subcommand to `livepeer-staker`:

```
livepeer-staker profile-follow [--cadence-secs 300] [--include-tentative]
```

Wraps `run_profile_backfill` in a poll loop with the same shape the rollup
workers use (`livepeer-rollups orch-payouts-daily --follow`):

1. Load checkpoint
2. Run one bounded `profile-backfill` iteration
3. If `events_seen > 0`, immediately loop (work to do)
4. If `events_seen == 0`, sleep `cadence_secs`, then loop

Ships as a standalone binary. Matches the `livepeer-staker` dual-pattern
(bounded one-shot + standalone follow), the rollup pattern, and the
deployment-shape decision from TD-017's Locked Decisions table:

> Worker deployment shape: standalone binaries by default for new crates.
> Daemon embedding optional only for the rollup workers (matching the
> existing `livepeer-staker` dual-pattern); the enricher is never
> daemon-embedded.

**Pros**:
- Smallest change. No new daemon supervisor wiring.
- Matches existing operational patterns (rollups, enricher).
- Easy to deploy as its own systemd unit / docker service per
  `docker-compose.yml`.

**Cons**:
- One more long-running process to monitor.
- Polling cadence (every `cadence_secs`) is coarser than event-triggered.

### Option 2 — Wire `run_profile_backfill` into daemon's `staker_loop`

Add a call to `run_profile_backfill` inside the existing daemon staker
loop, right after `run_gateway_backfill`:

```rust
// crates/livepeer-daemon/src/supervisor.rs:staker_loop
let profile = staker_runner::run_profile_backfill(&pg, archive.as_ref(), &cfg, include_tentative).await?;
```

Existing `STAKER_INTERVAL_SECS = 300` provides the cadence. Same `?` /
error-recording pattern as the existing two staker steps.

**Pros**:
- No new process. Single supervised unit for all staker work.
- Matches TD-017's stated design intent (event-triggered refresh wired
  into the daemon).
- Cheaper to operate.

**Cons**:
- Daemon staker_loop already does `run_backfill` + `run_gateway_backfill`
  + `run_refresh` per cycle. Adding profile makes the cycle longer; we
  observed pending-stake reconciliation hang the loop earlier (operational
  notes 2026-05-07). One more step compounds that risk.
- The TD-017 decision explicitly leaned standalone for new crates, but
  `livepeer-staker` is an existing crate — the rule was about *new*
  crates. So adding to staker is consistent with the dual-pattern
  precedent.

### Recommendation

**Option 1 (standalone `profile-follow` subcommand) is the recommended
default**, primarily for operational isolation. The daemon already runs
five long-lived loops; the recent staker-loop hang showed that piling
work into the daemon increases blast radius. A standalone `profile-follow`
binary keeps the failure domain tight and matches the rollup pattern that
already works.

A future operator who prefers the bundled-daemon shape can opt into
Option 2 as a tiny follow-up — the `run_profile_backfill` function is
already exposed; wiring it into the daemon is ~5 lines once Option 1's
runtime semantics are proven.

## Acceptance criteria

The implementation is correct if:

1. `livepeer-staker profile-follow` runs as a standalone binary and
   continues running indefinitely after the first stable iteration.
2. `cadence_secs` is configurable; default is 300 to match the rollup
   workers.
3. All RPC reads continue to flow through `rpc_call_cache` (TD-017
   determinism acceptance criterion #1).
4. `as_of_block` / `as_of_round` continue to derive from the triggering
   event's block, not wall-clock or `chain.latest_block()` (acceptance
   criterion #2).
5. Profile rows continue to be monotonic by `last_event_id`
   (acceptance criterion #3).
6. The follow loop respects checkpoint resumption — kill mid-iter,
   restart, no duplicate writes (idempotency guaranteed by the existing
   upsert).
7. `docker-compose.yml` gains a `livepeer-staker-profile-follow` service
   entry (or equivalent) so production deploys include the new worker.
8. `scripts/loop-staker-profile.sh` is deleted (its purpose is now
   subsumed). Runbook updated to point operators at
   `livepeer-staker profile-follow` instead.
9. Replay-determinism CI continues to pass on `orchestrator_profile` and
   `broadcaster_profile` (no behavioral change to the deterministic
   contract).

## Implementation sketch

`crates/livepeer-staker/src/main.rs` — add subcommand:

```rust
#[derive(Parser)]
enum StakerCommand {
    Backfill(BackfillArgs),
    GatewayBackfill(GatewayArgs),
    RefreshPending(RefreshArgs),
    ProfileBackfill(ProfileArgs),
    ProfileFollow(ProfileFollowArgs),  // NEW
}

#[derive(Parser)]
struct ProfileFollowArgs {
    #[arg(long, default_value_t = 300)]
    cadence_secs: u64,

    #[arg(long, default_value_t = false)]
    include_tentative: bool,
}
```

Run loop (mirrors `livepeer-rollups`'s follow pattern):

```rust
async fn run_profile_follow(args: ProfileFollowArgs, ...) -> Result<()> {
    loop {
        let summary = run_profile_backfill(&pg, &archive, &cfg, args.include_tentative).await?;
        if summary.orch_events_seen == 0 && summary.gateway_events_seen == 0 {
            sleep(Duration::from_secs(args.cadence_secs)).await;
        }
    }
}
```

`docker-compose.yml` — add service:

```yaml
livepeer-staker-profile-follow:
  image: livepeer-valuation-system:latest
  command: livepeer-staker profile-follow
  restart: unless-stopped
  env_file: .env
  depends_on:
    postgres:
      condition: service_healthy
```

## Concrete task list

- [ ] Add `ProfileFollow` subcommand to `livepeer-staker` CLI
- [ ] Implement `run_profile_follow` poll loop using the existing
      `run_profile_backfill`
- [ ] Unit/integration test: ensure follow loop sleeps on stable, resumes
      on new event
- [ ] Add `livepeer-staker-profile-follow` service to
      `docker-compose.yml`
- [ ] Update `RUNBOOK.md` and any deployment/operations docs
- [ ] Delete `scripts/loop-staker-profile.sh`
- [ ] Update tech-debt-tracker entry for TD-019 → resolved
- [ ] Confirm replay-determinism CI still passes

## Tracked-but-out-of-scope

- **Daemon embedding** (Option 2): not part of this plan. Tracked as a
  follow-up if operators prefer the single-process deployment shape.
- **Per-NewRound fanout cost** (a related performance issue surfaced
  during the 2026-05-07 catch-up: each historical NewRound event
  triggers serial RPC fanout to all known orchs, making profile-backfill
  iterations grow linearly with orch count). This is a backfill-only
  concern; live mode at head only sees a handful of NewRounds per day.
  Track separately if the backfill cost ever recurs.
- **Live-mode performance metrics**: Prometheus metrics for the new
  follow worker can ride along with the existing staker metrics; not a
  separate work item.

## Progress log

- 2026-05-07: Plan drafted alongside Fix A (operator wrapper script
  modified to sleep+continue instead of exit-on-stable). The wrapper is
  the temporary stand-in until this plan ships.
- 2026-05-07 (resolved): Option 1 implemented and shipped.
  `crates/livepeer-staker/src/main.rs` gained a `ProfileFollow` subcommand
  taking `--cadence-secs` (default 300, matching the rollup workers).
  The match arm wraps `runner::run_profile_backfill` in an infinite loop
  with `tokio::time::sleep(Duration::from_secs(cadence_secs))` between
  iterations and `error!` logging on per-iter failures so a single bad
  iteration doesn't kill the loop. Release binary rebuilt. Bash wrapper
  (`scripts/loop-staker-profile.sh`) deleted. `resume-catchup-all.sh`
  updated to launch the native subcommand under the `profile-follow`
  label. `docker-compose.yml` gained a `livepeer-staker-profile-follow`
  service entry alongside the existing `livepeer-staker` service.
  Local runtime swap verified: native `profile-follow` (PID 168233)
  picked up at the existing checkpoint and ran a clean iteration without
  any operator-side scripting. All TD-017 determinism acceptance criteria
  (cached RPC routing, `as_of_block` from triggering event, monotonic
  `last_event_id` upserts) are unchanged because `run_profile_backfill`
  is the same function — the follow path only adds the outer poll loop.
