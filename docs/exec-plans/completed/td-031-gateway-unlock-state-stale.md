# TD-031: Gateway unlock state — script parser bug, not matview bug

**Status:** Resolved 2026-05-10 — indexer fix landed; the originating "matview is stale" claim turned out to be a parser bug in the validation script, not a real API defect.
**Author:** 2026-05-10
**Severity:** low (originally filed as medium-high based on the false-positive finding)
**Source:** TD-030 Phase A `scripts/validate-vs-onchain.sh` first runs against prod, 2026-05-10

## Honest summary

Originally filed as a user-visible matview-drift bug after `validate-vs-onchain.sh` reported 8-9 of every 10 random gateways with `unlock_in_progress=false` in API but `true` on chain, plus systematic reserve drift (worst case "Livepeer Inc reports 18 ETH reserve vs ~zero on chain — ~$54k display lie").

**Phase A diagnosis revealed two separate findings, only one of which was a real API bug, and that one had minimal user-visible impact:**

### Finding 1: Script parser bug (the actual cause of all the FAIL reports)

The script's `getSenderInfo` parser used `re.findall(r'\d+', s)` on cast's default text output. Cast adds scientific-notation annotations to large uint outputs:

```
(151901607346938834 [1.519e17], 0)
```

The regex extracted digits from inside `[1.519e17]` as if they were separate numbers, mangling the field assignments:

| Expected | Got |
|---|---|
| `deposit = 151901607346938834` | `deposit = 151901607346938834` ✓ |
| `withdrawRound = 0` | `withdrawRound = 1` ✗ (digit pulled from "1.519") |
| `fundsRemaining = 361000000000000000` | `fundsRemaining = 519` ✗ (digit pulled from "1.519e17") |
| `claimedInCurrentRound = 0` | `_ = 17` ✗ (digit pulled from "e17") |

So every gateway with a non-trivial deposit was reported as `unlock=true` (because `withdraw_round=1 → !=0`) and `reserve≈0` (because `reserve_raw=519` wei is essentially zero ETH). All ~17 gateways flagged across two runs were false positives.

Direct cross-check after the parser fix: the same `0xc3c7c4…36e61e` "Livepeer Inc" gateway that was reported as "18 ETH reserve display lie" actually has `latest_reserve=0.099...` in the API and the same on chain. **No display lie. No drift. The script lied.**

**Fix:** commit `f12f540` switches the parser to `cast call ... --json` (returns nested arrays of ints — no annotations, no parsing ambiguity).

After the fix: 15/15 random gateways in dev PASS.

### Finding 2: Indexer Unlock decoder bug (real but tiny impact)

While diagnosing the false positive, the H1 hypothesis SQL revealed a real defect:

```
event_name      | total | with_from
----------------+-------+----------
Unlock          |    26 |        0   ← all NULL
UnlockCancelled |    11 |       11   ← fine
```

`crates/livepeer-indexer/src/backfill.rs:913-914` had an empty Unlock decoder:

```rust
} else if topic0 == TicketBroker::Unlock::SIGNATURE_HASH {
    decoded!("Unlock", {});  // ← no decode call, no field assignments
}
```

`UnlockCancelled` next to it was correctly decoded. This left `from_address` NULL on every Unlock row, which silently filtered them out of the gateway-balance-backfill candidate query (filter requires `from_address IS NOT NULL`).

**User-visible impact was minimal** because the matview's `DISTINCT ON (gateway_address) ORDER BY block_number DESC` picks the latest snapshot per gateway, and other event triggers (`DepositFunded`, `ReserveFunded`, `Withdrawal`, ticket events) usually write a fresh snapshot post-Unlock that captures the `withdrawRound`/`reserve` change. The bug only mattered for gateways that called `Unlock` and then had no other on-chain activity.

**Fix:** commit `946e882` adds `Unlock` to the imports and decodes the indexed `sender` into `row.from_address`, mirroring the existing `UnlockCancelled` handler.

**Backfill applied to dev:** existing 26 historical Unlock rows updated via `UPDATE … SET from_address = '0x' || lower(substring(raw_event->'topics'->>1 from 27))`. Worker checkpoint rolled back to `MIN(Unlock.block_number) - 1`, gateway-backfill re-run, wrote 26 fresh snapshots across 18 affected gateways. Matview refreshed. No visible state change in the API (because the matview was already correct via other event triggers — confirming the user-visible impact was minimal).

## Lessons / process notes

- **`cast` text output is hostile to regex parsing** because of the bracketed scientific-notation annotations on large uints. Use `--json` for any programmatic consumption. Documented inline in the script header.
- **One false-positive finding led to one real underlying bug.** The script bug WAS the catalyst that surfaced the indexer bug — even though the original symptom claim was wrong, the diagnostic SQL was correct and the underlying defect was real. Validation tooling earns its keep even when its specific findings are misread.
- **The "$54k display lie" framing in the original TD was wrong.** Should have spot-checked one gateway with both API and chain reads (without the script in between) before writing severity-medium-high TDs. Lesson: when validation reports systematic drift, sanity-check the validator before believing it.

## Related observation (NOT this TD): orchestrator cut snapshot lag

While running validate against dev, one orchestrator (`open-pool.eth` / `0x5263e0…30b077`) flagged real drift: API says `reward_cut_percent=15.4`, chain says `30`. Chain's `lastRewardRound=4194` and `lastActiveStakeUpdateRound=4195` indicate the cut was updated after the round-4193 snapshot the matview reflects. The API is honest about this (`as_of_round: "4193"` is in the response).

This is by-design snapshot lag, not a defect — the matview tracks per-round snapshots, and orchs can update cuts mid-round via `TranscoderUpdate` events that don't trigger a fresh snapshot. Worth a separate consideration if the FE ever needs live cut values, but not in scope here.

## Status

- Indexer fix: **committed `946e882`**.
- Script parser fix: **committed `f12f540`**.
- Dev backfill: applied; matview refreshed; validate confirms 15/15 gateway PASS.
- Prod: needs the new image deployed to pick up the indexer fix. The fix is forward-looking only; the existing 26 prod Unlock rows will need the same SQL backfill after the new binary is deployed (or operators can run the same `UPDATE … from_address = '0x' || lower(substring(raw_event->'topics'->>1 from 27))` query directly against the prod DB anytime — the fix is idempotent because the candidate-finder dedupes on `(gateway_address, block_number)`).
