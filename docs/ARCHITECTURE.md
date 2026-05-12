# Architecture

A 10,000-foot view of Livepeer Protocol Explorer. This document summarizes the design, implementation, and data flow of the entire workspace.

> **This is the summary, not the canonical reference.** The authoritative description lives in [docs/product-specs/v1-livepeer-indexer.md](product-specs/v1-livepeer-indexer.md). For per-decision design records see [docs/design-docs/](design-docs/index.md). For runtime operations see [docs/RUNBOOK.md](RUNBOOK.md).

---

## System overview

Livepeer Protocol Explorer ingests every Livepeer protocol log on Arbitrum One into a Postgres database, prices every monetary event at its own block via on-chain oracles, exposes the result over a versioned HTTP API, and serves a static SPA from the same Axum process.

It exists because the Livepeer protocol emits a great deal of monetary activity (bonds, unbonds, rewards, ticket redemptions, gateway deposits) but none of those logs carry a USD amount. Valuing them after the fact through a third-party REST API at query time produces non-reproducible numbers — the same query at a different time returns a different answer.

The system solves this by making one load-bearing guarantee: **byte-deterministic replay**. Given the same RPC cache and SQLite seed, the entire pipeline produces a bit-identical output database. Three subsidiary invariants make that possible:

- **Raw events are immutable.** Once `raw_protocol_events` has a row, only a reorg-driven `block_number`/`block_hash` swap can mutate it (and that mutation is fully audited).
- **Valuations are immutable and versioned.** New pricing logic = new `valuation_version`; existing rows are never edited.
- **No external pricing APIs in the primary path.** Only on-chain Uniswap V3 TWAP × Chainlink ETH/USD plus the trusted SQLite seed.

Two execution modes share the same pipeline:

- **Live mode** — `livepeer-daemon` supervises a long-running loop that pulls logs every 12 s, advances finality every 60 s, prices new events every 60 s, refreshes derived state every 5 min, refreshes matviews every 30 s.
- **Replay mode** — `livepeer-orchestrator replay` runs the same pipeline non-interactively against a fixture (RPC cache + seed) and is the basis for the CI determinism gate.

---

## Component diagram

```mermaid
flowchart TB
  subgraph EXT["External"]
    ARB["Arbitrum One RPC<br/>(primary + secondary)"]
    L1["L1 Ethereum RPC<br/>(finality + ENS)"]
  end

  subgraph IDX["Indexing"]
    INDEXER["livepeer-indexer<br/>logs → raw_protocol_events"]
    REORG["livepeer-reorg-watcher<br/>parent-hash continuity"]
    FINALITY["livepeer-finality-watcher<br/>advances finality column"]
  end

  subgraph PRICE["Pricing"]
    VAL["livepeer-valuator<br/>Chainlink + Uniswap V3 TWAP"]
    SEED["livepeer-seed-migrator<br/>one-shot SQLite import"]
  end

  subgraph DERIVE["Derivation"]
    STAKER["livepeer-staker<br/>stake / gateway / profile tables"]
    ENRICH["livepeer-enricher<br/>ENS L1 lookups"]
  end

  subgraph AGG["Aggregation"]
    ROLLUPS["livepeer-rollups<br/>4 daily aggregate workers"]
  end

  subgraph SERVE["Serving"]
    API["livepeer-api<br/>Axum HTTP on :8080"]
    SPA["frontend-ui SPA<br/>served via FE_STATIC_DIR"]
  end

  subgraph SUPER["Supervision"]
    DAEMON["livepeer-daemon<br/>live-mode loop"]
    ORCH["livepeer-orchestrator<br/>one-shot CLI"]
    ALERT["livepeer-alert-bot<br/>Telegram + Discord"]
  end

  PG[("Postgres 17")]

  ARB --> INDEXER
  ARB --> REORG
  L1 --> FINALITY
  L1 --> ENRICH
  ARB --> VAL

  INDEXER --> PG
  REORG --> PG
  FINALITY --> PG
  VAL --> PG
  SEED --> PG
  STAKER --> PG
  ENRICH --> PG
  ROLLUPS --> PG

  DAEMON -.supervises.-> INDEXER
  DAEMON -.supervises.-> FINALITY
  DAEMON -.supervises.-> VAL
  DAEMON -.supervises.-> STAKER

  ORCH -.invokes.-> INDEXER
  ORCH -.invokes.-> VAL
  ORCH -.invokes.-> STAKER

  PG --> API
  API --> SPA

  ALERT --> PG

  classDef bin fill:#fef3c7,stroke:#92400e
  classDef ext fill:#dbeafe,stroke:#1e40af
  classDef store fill:#d1fae5,stroke:#065f46
  class INDEXER,REORG,FINALITY,VAL,SEED,STAKER,ENRICH,ROLLUPS,API,SPA,DAEMON,ORCH,ALERT bin
  class ARB,L1 ext
  class PG store
```

