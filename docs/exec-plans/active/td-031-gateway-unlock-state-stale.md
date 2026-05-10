# TD-031: Gateway unlock state stale in API matview

**Status:** Open
**Author:** 2026-05-10
**Severity:** medium-high (user-visible financial display drift)
**Source:** TD-030 Phase A `scripts/validate-vs-onchain.sh` first runs against prod, 2026-05-10

## Symptom

Every gateway currently in `Unlock`-in-progress state on-chain is misreported by the API as `unlock_in_progress=false` with a non-zero `latest_reserve` value, while the chain shows `withdrawRound != 0` and `getReserveInfo.fundsRemaining ≈ 0`.

Two consecutive runs of `validate-vs-onchain.sh` (random samples of 10 gateways each) returned 8/10 and 9/10 FAILs with this exact pattern. **The most extreme case observed:** gateway `0xc3c7c4…36e61e` (Livepeer Inc) reports `latest_reserve=18` ETH in the API while the chain reserve is essentially zero (~5e-16 ETH dust). At ETH ≈ $3000, that's a **~$54k display lie** on the front-end.

Reproducible runs:

```
$ bash scripts/validate-vs-onchain.sh --api-url https://livepeer-api.xode.app
...
--- GATEWAYS ---
  FAIL   0x68d6ff…4496f7  DRIFT: reserve api=0.361 chain≈0; unlock_in_progress api=false chain=true
  FAIL   0xca3331…fb7fef  DRIFT: deposit Δ-0.27 ETH; reserve api=0.496 chain≈0; unlock_in_progress api=false chain=true
  FAIL   0x5ae4e4…ec3d0b  DRIFT: deposit Δ-0.022 ETH; reserve api=0.34 chain≈0; unlock_in_progress api=false chain=true
  ...
  FAIL   0xc3c7c4…36e61e  DRIFT: deposit Δ-0.038 ETH; reserve api=18 chain≈0; unlock_in_progress api=false chain=true
```

The two PASSing gateways in each run were already fully drained (`deposit=0`, `reserve=0`, `unlock=true`) — they pass only because zero matches zero, not because the matview tracked their unlock correctly.

## Where the wiring looks right

1. `crates/livepeer-staker/src/gateway.rs:524,543,553` — the snapshot writer reads `TicketBroker.isUnlockInProgress(addr)` and persists `unlock_in_progress` into `gateway_balances_by_block`.
2. `crates/livepeer-staker/src/gateway.rs:259-268` — the candidate-finder SQL DOES filter for `'Unlock'` and `'UnlockCancelled'` event names alongside the deposit/reserve/withdrawal events. So an Unlock should trigger a snapshot refresh.
3. `migrations/042_replace_broadcaster_profile_with_view.up.sql:23-36` — the matview projects `unlock_in_progress` and `withdraw_round` from `gateway_balances_by_block` via `DISTINCT ON (chain_id, gateway_address) … ORDER BY block_number DESC`. It picks the latest snapshot per gateway, which is correct.
4. `crates/livepeer-api/src/routes/gateways.rs:41,1043` — the API serializes both fields from the matview.

So the data layer is plumbed correctly. The bug is upstream: snapshots reflecting the post-Unlock state are not being written to `gateway_balances_by_block`.

## Hypotheses (ranked by likelihood)

### H1: `from_address` not populated on `Unlock` events at indexer time

The candidate-finder requires `r.from_address IS NOT NULL` (`gateway.rs:258`) to match. The TicketBroker `Unlock` event signature is `event Unlock(address indexed sender, uint256 startRound, uint256 endRound)` — the sender is an indexed topic. If the indexer's `raw_protocol_events` row-shaping logic sets `from_address` from the `sender` parameter for `DepositFunded`/`ReserveFunded` (which it must, since those snapshots ARE being captured) but not for `Unlock` (perhaps because the sender field has a different name in the ABI, e.g. `_sender` vs `sender`), then the candidate-finder would silently skip every Unlock event.

**To verify:** query the prod DB directly:

```sql
SELECT event_name, COUNT(*) AS total,
       COUNT(*) FILTER (WHERE from_address IS NOT NULL) AS with_from
  FROM raw_protocol_events
 WHERE contract_name = 'TicketBroker'
   AND event_name IN ('DepositFunded','ReserveFunded','Unlock','UnlockCancelled','Withdrawal')
 GROUP BY event_name;
```

If `Unlock` shows `with_from = 0` while `DepositFunded` shows `with_from = total`, H1 is confirmed.

### H2: Unlock events ARE candidates but the snapshot logic skips them via the `LEFT JOIN g.gateway_address IS NULL` clause

The candidate-finder dedupes by `(gateway_address, block_number)` — if a snapshot already exists at that exact block (e.g. because the gateway also did a Deposit at the same block), the Unlock candidate is silently dropped. Less likely to explain the systematic pattern, but possible for some specific gateways.

**To verify:** for one affected gateway, check whether a snapshot exists at the block their Unlock fired:

