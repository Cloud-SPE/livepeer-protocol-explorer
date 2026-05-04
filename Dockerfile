# Multi-stage Rust build. One image, all service binaries + the seed migrator.
# Selected at runtime via `command:` in docker-compose.yml.

FROM rust:1.94.1-slim-bookworm AS builder

WORKDIR /build

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

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