Everything inside the dashed boundary is shipped as a single Docker image (12 binaries built from one workspace). The daemon and orchestrator don't duplicate worker logic — they invoke the same library code that the standalone binaries do.

---

## End-to-end data flow

```mermaid
flowchart LR
  LOG["eth_getLogs<br/>chunk"]
  RAW[("raw_protocol_events<br/>immutable")]
  CACHE[("rpc_call_cache<br/>determinism layer")]
  FIN["finality<br/>= finalized"]
  SEEDTABLE[("seeded_event_prices")]
  VALUATION[("event_valuations<br/>versioned")]
  TOKEN[("token_prices_by_block")]
  STAKE[("stake_balances_by_block<br/>orch_stake_by_round<br/>delegator_registry")]
  GW[("gateway_balances_by_block<br/>gateway_flows<br/>gateway_claimants_by_block")]
  PROFILE[("orchestrator_profile<br/>broadcaster_profile<br/>(matviews)")]
  ROLLUP[("orch_payouts_daily<br/>orch_rewards_daily<br/>tickets_daily<br/>event_metrics_daily")]
  ENS[("orchestrator_ens<br/>broadcaster_ens")]
  HTTP[/"/api/v1/*"/]

  LOG -->|indexer decode| RAW
  RAW -->|finality-watcher| FIN
  FIN --> VALUATION

  RAW -. seed pass .-> SEEDTABLE
  SEEDTABLE --> VALUATION
  VALUATION -.uses.-> CACHE
  VALUATION --> TOKEN

  RAW -->|staker flow| STAKE
  RAW -->|staker gateway| GW
  STAKE --> PROFILE
  GW --> PROFILE

  RAW -->|rollup workers| ROLLUP
  VALUATION --> ROLLUP

  RAW -. enricher .-> ENS

  STAKE --> HTTP
  GW --> HTTP
  PROFILE --> HTTP
  ROLLUP --> HTTP
  VALUATION --> HTTP
  ENS --> HTTP
```

Read top-to-bottom, left-to-right: chain logs arrive, get decoded and persisted, march to finality, get priced, get aggregated, get served. Every solid arrow is a write; dashed arrows are reads / one-shot bootstraps. The `rpc_call_cache` is the sole source of replay determinism — every off-chain read by `livepeer-valuator` is mediated by it.

---

## Key sequences

### Live-mode tick

The daemon supervisor interleaves tasks on independent cadences. A typical 5-minute window looks like this:

```mermaid
sequenceDiagram
  participant Sup as livepeer-daemon
  participant Idx as indexer worker
  participant Fin as finality-watcher
  participant Val as valuator
  participant Stk as staker
  participant Mv  as matview-refresh
  participant DB  as Postgres

  Note over Sup: STAKER_INTERVAL_SECS=300, INDEXER=12, FINALITY=60, VALUATOR=60, MATVIEW=30

  loop every 12s
    Sup->>Idx: run_backfill (next chunk)
    Idx->>DB: INSERT raw_protocol_events
    Idx->>DB: UPDATE indexer_checkpoints
  end

  loop every 60s
    Sup->>Fin: advance finality
    Fin->>DB: UPDATE raw_protocol_events.finality
  end

  loop every 60s
    Sup->>Val: backfill-all
    Val->>DB: SELECT finalized unvalued candidates
    Val->>DB: INSERT event_valuations (+ token_prices_by_block)
  end

  loop every 300s
    Sup->>Stk: run_backfill + gateway-backfill + profile refresh
    Stk->>DB: INSERT stake_balances_by_block, gateway_*, orch_stake_by_round
  end

  loop every 30s
    Sup->>Mv: REFRESH MATERIALIZED VIEW orchestrator_profile, broadcaster_profile (CONCURRENTLY)
    Mv->>DB: SELECT * FROM ...
  end
```

In practice the standalone follow-mode binaries (rollups, profile-follow, tx-receipts-follow, the gateway loop wrapper) run alongside the daemon — they have their own cadence loops for the same reason. The split mirrors SPEC §15.2.

