---
status: draft
verified: 2026-05-04
---

# Continuous Catch-Up Architecture

## Purpose

Define the target architecture for a system that:

1. performs a full historical backfill from genesis (or from the current
   checkpoint state),
2. drains the derived-state backlog,
3. transitions into a long-running near-head mode, and
4. keeps the database current as new blocks arrive.

This document does **not** change the determinism contract. It only defines
the runtime/orchestration shape needed to move from one-shot batch binaries to
continuous operation.

## Decision

The system should use **three explicit operating modes** with a single shared
data model:

- `bootstrap`
- `replay`
- `follow`

`bootstrap` and `replay` remain finite, batch-oriented jobs.
`follow` is the long-running supervised mode.

The daemon must **not** be the primary engine for first-time historical
backfill. Historical catch-up and continuous near-head following are different
operating modes with different correctness and operational constraints.

## Why this split is required

The current repo already performs bounded work well:

- index historical logs
- promote finality
- price finalized events
- derive stake state

What it lacks is a persistent runtime that coordinates those same bounded
operations near head.

If one mode tried to handle all of:

- empty-database bootstrap,
- deterministic replay,
- and infinite near-head following,

the result would blur:

- determinism boundaries,
- backpressure rules,
- RPC budgets,
- graceful shutdown behavior,
- and operator expectations.

## Target operating modes

### 1. `bootstrap`

Use for:

- first deployment
- empty DB rebuild
- large historical catch-up
- operator-driven post-outage rebuild

Contract:

- finite
- resumable from checkpoints
- may populate `rpc_call_cache`
- may use live RPC on cache miss
- runs the bounded pipeline in order

Expected flow:

1. migrations / boot checks
2. optional seed import
3. indexer backfill
4. reorg/finality passes as needed
5. valuator catch-up
6. staker catch-up
7. optional cross-check / replay validation

### 2. `replay`

Use for:

- deterministic CI
- operator verification from a fixed cache snapshot

Contract:

- finite
- must not use live RPC fallback
- any missing cached RPC call is a hard failure
- runs against a fresh DB or a freshly reset derived-state slice

Expected flow:

1. migrations
2. seed import
3. bounded indexer/finality/valuator/staker replay
4. hash comparison against expected outputs

### 3. `follow`

Use for:

- steady-state operation after the system is already near head

Contract:

- infinite service
- shared RPC manager
- coordinated shutdown
- metrics / alerts
- refuses to start if lag exceeds a configured threshold

Expected flow:

1. inspect lag
2. if lag too large, exit and instruct operator to use `bootstrap`
3. otherwise enter scheduler loop
4. run bounded iterations of each task forever

## Runtime model

The correct long-running shape is a **scheduler over bounded workers**.

The daemon should not re-implement business logic. It should repeatedly call
the same bounded work functions used by batch mode.

### Task graph

```text
indexer -> finality -> valuator -> staker
     \         |
      \        v
       \-> reorg/audit
```

Interpretation:

- **Indexer** creates raw events and advances ingestion checkpoints.
- **Finality** determines what is safe for downstream accounting.
- **Valuator** consumes finalized valuable events without terminal outcomes.
- **Staker** consumes stake-touching events and exact-state/pending refresh work.
- **Reorg** monitors canonical continuity and forces downstream reprocessing when needed.

### Scheduler responsibilities

The scheduler owns:

- startup mode selection
- shared shutdown signal
- per-task cadence / budget
- shared RPC manager
- lag accounting
- health/metrics
- error classification / retry policy

The scheduler does **not** own pricing math, stake math, or decode logic.

## Required code shape

Each worker must expose a bounded library entrypoint.

Target shape:

```rust
pub async fn run_once(ctx: &IterCtx) -> Result<IterSummary>;
```

Where `IterCtx` contains:

- DB pool
- shared provider manager
- config
- cancellation signal
- optional per-iteration limits

And `IterSummary` reports:

- units processed
- checkpoint movement
- lag before/after
- cache hits/misses
- retry/failure counts

Current CLI binaries then become thin wrappers around `run_once(...)` or a
bounded loop of `run_once(...)`.

## Shared RPC manager

`follow` mode requires a single shared RPC resource manager.

It must own:

- provider selection
- concurrency caps
- method-class budgets
- connection refresh / rotation policy
- provider health state
- metrics

This is load-bearing because batch-mode “one fresh client per process” does not
translate safely to continuous operation, especially with the known TD-011 LPT
throughput pathology.

## Mode transitions

The system should move between modes based on lag, not operator hope.

Suggested rules:

- If DB is empty or checkpoints absent: `bootstrap`
- If lag is very large: `bootstrap`
- If replay flag is set: `replay`
- If lag is within configured threshold: `follow`

Suggested threshold examples:

- `follow` start allowed only when indexer lag <= `50_000` blocks
- `bootstrap` remains the required path above that threshold

Exact numbers are operational config, not architectural constants.

## Backpressure model

The daemon should schedule by **available bounded work**, not fixed blind
sleep loops.

Each task asks:

- Indexer: what block range is unindexed?
- Finality: what rows can be promoted?
- Valuator: what finalized valuable rows have no terminal outcome?
- Staker: what stake rows or claim rows still need reconciliation?

This yields:

- cleaner backpressure
- simpler lag metrics
- better shutdown behavior
- fewer pointless wakeups

## Determinism contract

This architecture does not weaken any load-bearing invariants.

Required invariants remain:

- raw events are immutable except audited reorg mutation handling
- valuation rows remain immutable/versioned
- `rpc_call_cache` + seed remain the replay backbone
- bounded worker output must be byte-identical whether invoked by batch CLI or daemon

Acceptance for the architecture change is therefore not “daemon runs.” It is:

- same inputs
- same outputs
- different orchestration only

## Migration plan

### Phase 1 — bounded orchestration

Introduce a small top-level orchestration binary with:

- `bootstrap`
- `replay`

No daemon yet. The goal is to formalize finite-run operator workflows first.

### Phase 2 — `livepeer-daemon follow`

Introduce a daemon crate that:

- reuses the same bounded worker library functions
- starts only near head
- coordinates indexer/finality/valuator first

Staker and reorg can remain separate binaries initially if needed for simpler rollout.

### Phase 3 — production hardening

Add:

- Prometheus metrics across tasks
- alerting
- provider failover policy
- operator runbook hardening

## Non-goals

This decision does not introduce:

- multi-instance HA
- distributed work claiming
- replacing historical backfill with daemon mode
- changing the DB schema by itself

## Consequences

Positive:

- clean operator model
- explicit separation between replay, bootstrap, and steady-state follow
- safer RPC resource sharing
- bounded-task scheduler aligns with determinism

Negative:

- requires refactoring current binaries into reusable libraries
- adds a new runtime layer to reason about
- cannot fully land until TD-011 has a stable production RPC ceiling

## Verification target

This doc becomes accepted only after:

1. the bounded library entrypoints exist,
2. `bootstrap` and `replay` exist and are validated,
3. `follow` runs against the same DB deterministically,
4. replay output matches batch output on the same cached inputs.
