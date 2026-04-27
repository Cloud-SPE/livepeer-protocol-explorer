# Tech Debt Tracker

Known shortcuts, deferred work, and TODOs that are too small to warrant their own plan but should not be lost.

| ID | Item | Source | Severity | Status |
|---|---|---|---|---|
| TD-001 | `BondingVotes` ABI not vendored — only `LivepeerGovernor.json` available. SPEC §6.7 only lists Governor events, so v1 is unblocked, but BondingVotes proxy at `0x0B9C25...` has no registry entry. | Scaffold | low | Open. Fetch from Arbiscan when delegation events are needed (post-v1). |
| TD-002 | All SPEC §22 open data items (Q-OD-1 through Q-OD-10) marked `TODO(Q-OD-N)` in code/config — not yet resolved. | SPEC §22 | medium | **Partial.** Q-OD-1, Q-OD-2, Q-OD-3, Q-OD-4, Q-OD-7 resolved 2026-04-27 — see design-docs. Q-OD-5, Q-OD-6, Q-OD-8, Q-OD-9, Q-OD-10 still open. |
| TD-003 | `TransferBond` and `WithdrawFees` events appear in the SQLite seed but were not enumerated in SPEC §6.3. | sqlite-seed-mapping.md | medium | **Resolved** in SPEC v1.1 — added to §6.2 / §6.3 with classifications: `TransferBond` LPT-valued strict, `WithdrawFees` ETH-valued non-strict. |
| TD-004 | `events.payload` cross-check pass — verify that every RPC-derived event at `(tx_hash, log_index)` matches the SQLite payload field-by-field. Catches indexer/decoder bugs by comparing against a second decoder (the historical SQLite). | sqlite-seed-mapping.md | medium | **Promoted to v1 acceptance criterion** (SPEC v1.1, §24.1). Implementation: a `livepeer-test cross-check` binary that runs after backfill and writes a discrepancy report. |

## Resolved

| ID | Item | Resolved by | When |
|---|---|---|---|
| Q-OD-1 | LPT precision — SQLite REAL is lossy at last 2–3 of 18 LPT decimals; re-derive `amount_native` from RPC, take `*_usd` and `*_price` from SQLite. | [sqlite-seed-mapping.md](../design-docs/sqlite-seed-mapping.md) | 2026-04-27 |
| Q-OD-2 | `transaction_id` is unique in both `payout` (297,105) and `reward` (158,448) — no `log_index` disambiguation needed. | [sqlite-seed-mapping.md](../design-docs/sqlite-seed-mapping.md) | 2026-04-27 |
| Q-OD-3 | `events.payload` is denormalized JSON with consistent envelope; v1 ignores it (price-overlay, not event-mirror). Optional v1.5 cross-check (TD-004). | [sqlite-seed-mapping.md](../design-docs/sqlite-seed-mapping.md) | 2026-04-27 |
| Q-OD-4 | `block_cursors` not consumed (SPEC v1.2 simplification) — the valuator does a flat `(tx_hash, asset)` seed lookup; no per-type bound vector. The SQLite→on-chain naming bridge (e.g. `WinningTicket` → `WinningTicketRedeemed`) is still needed by the cross-check pass (TD-004). | [sqlite-seed-mapping.md](../design-docs/sqlite-seed-mapping.md) | 2026-04-27 |
| Q-OD-7 | BondingManager event field layouts: `Bond.additionalAmount` (NOT `bondedAmount`) is per-event LPT inflow; full mapping table for all stake-flow events. | [bonding-manager-event-fields.md](../design-docs/bonding-manager-event-fields.md) | 2026-04-27 |