### Single-event pricing

This is the determinism story in concrete form. Every off-chain read passes through `rpc_call_cache`:

```mermaid
sequenceDiagram
  participant V as valuator
  participant C as rpc_call_cache
  participant P1 as primary RPC<br/>(Chainstack)
  participant P2 as secondary RPC
  participant DB as Postgres

  V->>DB: SELECT candidate event<br/>(finalized, unvalued, LPT-asset)
  Note over V: compute call_hash for slot0 / observe / latestRoundData

  V->>C: SELECT response_bytes WHERE call_hash = $1
  alt cache hit
    C-->>V: cached bytes
  else cache miss
    V->>P1: eth_call slot0 / observe / latestRoundData
    V->>P2: same call
    Note over V: bytes-equal cross-check
    alt mismatch
      V->>DB: INSERT rpc_divergence_failures<br/>halt the pricing job
    else match
      V->>C: INSERT response_bytes
      C-->>V: bytes
    end
  end

  Note over V: decode → compute sqrtPriceX96² ÷ 2¹⁹² → LPT/WETH<br/>× Chainlink ETH/USD → LPT/USD

  V->>DB: INSERT event_valuations<br/>(amount_native, amount_usd, pricing_chain, version)
  V->>DB: INSERT token_prices_by_block<br/>(asset, block, price_usd)
```

The bytes-equal cross-check (primary vs secondary) is what catches RPC tampering or single-provider drift. The `rpc_call_cache` row is what enables a second run on a wiped DB to produce identical valuations.

### API request

```mermaid
sequenceDiagram
  participant B as Browser SPA
  participant A as livepeer-api (Axum)
  participant DB as Postgres

  B->>A: GET /config.json
  A-->>B: { baseApiUrl, explorerTxBase, explorerAddressBase }

  B->>A: GET /api/v1/orchestrators/0x525419…
  A->>A: route → routes::profiles::orchestrator_detail
  A->>DB: SELECT * FROM orchestrator_profile WHERE address = $1<br/>(matview, refreshed every 30s)
  DB-->>A: row
  A->>DB: SELECT cuts-history, stake-history, etc.
  A-->>B: JSON shape per OpenAPI schema

  B->>A: GET /
  A-->>B: index.html from FE_STATIC_DIR (same origin)
```

Business endpoints are prefixed with `/api/v1/`; operational endpoints (`/health`, `/metrics`, `/openapi.json`, `/config.json`, `/backfills/status`) stay un-prefixed by design.

---

## Database schema

There are ~30 tables and 2 materialized views grouped into six logical clusters. Each cluster is owned by one or two crates.

### Raw events + reorg tracking

```mermaid
erDiagram
  raw_protocol_events {
    bigint id PK
    bigint chain_id
    text contract_name
    text event_name
    bigint block_number
    text block_hash
    int log_index
    text tx_hash
    timestamptz block_timestamp
    text from_address
    text to_address
    text asset
    numeric amount_normalized
    bool is_canonical
    bool is_valuable
    text finality
    jsonb raw_event
  }
  decode_failures {
    bigint id PK
    bigint chain_id
    bigint block_number
    text event_name
    bytea topic0
    text error
    timestamptz resolved_at
  }
  reorg_events {
    bigint id PK
    bigint block_number
    text old_hash
    text new_hash
    timestamptz detected_at
  }
  reorg_mutations {
    bigint id PK
    bigint reorg_event_id FK
    bigint event_id FK
    text old_block_hash
    text new_block_hash
  }
  reorg_events ||--o{ reorg_mutations : "audits"
  raw_protocol_events ||--o{ reorg_mutations : "mutates"
```

Owned by: `livepeer-indexer`, `livepeer-reorg-watcher`, `livepeer-finality-watcher`.

### Valuations + pricing cache

```mermaid
erDiagram
  event_valuations {
    bigint event_id FK
    text valuation_version PK
    text asset PK
    text pricing_method
    text source
    bigint block_number
    numeric amount_native
    numeric native_usd_price
    numeric amount_usd
    jsonb pricing_chain
    text status
  }
  valuation_attempts {
    bigint event_id FK
    text valuation_version
    text asset
    text result_status
    jsonb error_detail
    timestamptz attempted_at
  }
  seeded_event_prices {
    text tx_hash PK
    text asset PK
    numeric amount_usd
    bigint block_number
  }
  token_prices_by_block {
    text asset PK
    text quote_asset PK
    bigint block_number PK
    numeric price
    timestamptz block_timestamp
  }
  rpc_call_cache {
    text call_hash PK
    text method
    jsonb params
    bigint block_param
    bytea response_bytes
    text response_sha256
  }
  rpc_divergence_failures {
    bigint id PK
    text method
    bigint block_number
    text provider_a
    text hash_a
    text provider_b
    text hash_b
    timestamptz detected_at
  }
  event_valuations ||--o{ valuation_attempts : "audited by"
  event_valuations ||--o{ token_prices_by_block : "produces"
```

