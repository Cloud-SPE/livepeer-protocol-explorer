# TD-022: Profile Worker RPC Batching via Multicall3

**Status:** Draft, awaiting sign-off
**Author:** 2026-05-08
**Severity:** medium
**Source:** Profile-follow throughput investigation 2026-05-08 — measured ~39 h to chain-head catch-up at current rate

## Problem

`livepeer-staker profile-backfill` / `profile-follow` materializes
`orchestrator_profile` rows by walking event-driven snapshot triggers
(primarily `NewRound`). Each `NewRound` triggers a fan-out across
all 1,936 known orchestrators; each orchestrator snapshot makes 2 distinct
on-chain `eth_call`s (`BondingManager.transcoderTotalStake` and
`ServiceRegistry.getServiceURI`).

That's **3,872 RPC calls per `NewRound`** event, sequenced through
`buffer_unordered(12)` against Chainstack's archive endpoint. With 1,703
historical `NewRound` events in the canonical-finalized event set, full
backfill = **~6.6 M RPC reads**.

Empirical throughput at concurrency 12: **~24 RPC/sec sustained**. Theoretical
cold-cache full-backfill: **~76 h**. Observed catch-up rate from the live
2026-05-08 follow-loop: **~39 h to chain head from current ~25% state**.

This is structural — not a bug, not a regression. The original concurrency
ceiling of 12 is empirically correct (24 and 32 both hit Chainstack 429s
during cold-cache bursts; see `profile.rs:NEW_ROUND_FANOUT_CONCURRENCY` for
the prior investigation).

The cost is real: profile-follow can't catch up before fresh `NewRound`
events arrive (~1/day on Arbitrum), so the live profile tables remain
permanently stale by ~1.6× the round cadence. Operators see this as
"`orchestrator_profile.updated_at` is hours behind chain head" and treat it
as an alert condition.

## Resolution

Replace the `single_call_cached` per-orchestrator fan-out with a
**Multicall3-batched** path that keeps the existing cache-keyed semantics:

- One RPC per batch of ~100 calls (vs. 1 RPC per call today)
- Each constituent call's response is **still stored in `rpc_call_cache`
  under its original `(method, params, block_number)` key**, so the
  determinism contract is preserved bit-exactly and existing replay
  fixtures don't need to change
- Combined with one small ServiceURI memoization win, expected speedup is
  **~100× RPC count reduction** → 76 h theoretical → **~25 min** for full
  backfill, ~3,872 calls/NewRound → ~40 calls/NewRound

## Scope

**In scope:**
- Add Multicall3 ABI under `crates/abi/Multicall3.json`
- New helper `livepeer_core::rpc::multicall::aggregate3_cached(...)` —
  cache-aware batch executor with the same individual-key cache shape as
  `single_call_cached`
- Refactor `read_orchestrator_snapshot` (and the gateway equivalent if it
  fan-outs similarly) to pass a Vec of pending calls into the batch helper
- Memoize `ServiceRegistry.getServiceURI(orch)` per orch since the value
  changes only on `ServiceURIUpdate` events (rare)
- Determinism: re-test the strict replay fixtures and confirm bit-identical
  hashes
- Production cutover + metrics (rpc batch count, batch fill ratio,
  per-batch latency)

**Out of scope:**
- Pre-filtering inactive orchs (Lever 3 in the investigation): changes
  the per-round snapshot row count and would require a SPEC update.
- Reconstructing snapshots from event deltas (Lever 4): `transcoderTotalStake`
  is delegator-driven and not derivable from event logs alone.
- Concurrency ceiling changes — the batched path needs far less concurrency
  to begin with, so the existing ceiling stays at 12.

## Multicall3 background

Multicall3 is deployed at the universal address
`0xcA11bde05f7d3D8ec0b6066beE9d9e8F26c5dE4D` on Arbitrum One (and most
EVM chains). The relevant function:

```solidity
struct Call3 {
    address target;
    bool   allowFailure;
    bytes  callData;
}

function aggregate3(Call3[] calldata calls)
    external payable
    returns (Result[] memory returnData);

struct Result { bool success; bytes returnData; }
```

Cost: ~21,000 gas + ~700 gas per call inside the batch. Since this is a
read-only `eth_call` (no state writes), gas is irrelevant. The only
practical limits are response size (a few hundred KB max from Chainstack)
and per-call decoder bytes — comfortable up to **~200 calls per batch**
for our shape.

## Architecture: cache-shape preservation

The load-bearing design choice. Today every successful RPC writes one
row to `rpc_call_cache` keyed by:

```
call_hash = blake3(method || params || block_number)
```

For an `eth_call` to BondingManager: `params = [{to, data}, blockTag]`.

After TD-022, the batched path is:

```rust
pub async fn aggregate3_cached(
    pg: &PgPool,
    archive: &Provider,
    block_number: i64,
    calls: Vec<PendingCall>,    // each carries method="eth_call", params, block
) -> Result<Vec<Vec<u8>>>
{
    // 1. Probe cache for each pending call; collect hits + misses
    let mut results = vec![None; calls.len()];
    let mut misses: Vec<usize> = vec![];
    for (i, c) in calls.iter().enumerate() {
        let call_hash = cache::compute_call_hash(&c.method, &c.params, Some(block_number));
        if let Some((bytes, _, _)) = cache::get(pg, &call_hash).await? {
            results[i] = Some(bytes);
        } else {
            misses.push(i);
        }
    }

    if misses.is_empty() {
        return Ok(results.into_iter().map(Option::unwrap).collect());
    }

    // 2. Build aggregate3 payload from misses
    let aggregate_calls: Vec<Call3> = misses.iter().map(|&i| {
        let c = &calls[i];
        Call3 {
            target: c.target,
            allowFailure: false,
            callData: c.calldata.clone(),
        }
    }).collect();
    let mc_data = Multicall3::aggregate3Call { calls: aggregate_calls }.abi_encode();
    let mc_params = json!([
        { "to": MULTICALL3_ADDRESS, "data": format!("0x{}", hex::encode(mc_data)) },
        BlockTag::Number(block_number as u64).to_param()
    ]);

    // 3. ONE RPC for the whole batch — note: this batch RPC is NOT cached
    //    as a unit. Only its decoded constituents are.
    let raw = archive.call("eth_call", &mc_params).await?;
    let decoded = Multicall3::aggregate3Call::abi_decode_returns(&raw, true)?;

    // 4. For each miss: store the individual response in rpc_call_cache
    //    keyed by ORIGINAL (method, params, block_number) — i.e. the same
    //    key it would have had under single_call_cached. Replay reads this
    //    row and is byte-identical.
    for (out_idx, miss_idx) in misses.iter().enumerate() {
        let c = &calls[*miss_idx];
        let response = &decoded._0[out_idx];
        if !response.success {
            anyhow::bail!("multicall sub-call failed for {:?}", c);
        }
        let call_hash = cache::compute_call_hash(&c.method, &c.params, Some(block_number));
        let bytes = serde_json::to_vec(&Value::String(format!("0x{}", hex::encode(&response.returnData))))?;
        let hash = cache::hash_response_bytes(&bytes);
        cache::store(pg, &call_hash, &c.method, &c.params, Some(block_number),
                     &bytes, &hash, archive.name(), None, None).await?;
        results[*miss_idx] = Some(bytes);
    }

    Ok(results.into_iter().map(Option::unwrap).collect())
}
```

**Key invariants:**
- A constituent call's `rpc_call_cache` row is byte-identical to what it
  would have been under `single_call_cached`.
- Replay-mode operations never touch the multicall path (everything is in
  cache by construction); they continue to call `single_call_cached`.
- Cache-only mode (`livepeer-orchestrator replay --no-rpc`) works
  unchanged — the `aggregate3_cached` function returns from cache without
  ever touching `archive.call`.
- The multicall batch RPC itself is NOT stored as a cache row. It's an
  intermediate that gets decomposed.

This means **no replay fixture regeneration** for any historical run that
already populated `rpc_call_cache`. The fixture format is unchanged.

## Phases

### Phase A — Multicall3 ABI + helper (½ day)

