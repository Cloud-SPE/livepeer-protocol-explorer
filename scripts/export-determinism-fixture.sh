#!/usr/bin/env bash
set -euo pipefail

ROOT=/home/mazup/git-repos/crypto-price-feed
cd "$ROOT"

if [[ -z "${DATABASE_URL:-}" ]]; then
  echo "DATABASE_URL is required" >&2
  exit 1
fi

FIXTURE_DIR="${1:-$ROOT/tests/fixtures}"
SEED_SQLITE="${2:-}"

mkdir -p "$FIXTURE_DIR"

psql "$DATABASE_URL" -c "\\copy (
  SELECT
    call_hash,
    method,
    params::text AS params_json,
    COALESCE(block_number::text, '') AS block_number,
    encode(response_bytes, 'hex') AS response_hex,
    response_hash,
    provider,
    COALESCE(cross_check_provider, '') AS cross_check_provider,
    COALESCE(cross_check_response_hash, '') AS cross_check_response_hash
  FROM rpc_call_cache
  ORDER BY call_hash
) TO '$FIXTURE_DIR/rpc_cache.csv' CSV HEADER"

psql "$DATABASE_URL" -c "\\copy (
  SELECT name, last_processed_block
  FROM indexer_checkpoints
  WHERE name IN ('replay_finality_latest_l1_ts', 'replay_finality_finalized_l1_ts')
  ORDER BY name
) TO '$FIXTURE_DIR/replay_checkpoints.csv' CSV HEADER"

if [[ -n "$SEED_SQLITE" ]]; then
  cp "$SEED_SQLITE" "$FIXTURE_DIR/seed.sqlite"
fi

bash scripts/compute-determinism-hashes.sh "$FIXTURE_DIR/expected_hashes.json"