Owned by: `livepeer-valuator`, `livepeer-seed-migrator`. The `rpc_call_cache` is the determinism layer — see [DETERMINISM.md](DETERMINISM.md).

### Derived state — stake + gateway

```mermaid
erDiagram
  stake_balances_by_block {
    text delegator_address PK
    bigint block_number PK
    text transcoder_address
    numeric bonded_amount
    numeric pending_stake
    numeric pending_fees
  }
  orch_stake_by_round {
    text address PK
    bigint round PK
    bigint block_number
    numeric total_stake
    numeric self_stake
    numeric delegated_stake
    int delegator_count
    numeric latest_reward_cut_percent
    numeric latest_fee_share_percent
    numeric latest_fee_cut_percent
  }
  delegator_registry {
    text delegator_address PK
    bigint first_bond_block
    bigint last_seen_block
    bool is_active
  }
  gateway_balances_by_block {
    text gateway_address PK
    bigint block_number PK
    numeric deposit
    numeric reserve_funds_remaining
    bool unlock_in_progress
  }
  gateway_flows {
    bigint event_id FK
    text gateway_address
    bigint block_number
    text flow_type
    numeric amount
  }
  gateway_claimants_by_block {
    text gateway_address PK
    bigint block_number PK
    text claimant_address PK
    numeric claimed_in_round
  }
  tx_receipts {
    text tx_hash PK
    bigint block_number
    numeric gas_used
    numeric effective_gas_price
  }
```

Owned by: `livepeer-staker` (and `tx_receipts` via `tx-receipts-follow`).

### Profiles (materialized views)

```mermaid
erDiagram
  orchestrator_profile {
    text address PK
    bigint chain_id
    numeric total_stake
    bigint delegator_count
    bool is_active
    numeric latest_reward_cut_percent
    numeric latest_fee_share_percent
    numeric latest_fee_cut_percent
    timestamptz refreshed_at
  }
  broadcaster_profile {
    text address PK
    bigint chain_id
    numeric current_deposit
    numeric current_reserve
    bool unlock_in_progress
    timestamptz refreshed_at
  }
  orch_stake_by_round ||..|| orchestrator_profile : "feeds"
  gateway_balances_by_block ||..|| broadcaster_profile : "feeds"
```

Refreshed `CONCURRENTLY` every 30 s by the daemon (`MATVIEW_REFRESH_INTERVAL_SECS`).

### Rollups

```mermaid
erDiagram
  orch_payouts_daily {
    text orchestrator_address PK
    date day PK
    text valuation_version PK
    bigint ticket_count
    numeric sum_face_value_native
    numeric sum_face_value_usd
    numeric sum_commission_native
    numeric sum_commission_usd
    numeric sum_delegators_share_usd
    int distinct_gateways
  }
  orch_rewards_daily {
    text orchestrator_address PK
    date day PK
    text valuation_version PK
    bigint reward_count
    numeric sum_amount_native
    numeric sum_amount_usd
  }
  tickets_daily {
    date day PK
    text broadcaster_kind PK
    bigint ticket_count
    int distinct_gateways
    int distinct_orchestrators
  }
  event_metrics_daily {
    text contract_name PK
    text event_name PK
    text asset PK
    text valuation_version PK
    date day PK
    bigint count
    numeric sum_amount_native
    numeric sum_amount_usd
  }
```

Owned by: `livepeer-rollups`. Each table has its own checkpoint in `indexer_checkpoints` (e.g., `rollup_orch_payouts_daily`).

### Enrichment + operational

