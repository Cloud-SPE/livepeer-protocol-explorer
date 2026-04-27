# crypto-price-feed

Livepeer protocol event indexing & exact historical valuation system. Arbitrum One. Rust + Postgres.

> **Status: scaffold.** Skeleton only — binaries log and exit. See [AGENTS.md](AGENTS.md) for the repo map and load-bearing invariants. Authoritative spec: [docs/product-specs/v1-livepeer-indexer.md](docs/product-specs/v1-livepeer-indexer.md).

## Build

```sh
cargo build --workspace
```

Toolchain is pinned to **1.94.1** via `rust-toolchain.toml`.

## Run a service (skeleton)

```sh
cargo run --bin livepeer-indexer
cargo run --bin livepeer-valuator
cargo run --bin livepeer-api
# ... etc
```

Each service currently logs `skeleton — not implemented` and exits.

## Run the stack

```sh
cp .env.example .env
# fill in CHAINSTACK_RPC_URL, LOCAL_NITRO_URL, etc.
docker compose up --build
```

## Backfill workflow (when implemented)

```sh
# 1. seed migration (one-shot)
docker compose run --rm livepeer-seed-migrator \
  livepeer-seed-migrator --source-sqlite /seed/sqlite-4.0.db

# 2. backfill events
cargo run --bin livepeer-indexer -- backfill --from-block <N> --to-block <M>

# 3. value finalized events
cargo run --bin livepeer-valuator -- backfill --version v1_lpt_weth_twap_30min_x_chainlink_eth
```

## Documentation

- **[AGENTS.md](AGENTS.md)** — repo map + invariants.
- **[docs/product-specs/v1-livepeer-indexer.md](docs/product-specs/v1-livepeer-indexer.md)** — the spec.
- **[docs/RUNBOOK.md](docs/RUNBOOK.md)** — operations.
- **[docs/DETERMINISM.md](docs/DETERMINISM.md)** — replay-test contract.
- **[docs/design-docs/core-beliefs.md](docs/design-docs/core-beliefs.md)** — operating principles.

## License

UNLICENSED — internal.