```sql
SELECT g.block_number, g.unlock_in_progress, g.withdraw_round, g.deposit, g.reserve_funds_remaining
  FROM gateway_balances_by_block g
 WHERE g.gateway_address = '0xca3331d67e87816adb30d9562a6e8c0623fb7fef'
 ORDER BY g.block_number DESC
 LIMIT 10;
```

Compare the latest `unlock_in_progress` to the chain's `isUnlockInProgress` at the same block.

### H3: Snapshot is taken but `isUnlockInProgress` returns the pre-Unlock value due to RPC block lag

When the worker processes an Unlock candidate at block N, it eth_calls `isUnlockInProgress` against `latest` block — but what if `latest` on the RPC is still at block N-1 due to provider caching? The snapshot would record `unlock_in_progress=false` and the matview would forever show that wrong value (since later events at later blocks wouldn't necessarily change unlock state).

**To verify:** check whether the `block_number` field on the affected gateway snapshots matches an Unlock event's block, and whether `unlock_in_progress` was stored as `false` at that snapshot.

## Phases

### Phase A — Diagnose (~1 hour)

Run the three SQL queries from H1/H2/H3 above against prod DB. Report which hypothesis matches. Consider running:

```bash
# Pick one specific affected gateway and trace its full history
GW=0xca3331d67e87816adb30d9562a6e8c0623fb7fef

# 1. All TicketBroker events for this gateway
docker exec ... psql ... -c "
  SELECT block_number, event_name, from_address IS NOT NULL AS has_from, log_index, finality
    FROM raw_protocol_events
   WHERE contract_name = 'TicketBroker'
     AND (from_address = '$GW' OR to_address = '$GW')
   ORDER BY block_number DESC LIMIT 20;"

# 2. All snapshots for this gateway
docker exec ... psql ... -c "
  SELECT block_number, unlock_in_progress, withdraw_round, deposit, reserve_funds_remaining,
         triggering_event_id
    FROM gateway_balances_by_block
   WHERE gateway_address = '$GW'
   ORDER BY block_number DESC LIMIT 10;"
```

Cross-reference: did an Unlock event fire? Was a snapshot written at that block? If a snapshot was written, what value did `isUnlockInProgress` return?

### Phase B — Fix (~1-3 hours, depending on root cause)

**If H1 (most likely):** patch the indexer's TicketBroker event-shaping to populate `from_address` for `Unlock`/`UnlockCancelled` events. Probably a one-line fix in the per-event mapping (e.g. `crates/livepeer-indexer/src/decoders/ticket_broker.rs` or wherever).

**If H2:** change the candidate-finder JOIN to also match on `(gateway_address, event_id)` so multiple events at the same block can each trigger a snapshot — but this would over-snapshot. Better: change the matview semantic to prefer rows where `triggering_event_id` corresponds to an Unlock event when both exist at the same block. This is more invasive.

**If H3:** in the snapshot writer, if the triggering event is `Unlock`/`UnlockCancelled`, force the eth_call to use `block_number = candidate.block_number` rather than `latest`. This is a small change but requires archive RPC.

### Phase C — Backfill stale gateways (~30 min)

Once the fix lands, the live worker will start writing correct snapshots for new Unlock events. But the ~9 currently-affected gateways will stay stale until the gateway does another deposit/withdrawal/etc.

A small backfill: run a one-shot that reads each currently-active gateway, eth_calls `getSenderInfo` + `isUnlockInProgress` at chain head, and writes a fresh snapshot. Could be a new flag on `livepeer-staker gateway-backfill --refresh-active` or just a SQL+RPC script.

### Phase D — Verify (~5 min)

Re-run `bash scripts/validate-vs-onchain.sh --all-active` against prod. Expect 0 gateway FAILs (modulo any gateway whose unlock state changed during the run, which is normal noise).

## Risks

| Risk | Mitigation |
|---|---|
| Phase A confirms a different root cause than H1/H2/H3 | The diagnostic SQL output is dispositive; pivot to whatever it reveals |
| Phase B fix in indexer touches deterministic event-shaping (regenerates abi_hash_used) | If H1 is the cause and we fix indexer-side, run `livepeer-orchestrator replay` against the determinism fixtures to confirm no regression. May need to regenerate the fixtures (TD-024 pattern). |
| Backfill writes a snapshot at the current head block — drift returns at next round close if matview snapshot logic is the actual issue | Phase A diagnostic should rule this in/out before we fix. |

## Estimated effort

- Phase A: 1 h
- Phase B: 1-3 h
- Phase C: 0.5 h
- Phase D: 5 min
- **Total: ~3-5 hours**

## Dependencies

- TD-014/15/16 (gateway phase 2/3/operability) — Resolved. This bug is in the existing flow, not a new feature.
- TD-030 Phase A — landed (the script that surfaced this).

## Future-proofing

Once this is fixed, TD-030 Phase B (the Rust `validate` subcommand) should include a `--check unlock-consistency` invariant that explicitly compares `unlock_in_progress` against `isUnlockInProgress` for every active gateway. Catches this class of bug as a CI gate going forward.
