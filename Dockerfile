# Multi-stage Rust build. One image, all service binaries + the seed migrator.
# Selected at runtime via `command:` in docker-compose.yml.

FROM rust:1.94.1-slim-bookworm AS builder

WORKDIR /build

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Cache dependencies separately from source.
COPY rust-toolchain.toml Cargo.toml Cargo.lock* ./
COPY crates/core/Cargo.toml crates/core/
COPY crates/livepeer-indexer/Cargo.toml crates/livepeer-indexer/
COPY crates/livepeer-reorg-watcher/Cargo.toml crates/livepeer-reorg-watcher/
COPY crates/livepeer-finality-watcher/Cargo.toml crates/livepeer-finality-watcher/
COPY crates/livepeer-valuator/Cargo.toml crates/livepeer-valuator/
COPY crates/livepeer-staker/Cargo.toml crates/livepeer-staker/
COPY crates/livepeer-api/Cargo.toml crates/livepeer-api/
COPY crates/livepeer-seed-migrator/Cargo.toml crates/livepeer-seed-migrator/
COPY crates/livepeer-orchestrator/Cargo.toml crates/livepeer-orchestrator/
COPY crates/livepeer-daemon/Cargo.toml crates/livepeer-daemon/
COPY crates/livepeer-alert-bot/Cargo.toml crates/livepeer-alert-bot/

# Stub out src so cargo can build the dependency graph.
RUN mkdir -p crates/core/src && echo "" > crates/core/src/lib.rs && \
    for c in livepeer-indexer livepeer-reorg-watcher livepeer-finality-watcher \
             livepeer-valuator livepeer-staker livepeer-api livepeer-seed-migrator \
             livepeer-orchestrator livepeer-daemon livepeer-alert-bot; do \
      mkdir -p crates/$c/src && echo "fn main() {}" > crates/$c/src/main.rs; \
    done && \
    cargo build --release --workspace && \
    rm -rf crates/*/src target/release/livepeer-* target/release/deps/livepeer*

# Real build.
COPY . .
RUN cargo build --release --workspace

# ---- Runtime ----
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -m -u 1000 livepeer

COPY --from=builder /build/target/release/livepeer-indexer            /usr/local/bin/
COPY --from=builder /build/target/release/livepeer-reorg-watcher      /usr/local/bin/
COPY --from=builder /build/target/release/livepeer-finality-watcher   /usr/local/bin/
COPY --from=builder /build/target/release/livepeer-valuator           /usr/local/bin/
COPY --from=builder /build/target/release/livepeer-staker             /usr/local/bin/
COPY --from=builder /build/target/release/livepeer-api                /usr/local/bin/
COPY --from=builder /build/target/release/livepeer-seed-migrator      /usr/local/bin/
COPY --from=builder /build/target/release/livepeer-orchestrator       /usr/local/bin/
COPY --from=builder /build/target/release/livepeer-daemon             /usr/local/bin/
COPY --from=builder /build/target/release/livepeer-alert-bot          /usr/local/bin/

COPY --chown=livepeer:livepeer abi/    /opt/livepeer/abi/
COPY --chown=livepeer:livepeer config/ /opt/livepeer/config/

USER livepeer
WORKDIR /opt/livepeer

# `command:` selects the service. Default = api.
CMD ["livepeer-api"]