```mermaid
erDiagram
  orchestrator_ens {
    text address PK
    text ens_name
    text avatar_url
    timestamptz resolved_at
  }
  broadcaster_ens {
    text address PK
    text ens_name
    text avatar_url
    timestamptz resolved_at
  }
  broadcaster_classifications {
    text address PK
    text classification
    text source
  }
  name_avatar_overrides {
    text address PK
    text override_name
    text override_avatar_url
  }
  indexer_checkpoints {
    text name PK
    bigint last_processed_block
    timestamptz updated_at
  }
  contract_abi_registry {
    text contract_name PK
    text proxy_address
    text target_address
    bigint from_block
    text abi_path
    text abi_hash
    bool strict_decode
  }
  discord_payouts_sent {
    bigint id PK
    text orchestrator_address
    date day
    timestamptz sent_at
  }
  discord_summaries_sent {
    bigint id PK
    text period
    date period_end
    timestamptz sent_at
  }
```

Owned by: `livepeer-enricher` (ENS), `livepeer-indexer` (checkpoints), `livepeer-alert-bot` (discord_*_sent), `livepeer-orchestrator` (contract_abi_registry boot).

> The schema groups above show PK/FK shape only — for canonical column types, constraints, and indexes see the migrations in [`migrations/`](../migrations/) and SPEC §11.

---

## Configuration model

Four cascading layers. None of them depends on the others at build time; the binary just reads what's set.

| Layer | File / source | Committed? | Purpose |
|-------|---------------|------------|---------|
| Static chain config | `config/arbitrum.yaml` | ✓ | Contract addresses, ABI paths, pricing parameters (pool, Chainlink feed, sequencer feed), retry/concurrency policy |
| Env-specific config | `config/env/{dev,staging,prod}.yaml` | ✓ | RPC concurrency, batch sizes, alert toggles per environment |
| Secrets | `.env` | ✗ (gitignored) | `CHAINSTACK_RPC_URL`, `POSTGRES_PASSWORD`, `TELEGRAM_BOT_TOKEN`, `L1_RPC_URL`, etc. Template at `.env.example` |
| Runtime SPA config | `FE_*` env vars on the `livepeer-api` process | ✗ (env) | Surfaced as `GET /config.json` to the browser. See `crates/livepeer-api/src/routes/operational.rs::frontend_config` for the env-var → JSON-key mapping |

The CLI binaries take `--static-config` and `--env-config` paths so a single binary can run dev or prod by pointing at different YAML files. The `.env` is loaded by docker-compose (`env_file: .env`) or sourced by the operator scripts.

---

## Determinism contract

The single most important invariant. Restated tightly:

1. Drop the entire Postgres DB.
2. Replay from a fixture (`tests/fixtures/case-{a,b}/`) consisting of an `rpc_call_cache` CSV dump, the seed SQLite, and a `fixture.env` with `FROM_BLOCK` / `TO_BLOCK`.
3. Hash every output table per SPEC §12.4.
4. The hashes must match `expected_hashes.json` byte-for-byte.

Mechanism: every off-chain read in `livepeer-valuator` (and in any path that uses `livepeer_core::rpc::cross_check`) is mediated by `rpc_call_cache`. Cache miss → primary RPC + secondary RPC cross-check → bytes-equal → store the bytes. Cache hit → return the stored bytes. With the cache pre-populated from the fixture, no network call is made, so output is purely a deterministic function of `(raw_protocol_events ∪ seeded_event_prices ∪ rpc_call_cache)`.

Enforced by the `Determinism` GitHub workflow (manual / on-demand). Full contract in [DETERMINISM.md](DETERMINISM.md).

If you intentionally change the pricing logic, **bump `valuation_version`** and regenerate `expected_hashes.json` via `scripts/compute-determinism-hashes.sh` — never overwrite an existing version's hashes.

---

## Deployment topology

SPEC §15.2 — single host, single Docker network, single Postgres.

```
                 ┌──────────────────────────────────────────────┐
                 │  Single host (Ubuntu)                        │
                 │                                              │
                 │  ┌────────────────────────────────────────┐  │
                 │  │ Docker network: livepeer               │  │
                 │  │                                        │  │
                 │  │ ┌──────────────────┐  ┌──────────────┐ │  │
                 │  │ │ postgres:17      │  │ livepeer-api │ │  │
                 │  │ │ (volume:         │  │ :8080        │ │  │
                 │  │ │  pgdata)         │  └──────────────┘ │  │
                 │  │ └──────────────────┘  ┌──────────────┐ │  │
                 │  │                       │ daemon       │ │  │
                 │  │                       └──────────────┘ │  │
                 │  │                       ┌──────────────┐ │  │
                 │  │                       │ 4× rollups   │ │  │
                 │  │                       └──────────────┘ │  │
                 │  │                       ┌──────────────┐ │  │
                 │  │                       │ staker x3    │ │  │
                 │  │                       └──────────────┘ │  │
                 │  │                       ┌──────────────┐ │  │
                 │  │                       │ enricher     │ │  │
                 │  │                       └──────────────┘ │  │
                 │  │                       ┌──────────────┐ │  │
                 │  │                       │ alert-bot    │ │  │
                 │  │                       └──────────────┘ │  │
                 │  └────────────────────────────────────────┘  │
                 │                                              │
                 │  Cloudflare Tunnel ──► livepeer-api:8080     │
                 └──────────────────────────────────────────────┘
                                       ▲
                                       │
                              ┌────────┴────────┐
                              │ External: Prom  │  (separate host)
                              │ + Grafana       │  (SPEC §15.1)
                              └─────────────────┘
```