1. Vendor `crates/abi/Multicall3.json` (ABI from
   https://github.com/mds1/multicall — `Multicall3.json` artifact).
2. Add `pub mod multicall;` to `crates/core/src/rpc/mod.rs`.
3. Implement `aggregate3_cached(pg, archive, block_number, calls)` per
   the architecture above.
4. Unit test: build a 3-call batch (e.g. 3 `transcoderTotalStake` calls
   for known orchs at a recent block), assert (a) 1 RPC made, (b) 3
   cache rows written keyed by original params, (c) results match what
   `single_call_cached` would have returned.

**Acceptance:** unit tests green; cache rows are bit-identical between
batched and individual paths.

### Phase B — Refactor `read_orchestrator_snapshot` (½ day)

1. Split the snapshot-read logic into:
   - A "build pending calls" pass that returns `Vec<PendingCall>` per
     orch (3 entries: stake, controller→service_registry, service_uri)
   - A "decode" pass that turns the response bytes back into typed fields
2. The fan-out across orchs becomes:
   - Build one big Vec of pending calls (all orchs × 3 calls)
   - Pass to `aggregate3_cached(block_number, all_calls)` in batches
     of 100 (so each RPC stays well within Chainstack response budget)
   - Decode each orch's response group
3. Preserve the existing structure of `OrchestratorSnapshot` and the
   upsert path — only the read step changes.

**Acceptance:** running `profile-backfill --batch-limit 1` against a
single NewRound event emits 1-2 RPC calls (down from ~3,872) and writes
the same `orchestrator_profile` row as before.

### Phase C — ServiceURI memoization (½ day)

1. Add an in-memory `HashMap<String /* orch */, (i64 /* block */, Option<String>)>`
   inside `run_profile_backfill`'s outer state.
2. On each orch snapshot: check if the cached entry's block ≥ the latest
   `ServiceURIUpdate` event's block for that orch. If yes, skip the RPC
   and reuse. If no, query.
3. The replay path is unaffected — `rpc_call_cache` already memoizes at
   the same shape.

**Acceptance:** running on a fully-backfilled rpc_call_cache (warm),
`profile-backfill` makes near-zero ServiceURI calls in steady state.

### Phase D — Determinism replay re-test (½ day)

1. Run `scripts/run-determinism-replay.sh` against both case-a and case-b
   fixtures. Expected outcome: **green, no fixture changes needed.**
2. If any hash diff appears, investigate — the design intent is that
   replay results are bit-identical because cache contents are
   bit-identical.

**Acceptance:** replay green for both committed cases.

### Phase E — Production cutover + metrics (½ day)

1. Build release binary; restart `livepeer-staker profile-follow` with
   the new logic.
2. Add Prometheus counters via `livepeer_staker::metrics`:
   - `staker_multicall_batches_total{result}` — batch count
   - `staker_multicall_calls_packed_total` — total constituent calls
   - `staker_multicall_calls_per_batch` — gauge of avg fill rate
   - `staker_multicall_batch_seconds` — gauge of last batch latency
3. Watch `staker_orch_profile` checkpoint advance rate over a 30 min
   window. Expected: **>100× faster** vs. baseline.
4. Spot-check 10 random orchs in `orchestrator_profile` before/after —
   `total_stake` and `service_uri` should be byte-identical.

**Acceptance:** observed catch-up rate ≥ 1 M blocks/min (vs. ~22 K/min
today); profile tables converge to chain head within hours instead of
days.

## Risks

| Risk | Mitigation |
|---|---|
| Multicall3 response too large for some block ranges | Batch ceiling of 100 calls/RPC; each call returns ~32 bytes typed → ~3.2 KB per batch. Chainstack budget is far higher. If we ever hit it, drop batch size. |
| `ServiceRegistry.getServiceURI(orch)` reverts for an orch with no URI | `aggregate3` with `allowFailure=false` would propagate the revert. We use `allowFailure=true` and treat reverts as `None` (matches today's behavior — the read returns empty string today). |
| Cache write race when multiple concurrent batches touch the same call | `cache::store` uses `ON CONFLICT (call_hash) DO NOTHING`. Both writers store identical bytes; whichever loses the race is fine. |
| Replay reading a partial cache (some sub-calls cached, others not) | The `aggregate3_cached` helper handles this naturally — it only includes misses in the new batch RPC. Hits are served from cache. |
| Determinism contract regression | Phase D explicitly retests both fixtures. The architectural promise (constituent cache rows byte-identical) means a regression here would also be a unit-test failure in Phase A. |

## Open questions for sign-off

1. **Batch size — 100 or higher?** Chainstack response budget supports
   200+ for our small response shape, but bigger batches mean longer
   per-RPC latency. Recommend starting at 100; tune up if benchmarks
   suggest headroom.
2. **Should `ServiceURI` memoization go in core or stay scoped to
   `livepeer-staker`?** Keep it in staker — it's profile-specific and
   the in-memory map is short-lived.
3. **Apply the same batching to gateway profile reads?** `read_gateway_snapshot`
   is also fan-out-shaped (per-gateway across each Deposit/Withdrawal
   event). The same helper applies. Recommend: yes, fold gateway snapshots
   into Phase B as a same-day extension.

## Dependencies

- Multicall3 deployed on Arbitrum One: confirmed at
  `0xcA11bde05f7d3D8ec0b6066beE9d9e8F26c5dE4D`.
- TD-019 (profile-follow) — already shipped, this is its successor.
- No upstream blockers.

## Estimated effort

- Phase A: 0.5 day
- Phase B: 0.5 day
- Phase C: 0.5 day
- Phase D: 0.5 day
- Phase E: 0.5 day
- **Total active-coding time: ~2.5 days**
- **Total wall-clock from green-light to closed: ~3 days**

## Expected impact

| Metric | Before | After |
|---|---|---|
| RPC calls per NewRound | ~3,872 | ~40 |
| Full-backfill RPC reads | ~6.6 M | ~66 K |
| Theoretical full-backfill time | ~76 h | **~25 min** |
| Live-mode catch-up after a NewRound | ~50 s | **<1 s** |
| Live profile staleness vs. round cadence | ~1.6× | **steady-state caught up** |

This eliminates the structural staleness in `orchestrator_profile` /
`broadcaster_profile` and removes the operator pain point of
"profile-follow looks stuck."
