#!/usr/bin/env bash
set -euo pipefail

ROOT=/home/mazup/git-repos/crypto-price-feed
cd "$ROOT"

if [[ -z "${DATABASE_URL:-}" ]]; then
  echo "DATABASE_URL is required" >&2
  exit 1
fi

export CHAINSTACK_RPC_URL="${CHAINSTACK_RPC_URL:-http://127.0.0.1:8545}"
export SECONDARY_RPC_URL="${SECONDARY_RPC_URL:-http://127.0.0.1:8546}"
export L1_RPC_URL="${L1_RPC_URL:-http://127.0.0.1:9545}"

ROOT_FIXTURE_DIR="${1:-$ROOT/tests/fixtures}"
if [[ ! -d "$ROOT_FIXTURE_DIR" ]]; then
  echo "fixture dir not found: $ROOT_FIXTURE_DIR" >&2
  exit 1
fi

declare -a CASE_DIRS=()
if [[ -f "$ROOT_FIXTURE_DIR/fixture.env" ]]; then
  CASE_DIRS+=("$ROOT_FIXTURE_DIR")
else
  while IFS= read -r dir; do
    CASE_DIRS+=("$dir")
  done < <(find "$ROOT_FIXTURE_DIR" -mindepth 1 -maxdepth 1 -type d | sort)
fi

if [[ "${#CASE_DIRS[@]}" -eq 0 ]]; then
  echo "no fixture cases found under $ROOT_FIXTURE_DIR" >&2
  exit 1
fi

for FIXTURE_DIR in "${CASE_DIRS[@]}"; do
  if [[ ! -f "$FIXTURE_DIR/fixture.env" ]]; then
    continue
  fi
  source "$FIXTURE_DIR/fixture.env"

  cargo run --quiet --bin livepeer-orchestrator -- --env-config config/env/dev.yaml migrate-only
  bash scripts/load-determinism-fixture.sh "$FIXTURE_DIR"

  cargo run --quiet --bin livepeer-orchestrator -- --env-config config/env/dev.yaml replay \
    --source-sqlite "$FIXTURE_DIR/seed.sqlite" \
    --from-block "$FROM_BLOCK" \
    --to-block "$TO_BLOCK" \
    --skip-cross-check

  # TD-025/TD-026: orchestrator_profile and broadcaster_profile are now
  # materialized views over orch_stake_by_round and gateway_balances_by_block.
  # Replay populates the source tables but the matviews need an explicit
  # refresh before their post-replay content is observable for hashing.
  # In live mode the daemon's matview-refresh loop handles this every 30 s.
  psql "$DATABASE_URL" <<'SQL'
REFRESH MATERIALIZED VIEW broadcaster_profile;
REFRESH MATERIALIZED VIEW orchestrator_profile;
SQL

  actual="$(mktemp)"
  bash scripts/compute-determinism-hashes.sh "$actual"

  if ! diff -u "$FIXTURE_DIR/expected_hashes.json" "$actual"; then
    rm -f "$actual"
    echo "determinism replay mismatch for $FIXTURE_DIR" >&2
    exit 1
  fi
  rm -f "$actual"
  echo "determinism replay matched expected hashes for $FIXTURE_DIR"
done