All 12 binaries build into one Docker image (`tztcloud/livepeer-protocol-explorer:latest`). The runtime image is slim; each compose service overrides only the `command:` to pick which binary runs. See [DEPLOYMENT.md](DEPLOYMENT.md) for the full prod compose, and [CLOUDFLARE_TUNNEL.md](CLOUDFLARE_TUNNEL.md) for the external-access shape.

---

## CI / workflows

| Workflow | Trigger | Steps | Purpose |
|----------|---------|-------|---------|
| `ci.yml` | push to `main`, every PR | `fmt` / `clippy` / `build` / `test`, all under `RUSTFLAGS="-D warnings"` | Lint + build + unit-test gate. Must be green for any merge. |
| `determinism.yml` | `workflow_dispatch` only | Spin up Postgres 17, run `scripts/run-determinism-replay.sh` per fixture, diff `expected_hashes.json` | Manual enforcement of the SPEC §12.4 replay invariant. Heavy (~15 min); run before any pricing-logic change or fixture update. |

Five integration tests in `livepeer-api` (route tests against a live Postgres) are tagged `#[ignore]` because they require `DATABASE_URL` and a migrated DB. Plain `cargo test --workspace` skips them; run them locally with:

```sh
docker compose up -d postgres
target/release/livepeer-orchestrator migrate-only
cargo test -p livepeer-api -- --ignored
```

---

## Operating modes summary

| Mode | Entry point | When to use |
|------|-------------|-------------|
| **Cold start / bootstrap** | `livepeer-orchestrator bootstrap` | First-time DB setup. Migrates schema, seeds ABI registry, runs full indexer + valuator + staker backfill for the requested range. |
| **Live mode** | `livepeer-daemon follow` | Steady-state operation. Single supervisor process keeps every worker on its cadence. |
| **Live mode (decomposed)** | `scripts/resume-catchup-all.sh` | Same as above but each worker is its own OS process (4× rollups, profile-follow, tx-receipts-follow, gateway loop, enricher follow, daemon follow, api). Easier to introspect / restart individually. |
| **Replay** | `livepeer-orchestrator replay --source-sqlite … --from-block N --to-block M` | Determinism check. Drops state, replays from RPC cache + seed, computes hashes. Used by CI. |
| **One-off subcommands** | `livepeer-orchestrator backfill-cuts`, `livepeer-valuator backfill-lpt-onchain`, … | Ad-hoc maintenance — e.g., recompute a column from chain truth, or re-price one asset class without touching the rest. |

---

## Cross-references

- **[docs/product-specs/v1-livepeer-indexer.md](product-specs/v1-livepeer-indexer.md)** — the canonical encyclopedia. Authoritative for table definitions, valuation algorithms, error semantics.
- **[AGENTS.md](../AGENTS.md)** — the repo map for contributors and agents. Restates the load-bearing invariants.
- **[docs/RUNBOOK.md](RUNBOOK.md)** — operational procedures: restart loops, common troubleshooting, alert escalation.
- **[docs/DEPLOYMENT.md](DEPLOYMENT.md)** — prod compose shape, image pull/push, secret rotation.
- **[docs/DETERMINISM.md](DETERMINISM.md)** — the replay-test contract in full.
- **[docs/POSTGRES_MAINTENANCE.md](POSTGRES_MAINTENANCE.md)** — DB upgrades, vacuum policy, backup/restore.
- **[docs/CLOUDFLARE_TUNNEL.md](CLOUDFLARE_TUNNEL.md)** — public-access tunnel topology.
- **[docs/design-docs/index.md](design-docs/index.md)** — per-decision design records (TOC).
- **[docs/exec-plans/tech-debt-tracker.md](exec-plans/tech-debt-tracker.md)** — known shortcuts + deferred work.
