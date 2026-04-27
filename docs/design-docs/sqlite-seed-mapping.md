---
title: SQLite Seed Mapping & Precision Policy
status: accepted
verified: 2026-04-27
resolves: [Q-OD-1, Q-OD-2, Q-OD-3, Q-OD-4]
source: livepeer-backend-rs/sqlite-4.0.db
---

# SQLite Seed Mapping & Precision Policy

How `livepeer-seed-migrator` reads the trusted historical SQLite and maps it into `seeded_event_prices` + `indexer_checkpoints`.

Resolves SPEC §22 open data items Q-OD-1, Q-OD-2, Q-OD-3, Q-OD-4. Verified against `/home/mazup/git-repos/livepeer-backend-rs/sqlite-4.0.db` on 2026-04-27.

## Q-OD-2: transaction_id is unique within each seed table

```
payout:  297,105 rows / 297,105 distinct transaction_id
reward:  158,448 rows / 158,448 distinct transaction_id
```

**Decision:** the migrator does **not** require `log_index` to disambiguate. `seeded_event_prices` rows are inserted with `log_index = NULL`. The PK `(chain_id, tx_hash, COALESCE(log_index, -1), asset)` (SPEC §11.5) handles this — seeded rows collide on `log_index = -1` per asset, which is fine because each tx hashes to one seeded row per asset.

If a future seed source ever has multiple log indexes per tx, this assumption breaks; the migrator must reject the import in that case. Add a sanity check: `SELECT COUNT(*) FROM payout GROUP BY transaction_id HAVING COUNT(*) > 1` returns zero rows. Same for `reward`.

## Q-OD-4: `block_cursors` not consumed (SPEC v1.2)

The SQLite `block_cursors` table holds 18 per-event-type seed-coverage cursors. **We don't import them.** The valuator does a flat `(chain_id, tx_hash, asset)` lookup against `seeded_event_prices` — a hit means the seed has it, a miss means on-chain pricing. No per-type bound vector required.

This is a simplification from earlier spec versions. SPEC §8.3 lists `block_cursors` in the explicitly-ignored set alongside `orchestrator`, `broadcaster`, `proposals`, `votes`.

### Naming bridge (SQLite → on-chain)

The seed's `event_type` strings diverge from on-chain event names in a few places. Even though we don't consume `block_cursors`, the staging cross-check pass (TD-004) needs the same mapping when comparing `events.payload` rows to RPC-derived events:

| SQLite `event_type` | On-chain event name | Notes |
|---|---|---|
| `WinningTicket` | `WinningTicketRedeemed` | Sole TicketBroker payout event |
| `Withdrawal` | `Withdraw` | TicketBroker |
| `TranscoderActivation` | `TranscoderActivated` | BondingManager |
| `TranscoderDeactivation` | `TranscoderDeactivated` | BondingManager |
| `Bond` / `Unbond` / `Rebond` / `WithdrawStake` / `Reward` / `EarningsClaimed` | identical | BondingManager |
| `DepositFunded` / `ReserveFunded` / `ReserveClaimed` | identical | TicketBroker |
| `TransferBond` | `TransferBond` | (Not in SPEC §6 catalog yet — see TD-003) |
| `WithdrawFees` | `WithdrawFees` | (Not in SPEC §6 catalog yet — see TD-003) |
| `ProposalCreated` / `VoteCast` | identical | Governor |
| `TranscoderUpdate` | identical | BondingManager |

`TransferBond` and `WithdrawFees` are present in the seed but not enumerated in SPEC §6.3. They are real BondingManager events and need a SPEC update or explicit out-of-scope decision before the migrator handles them. Tracked as **TD-003**.

## Q-OD-3: `events.payload` is denormalized JSON; treated as informational only

Every payload row is a JSON object with a common envelope:

```json
{
  "block_number": "0x5ca71d",
  "block_hash":   "0x526a95...",
  "log_index":    "0xa",
  "tx_hash":      "0x8293...",
  "tx_index":     "0x5",
  "tx_addr":      "0x35bcf3c30594191d53231e4ff333e8a770453e40",
  ... event-specific fields in snake_case, hex-encoded uint256 values ...
}
```

Hex values are 0x-prefixed. Event-specific field names track the ABI input names with snake_case conversion (`additional_amount` ↔ `additionalAmount`).

`Reward` payloads additionally embed `eth_price` and `lpt_price` as JSON floats — the same precision-lossy values as `reward.eth_price` / `reward.lpt_price` columns. Use the columns, not the payload, for valuation.

**Decision:** v1 ignores `events.payload` for migration. SPEC §8.5 — "The SQLite is a price overlay, not an event mirror." We re-fetch every event canonically from RPC. Optional v1.5: cross-check that RPC-derived `(tx_hash, log_index)` exists in `events` and that field values match — strictly a sanity check, not a primary path. Tracked as v1.5 follow-up.

## Q-OD-1: LPT precision is lossy at the last 2–3 of 18 decimals

SQLite REAL is 53-bit IEEE-754 double — mantissa precision ≈ 15.95 decimal digits. On-chain LPT is `uint256` with 18 decimals. The last 2–3 of those 18 decimals cannot be faithfully represented in REAL.

Sampled rows confirm the precision floor:

```
18.19926758439107000000    16 sig figs, trailing zeros from REAL→TEXT cast
12.08276606644477000000    16 sig figs
 7.52968559572978800000    15 sig figs (note trailing 8)
```

**Decision:** apply SPEC §8.7's fallback for every seeded reward. Per `seeded_event_prices` row:
- `amount_native`     → re-derived from the on-chain `Reward` log at valuation time (NOT from `reward.total_tokens`)
- `asset_usd_price`   → from `reward.lpt_price` (lossy at LSB but acceptable for USD valuation; pricing precision is much coarser than amount precision)
- `amount_usd`        → from `reward.orch_tokens_usd` for the orchestrator portion (full row preserved in the `raw` JSONB column)

Same logic for `payout` ETH amounts: re-derive `face_value` from on-chain `WinningTicketRedeemed.faceValue`; use SQLite `eth_price` and `face_value_usd`.

This means the migrator does NOT trust SQLite for any uint256 amount. The valuator combines SQLite-trusted prices with RPC-trusted amounts at valuation time.

## Open follow-ups (not blocking)

- **TD-003** — `TransferBond` and `WithdrawFees` are present in the seed but absent from SPEC §6.3. Confirm whether they should be added to the catalog, classified, and marked critical/non-critical.
- **v1.5** — payload cross-check pass after first canonical backfill completes.
