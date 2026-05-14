# Livepeer Protocol Explorer

[![CI](https://github.com/Cloud-SPE/livepeer-protocol-explorer/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/Cloud-SPE/livepeer-protocol-explorer/actions/workflows/ci.yml)
[![Determinism](https://github.com/Cloud-SPE/livepeer-protocol-explorer/actions/workflows/determinism.yml/badge.svg)](https://github.com/Cloud-SPE/livepeer-protocol-explorer/actions/workflows/determinism.yml)
![Rust](https://img.shields.io/badge/rust-1.94.1-orange?logo=rust)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Powered by Cloud SPE](https://img.shields.io/badge/Powered%20by-Cloud%20SPE-00d188)](https://www.livepeer.cloud)

A Rust + Postgres system that indexes Livepeer protocol events on **Arbitrum One**, prices every monetary event at block-level precision via **Uniswap V3 TWAP × Chainlink**, and exposes the result over an HTTP API plus a single-page web UI.

The load-bearing correctness guarantee is **byte-deterministic replay**: a database wipe followed by a re-run from cached RPC inputs and the seeded SQLite produces an identical output database (SPEC §1.4 / §12.4).

Built by [Cloud SPE](https://www.livepeer.cloud).

---

## What it is

The Livepeer protocol emits a lot of monetary events on-chain — bonds, unbonds, rewards, ticket redemptions, gateway deposits. None of them carry a USD value at the point of emission, and pricing them after the fact non-deterministically (e.g., via a third-party REST API at query time) makes historical accounting unreliable.

This system solves that by:

1. **Indexing** every Livepeer protocol log on Arbitrum One into an immutable `raw_protocol_events` table.
2. **Pricing** each finalized monetary event at its own block, using on-chain oracles only (Uniswap V3 30-minute TWAP for LPT, Chainlink ETH/USD aggregator for the L1 reference rate, Chainlink L2 sequencer-uptime feed as a liveness gate).
3. **Caching** every RPC call into `rpc_call_cache` so the entire pipeline is replayable bit-for-bit from cold storage.
4. **Aggregating** the priced rows into daily rollups for payouts, rewards, tickets, and event metrics.
5. **Serving** the result over a versioned HTTP API (`/api/v1/...`) and a Lit-based SPA bundled into the same Axum process.

---

## How it works

```
                     Arbitrum One RPC (Chainstack + secondary)
                                       │
                                       ▼
┌───────────────────────────────────────────────────────────────────────┐
│  livepeer-indexer           pulls logs in chunks, decodes via vendored │
│                             ABIs, writes raw_protocol_events (append) │
│  livepeer-reorg-watcher     validates parent-hash chain continuity    │
│  livepeer-finality-watcher  advances finality field on L1 batch posts │
└───────────────────────────────────────────────────────────────────────┘
                                       │
                                       ▼
┌───────────────────────────────────────────────────────────────────────┐
│  livepeer-valuator          prices finalized events via               │
│                             Chainlink + Uniswap V3 TWAP →             │
│                             event_valuations (immutable, versioned)   │
│  livepeer-staker            stake_balances_by_block, gateway_*,       │
│                             orchestrator/broadcaster profile          │
└───────────────────────────────────────────────────────────────────────┘
                                       │
                                       ▼
┌───────────────────────────────────────────────────────────────────────┐
│  livepeer-rollups           orch_payouts_daily, orch_rewards_daily,   │
│                             tickets_daily, event_metrics_daily        │
│  livepeer-enricher          ENS name + avatar resolution (L1 lookup)  │
└───────────────────────────────────────────────────────────────────────┘
                                       │
                                       ▼
┌───────────────────────────────────────────────────────────────────────┐
│  livepeer-api               Axum HTTP server on :8080                  │
│                             • /api/v1/* business endpoints            │
│                             • /health /metrics /openapi.json          │
│                             • Serves the SPA bundle as same-origin    │
└───────────────────────────────────────────────────────────────────────┘
```

`livepeer-daemon` supervises the live-mode loop in production; `livepeer-orchestrator` is the one-shot CLI for replays, bootstraps, and migration-only runs. `livepeer-seed-migrator` is a one-time SQLite-to-Postgres import for trusted historical valuations (SPEC §8.1 seed pass).

---

## Quick start

Requires Docker (or a local Postgres 17), Rust 1.94.1 (`rustup` will install automatically via the toolchain pin), Node 22+, and a Chainstack Arbitrum One RPC URL.

```sh
# 1. Configure
cp .env.example .env       # then fill in CHAINSTACK_RPC_URL, etc.

# 2. Start Postgres
docker compose up -d postgres

# 3. Build everything (Rust + frontend)
cargo build --release --workspace
(cd frontend-ui && npm install && npm run build)

# 4. Apply migrations
target/release/livepeer-orchestrator migrate-only

# 5. Bring up the worker fleet (HTTP API on :8080, daemon, rollups, staker, enricher)
bash scripts/resume-catchup-all.sh

# 6. Smoke-check
curl -s http://127.0.0.1:8080/health   # → ok
open http://127.0.0.1:8080/            # → SPA dashboard
```

Stop everything with `bash scripts/stop-all.sh` (workers) and `docker compose down` (Postgres — preserves the `postgres_data` volume).

---

## Build, configure, run

### Toolchain

Pinned in `rust-toolchain.toml` to **Rust 1.94.1**. CI matches via `dtolnay/rust-toolchain@1.94.1`. Dependencies are pinned exactly (`=x.y.z`) — bumps go through PR review.

### Configuration layers

1. **`.env`** at the repo root — secrets (`CHAINSTACK_RPC_URL`, `POSTGRES_*`, alert tokens). Gitignored. Use `.env.example` as a template.
2. **`config/arbitrum.yaml`** — static, chain-level config (contract addresses, ABI paths, pricing parameters). Committed.
3. **`config/env/{dev,staging,prod}.yaml`** — environment overrides (RPC concurrency, batch sizes, etc.). Committed.
4. **`FE_STATIC_DIR`** env var (defaults to `/opt/livepeer/frontend-ui/dist` for the Docker image) — points the API at the SPA bundle. The local-dev script `scripts/resume-catchup-all.sh` sets it to `$PWD/frontend-ui/dist` automatically.

### Binaries

| Binary | Role |
|--------|------|
| `livepeer-indexer` | Pull logs for one contract over a block range and decode them. |
| `livepeer-reorg-watcher` | Validate parent-hash continuity and route reorg mutations. |
| `livepeer-finality-watcher` | Advance the `finality` column when L1 batches post. |
| `livepeer-valuator` | Price finalized events. Subcommands: `backfill-from-seed`, `backfill-eth-onchain`, `backfill-lpt-onchain`, `backfill-multi-asset`, `backfill-all`, `backfill-eth-prices`. |
| `livepeer-staker` | Compute stake/gateway/profile derived tables. Subcommands: `backfill`, `gateway-backfill`, `profile-follow`, `tx-receipts-follow`, `refresh-pending`. |
| `livepeer-rollups` | Daily aggregate workers. Subcommands: `orch-payouts-daily`, `orch-rewards-daily`, `tickets-daily`, `event-metrics-daily` (each accepts `--follow` for live mode). |
| `livepeer-enricher` | ENS L1 name + avatar resolver for orchestrators and gateways. |
| `livepeer-daemon` | Live-mode supervisor — runs all the workers on internal cadences. |
| `livepeer-orchestrator` | One-shot CLI: `migrate-only`, `replay`, `bootstrap`, `backfill-cuts`. |
| `livepeer-seed-migrator` | One-time SQLite → `seeded_event_prices` import. |
| `livepeer-api` | Axum HTTP server. Serves `/api/v1/*`, operational endpoints, and the SPA. |
| `livepeer-alert-bot` | Telegram + Discord alerting on indexer-health metrics. |

### API surface

- **`:8080/`** — SPA (Livepeer Protocol Explorer dashboard).
- **`:8080/api/v1/*`** — business endpoints (events, valuations, rollups, gateways, orchestrators, rounds, governance, network stats). Full schema: `/openapi.json`.
- **`:8080/health`**, **`:8080/metrics`**, **`:8080/config.json`**, **`:8080/backfills/status`** — operational, unversioned.

---

## Repository conventions

Restated from [AGENTS.md](AGENTS.md) — the authoritative repo map. Load-bearing invariants any change must respect:

1. **Byte-deterministic replay.** Drop the DB, replay from `rpc_call_cache` + seed → identical output. The Determinism CI workflow enforces this.
2. **Raw events are immutable.** Only permitted mutation is a reorg-induced `block_number` / `block_hash` swap, fully audited.
3. **Valuations are immutable and versioned.** New pricing logic = new `valuation_version`; never edit existing rows.
4. **No external pricing APIs in the primary path.** Only on-chain TWAP/Chainlink and the trusted SQLite seed.
5. **Migrations are forward-only and immutable once merged.**
6. **All deps pinned exactly.** Bumps go through PR review.
7. **Single-instance workers.** No claim mechanism in v1.

### CI gates

The `ci.yml` workflow runs four jobs on every push to `main` and every PR, all with `RUSTFLAGS="-D warnings"`:

| Job | Command |
|-----|---------|
| fmt | `cargo fmt --all --check` |
| clippy | `cargo clippy --workspace --all-targets -- -D warnings` |
| build | `cargo build --workspace --all-targets` |
| test | `cargo test --workspace` |

The `determinism.yml` workflow is **manual / on-demand only** (`workflow_dispatch`) — it spins up Postgres, replays each fixture in `tests/fixtures/`, and diffs the SHA-256 hash table against `expected_hashes.json`. Run it from the GitHub Actions UI or:

```sh
gh workflow run Determinism --ref main
```

### Tests

The `cargo test --workspace` suite excludes integration tests that require a live Postgres. Those are tagged `#[ignore]` with a reason string. To run them locally:

```sh
docker compose up -d postgres
target/release/livepeer-orchestrator migrate-only
cargo test -p livepeer-api -- --ignored
```

Or for the full sweep (regular + integration):

```sh
cargo test --workspace -- --include-ignored
```

### Plans, decisions, and tech debt

- **Plans** go in `docs/exec-plans/active/` (move to `completed/` when done).
- **Decisions** go in `docs/design-docs/` with status (`draft` / `accepted` / `superseded`) and a `verified` date.
- **Tech debt** is tracked in `docs/exec-plans/tech-debt-tracker.md`.
- **Spec changes** require explicit approval and a version bump (SPEC v1.0 → v1.1).

---

## Repository layout

```
.
├── crates/                         13 Cargo workspace members (see AGENTS.md)
├── abi/                            vendored Livepeer ABI JSON (hash-verified at boot)
├── config/                         static + env-overlay YAML config
├── migrations/                     sqlx-cli forward-only migrations
├── frontend-ui/                    Lit + TypeScript SPA (Vite)
├── tests/fixtures/                 determinism fixtures (rpc_cache, seed, hashes)
├── scripts/                        operator scripts (resume-catchup-all.sh, stop-all.sh, …)
├── docs/                           spec, design docs, runbook, deployment, exec plans
├── ops/                            Prometheus + Alertmanager configs
├── Dockerfile                      multi-stage; one image, all binaries
├── docker-compose.yml              local dev — single-host shape
├── docker-compose.prod.yml         production — pulls tztcloud/livepeer-protocol-explorer
└── rust-toolchain.toml             pinned 1.94.1
```

See [AGENTS.md](AGENTS.md) for the canonical, per-crate map.

---

## Documentation

| File | Purpose |
|------|---------|
| [AGENTS.md](AGENTS.md) | Repo map + load-bearing invariants. The canonical entry point for contributors and agents. |
| [docs/product-specs/v1-livepeer-indexer.md](docs/product-specs/v1-livepeer-indexer.md) | The authoritative spec (the encyclopedia). |
| [docs/RUNBOOK.md](docs/RUNBOOK.md) | Operational procedures (SPEC §19). |
| [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) | Production deployment shape. |
| [docs/DETERMINISM.md](docs/DETERMINISM.md) | Determinism replay-test contract (SPEC §12.4). |
| [docs/POSTGRES_MAINTENANCE.md](docs/POSTGRES_MAINTENANCE.md) | DB upgrade + maintenance procedures. |
| [docs/CLOUDFLARE_TUNNEL.md](docs/CLOUDFLARE_TUNNEL.md) | External-access tunnel setup. |
| [docs/design-docs/index.md](docs/design-docs/index.md) | Per-decision design records. |
| [docs/design-docs/core-beliefs.md](docs/design-docs/core-beliefs.md) | Load-bearing operating principles. |
| [docs/exec-plans/tech-debt-tracker.md](docs/exec-plans/tech-debt-tracker.md) | Known shortcuts + deferred work. |

---

## License

[MIT](LICENSE) — Copyright (c) 2026 Cloud SPE.
