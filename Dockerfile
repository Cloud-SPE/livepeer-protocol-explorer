# Multi-stage build:
#   - rust-builder: compiles every workspace binary
#   - fe-builder:   produces the static frontend bundle (Lit/Vite SPA)
#   - runtime:      Debian slim with just the binaries, ABI, config, and FE
# `command:` in docker-compose.yml selects which binary to run.

FROM rust:1.94.1-slim-bookworm AS rust-builder

WORKDIR /build

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

COPY . .
RUN cargo build --release --workspace

# ---- Frontend bundle ----
FROM node:22-bookworm-slim AS fe-builder

WORKDIR /fe

# Cache `npm ci` on the lockfile alone — only re-run when deps change.
COPY frontend-ui/package.json frontend-ui/package-lock.json ./
RUN npm ci --no-audit --no-fund

COPY frontend-ui/ ./
RUN npm run build

# ---- Runtime ----
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -m -u 1000 livepeer

COPY --from=rust-builder /build/target/release/livepeer-indexer            /usr/local/bin/
COPY --from=rust-builder /build/target/release/livepeer-reorg-watcher      /usr/local/bin/
COPY --from=rust-builder /build/target/release/livepeer-finality-watcher   /usr/local/bin/
COPY --from=rust-builder /build/target/release/livepeer-valuator           /usr/local/bin/
COPY --from=rust-builder /build/target/release/livepeer-staker             /usr/local/bin/
COPY --from=rust-builder /build/target/release/livepeer-enricher          /usr/local/bin/
COPY --from=rust-builder /build/target/release/livepeer-rollups           /usr/local/bin/
COPY --from=rust-builder /build/target/release/livepeer-api                /usr/local/bin/
COPY --from=rust-builder /build/target/release/livepeer-seed-migrator      /usr/local/bin/
COPY --from=rust-builder /build/target/release/livepeer-orchestrator       /usr/local/bin/
COPY --from=rust-builder /build/target/release/livepeer-daemon             /usr/local/bin/
COPY --from=rust-builder /build/target/release/livepeer-alert-bot          /usr/local/bin/

COPY --chown=livepeer:livepeer abi/        /opt/livepeer/abi/
COPY --chown=livepeer:livepeer config/     /opt/livepeer/config/
# `livepeer-orchestrator migrate-only` reads SQL files from this directory.
# Resolved by `resolve_migrations_path()` in livepeer-orchestrator/src/main.rs.
COPY --chown=livepeer:livepeer migrations/ /opt/livepeer/migrations/

# Static frontend bundle — served by livepeer-api as a fallback for any
# path not matched by an API route. Default location matches FE_STATIC_DIR
# in `crates/livepeer-api/src/lib.rs::static_frontend_service`.
COPY --from=fe-builder --chown=livepeer:livepeer /fe/dist /opt/livepeer/frontend-ui/dist

USER livepeer
WORKDIR /opt/livepeer

# `command:` selects the service. Default = api.
CMD ["livepeer-api"]
