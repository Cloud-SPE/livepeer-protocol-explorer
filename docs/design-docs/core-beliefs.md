# Core Beliefs

Operating principles that hold across every change in this repository. Any code change that conflicts with one of these is wrong by default and requires an explicit, reviewed exception.

These restate the load-bearing portions of [SPEC §2](../product-specs/v1-livepeer-indexer.md#2-design-principles) and [§4.2](../product-specs/v1-livepeer-indexer.md#42-external-tools). The spec is authoritative; this file exists so that an agent reading only the entry points still picks them up.

---

## 1. Byte-deterministic replay is the load-bearing correctness guarantee

Given a fixed `rpc_call_cache` + seeded SQLite, dropping the database and replaying produces a byte-identical output database. Every design choice subordinates to this. CI enforces it (SPEC §12.4).

If a change you are making would make the output non-deterministic, stop and either reframe the change or bump the `valuation_version`.

## 2. Raw events are immutable

The single permitted mutation to `raw_protocol_events` is the reorg-induced `block_number` / `block_hash` update, fully audited via `reorg_mutations`. No other field of any persisted event row is ever updated.

## 3. Valuations are immutable and versioned

`event_valuations` rows are write-once per `(event_id, valuation_version, asset)`. New pricing logic gets a new `valuation_version`. Old rows remain self-consistent forever. If a worker tries to write a conflicting valuation, the conflict fires a CRITICAL determinism alert.

## 4. Foundry `cast` reproduces every price

Every USD valuation must be reproducible by a human running `cast call ... --block N`. If a price cannot be reproduced this way, the pricing logic has a bug.

## 5. No external pricing APIs in the primary path

CoinGecko, CoinMarketCap, etc. are forbidden in the primary pricing chain or the audit trail. Prices come from the on-chain TWAP/Chainlink path or the trusted SQLite seed — nothing else.

## 6. Strict-decode halts on critical events

Decode failure on any event in the critical-events allowlist (SPEC §6.2: `Bond`, `Unbond`, `Rebond`, `WithdrawStake`, `Reward`, `EarningsClaimed`, `WinningTicketRedeemed`, `WinningTicketTransfer`, `Transfer`) halts the indexer until an operator updates the ABI registry. Non-critical decode failures dead-letter to `decode_failures`.

## 7. Migrations are immutable once merged

Forward-only. No editing past migrations. No production downs. Destructive migrations require an explicit `--allow-destructive` operator flag.

## 8. Idempotent writes everywhere

Every persistence path has a defined conflict key and `ON CONFLICT DO NOTHING` (or `GREATEST`-style monotonic update for checkpoints). Re-running any backfill command is always safe.

## 9. Single-instance workers in v1

Each long-running service runs as exactly one process. No claim mechanism, no `FOR UPDATE SKIP LOCKED`. Horizontal scaling is a v2 concern.

## 10. Listening on proxy, decoding via target

The indexer subscribes to **proxy** addresses. Target addresses come from the Controller resolver at boot and are used only to select an ABI registry row. The Controller is the only address hardcoded in the codebase.

## 11. All dependencies pinned

Every crate version, the Rust toolchain, and the Postgres major version are pinned. Floating versions are forbidden.

## 12. Repository-local knowledge only

If it isn't in the repo, the agent can't see it. Slack threads, Google Docs, and tacit knowledge are all invisible. When you learn something load-bearing, encode it here or under `docs/`.
