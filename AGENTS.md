# AGENTS.md

This file is a **map**, not an encyclopedia. It points to the deeper sources of truth and restates the load-bearing invariants any change must respect.

The product spec is the canonical reference. Anything below that disagrees with the spec is a bug in this file.

## What this is

A Rust + Postgres system that indexes Livepeer protocol events on Arbitrum One, prices every monetary event at block-level precision via Uniswap V3 TWAP × Chainlink, and exposes the result over HTTP. The load-bearing correctness guarantee is **byte-deterministic replay** — a database wipe followed by a re-run from cached RPC inputs and the seeded SQLite produces an identical output database.

Authoritative spec: [docs/product-specs/v1-livepeer-indexer.md](docs/product-specs/v1-livepeer-indexer.md).

## Repo map

```
.
├── crates/
│   ├── core/                       shared lib (config, db, rpc, abi, types, metrics)
│   ├── livepeer-indexer/           bin — pulls logs, decodes, writes raw_protocol_events
│   ├── livepeer-reorg-watcher/     bin — validates parent-hash chain continuity
│   ├── livepeer-finality-watcher/  bin — advances finality field on L1 batch posting
│   ├── livepeer-valuator/          bin — prices finalized events into event_valuations
│   ├── livepeer-staker/            bin — computes stake_balances_by_block + gateway_* + profile
│   ├── livepeer-rollups/           bin — daily aggregates (payouts / rewards / tickets / event_metrics)
│   ├── livepeer-enricher/          bin — ENS L1 name + avatar resolver
│   ├── livepeer-daemon/            bin — live-mode supervisor; runs all workers on cadences
│   ├── livepeer-orchestrator/      bin — one-shot CLI (migrate-only / replay / bootstrap / backfill-cuts)
│   ├── livepeer-api/               bin — Axum HTTP API + serves the SPA bundle
│   ├── livepeer-alert-bot/         bin — Telegram + Discord alerting on indexer-health metrics
│   ├── livepeer-seed-migrator/     bin — one-shot SQLite → seeded_event_prices import
│   └── livepeer-mcp-diag/          bin — read-only production diagnostics MCP server (SELECT-only diag_ro role + GET-only docker proxy)
├── abi/                            vendored ABI JSON. Hashes verified at boot (SPEC §5.5).
├── config/
│   ├── arbitrum.yaml               static config — addresses, pricing, retry policy
│   └── env/{dev,staging,prod}.yaml environment-specific config (no secrets)
├── migrations/                     sqlx-cli migrations (SPEC §11)
├── tests/fixtures/                 determinism fixtures (rpc_cache, seed sqlite, expected hashes)
├── frontend-ui/                    Lit + TypeScript SPA bundled into livepeer-api via FE_STATIC_DIR
├── docs/
│   ├── product-specs/              the spec (encyclopedia)
│   ├── design-docs/                per-decision design records
│   │   ├── index.md                ← table of contents
│   │   └── core-beliefs.md         ← load-bearing operating principles
│   ├── exec-plans/                 first-class execution plans (active / completed)
│   │   └── tech-debt-tracker.md    known shortcuts and deferred work
│   ├── generated/db-schema.md      generated from migrations (TODO)
│   ├── references/                 external references
│   ├── RUNBOOK.md                  operational procedures (SPEC §19)
│   ├── DEPLOYMENT.md               production deployment shape
│   └── DETERMINISM.md              determinism test contract (SPEC §12.4)
├── ops/                            Prometheus + Alertmanager configs
├── scripts/                        operator scripts (resume-catchup-all, stop-all, ...)
├── .github/workflows/              ci.yml (auto) + determinism.yml (workflow_dispatch only)
├── Dockerfile                      multi-stage; one image, all 13 binaries
├── docker-compose.yml              SPEC §15.2 single-host deployment (dev)
├── docker-compose.prod.yml         production compose pulling tztcloud/livepeer-protocol-explorer
└── rust-toolchain.toml             pinned to 1.94.1
```

## Load-bearing invariants

If you are about to change code, these must still hold afterward. Full statement: [docs/design-docs/core-beliefs.md](docs/design-docs/core-beliefs.md).

1. **Byte-deterministic replay.** Drop the DB, replay from `rpc_call_cache` + seed → identical output. CI enforces (SPEC §12.4). If your change breaks this, either reframe it or bump `valuation_version`.
2. **Raw events are immutable.** The single permitted mutation is reorg-induced `block_number`/`block_hash` update, fully audited.
3. **Valuations are immutable and versioned.** `event_valuations` rows are write-once per `(event_id, valuation_version, asset)`. New pricing logic = new version.
4. **`cast call ... --block N` reproduces every price.** If a USD valuation can't be reproduced this way, the pricing logic has a bug.
5. **No external pricing APIs in the primary path.** CoinGecko etc. are forbidden. Only on-chain TWAP/Chainlink and the trusted SQLite seed.
6. **Strict-decode halts on critical events.** `Bond`/`Unbond`/`Rebond`/`WithdrawStake`/`Reward`/`EarningsClaimed`/`WinningTicketRedeemed`/`WinningTicketTransfer`/`Transfer` decode failures halt the indexer (SPEC §6.2). All others dead-letter.
7. **Migrations are immutable once merged.** Forward-only. No production downs. Destructive changes require `--allow-destructive`.
8. **Idempotent writes.** Every persistence path has a defined conflict key with `ON CONFLICT DO NOTHING` (or `GREATEST` for monotonic counters).
9. **Single-instance workers.** No claim mechanism in v1.
10. **Listen on proxy, decode via target.** Controller is the only hardcoded address; targets resolved at boot.
11. **All deps pinned exactly.** Bumps go through PR review.

## How to work in this repo

- **Plans go in `docs/exec-plans/active/`.** One markdown file per plan, with a progress log. When done, move to `docs/exec-plans/completed/`.
- **Decisions go in `docs/design-docs/`** with a status (`draft` / `accepted` / `superseded`) and a `verified` date.
- **Repository-local knowledge only.** If it's not in the repo, the agent can't see it. Encode it here, in design-docs, or in the spec.
- **Update the tech-debt tracker** (`docs/exec-plans/tech-debt-tracker.md`) when you take a shortcut.
- **Spec changes require explicit approval and a version bump** (SPEC v1.0 → v1.1).

## Quick start

```sh
# verify toolchain pin
cat rust-toolchain.toml

# compile everything
cargo build --workspace

# run a service skeleton
cargo run --bin livepeer-api

# format / lint
cargo fmt --all
cargo clippy --workspace --all-targets

# bring up the stack (postgres + all services)
docker compose up --build

# run the seed migrator one-shot
docker compose run --rm livepeer-seed-migrator livepeer-seed-migrator --source-sqlite /seed/sqlite-4.0.db
```

## Open data items (SPEC §22)

The spec has 10 open data items (Q-OD-1 … Q-OD-10) that block real implementation but not the scaffold. They are tagged `TODO(Q-OD-N)` in code/config and listed in `docs/exec-plans/tech-debt-tracker.md`. Resolve them during implementation kickoff before the first real backfill.

## Scope

- **In scope (v1):** SPEC §1.1.
- **Out of scope (v1):** SPEC §20. Don't drift.
- **v2 roadmap:** SPEC §21. Do not pre-implement.

## When in doubt

Read the spec. If the spec is silent, write a design doc proposing the answer and link it from `docs/design-docs/index.md`.
