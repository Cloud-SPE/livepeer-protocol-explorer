#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${DATABASE_URL:-}" ]]; then
  echo "DATABASE_URL is required" >&2
  exit 1
fi

ROOT=/home/mazup/git-repos/crypto-price-feed
FIXTURE_DIR="${1:-$ROOT/tests/fixtures}"

if [[ ! -d "$FIXTURE_DIR" ]]; then
  echo "fixture dir not found: $FIXTURE_DIR" >&2
  exit 1
fi

psql "$DATABASE_URL" <<SQL
TRUNCATE TABLE rpc_call_cache, indexer_checkpoints RESTART IDENTITY CASCADE;
CREATE TEMP TABLE rpc_cache_import (
  call_hash TEXT,
  method TEXT,
  params_json TEXT,
  block_number TEXT,
  response_hex TEXT,
  response_hash TEXT,
  provider TEXT,
  cross_check_provider TEXT,
  cross_check_response_hash TEXT
);
\copy rpc_cache_import FROM '$FIXTURE_DIR/rpc_cache.csv' CSV HEADER
INSERT INTO rpc_call_cache (
  call_hash,
  method,
  params,
  block_number,
  response_bytes,
  response_hash,
  provider,
  cross_check_provider,
  cross_check_response_hash
)
SELECT
  call_hash,
  method,
  params_json::jsonb,
  NULLIF(block_number, '')::bigint,
  decode(response_hex, 'hex'),
  response_hash,
  provider,
  NULLIF(cross_check_provider, ''),
  NULLIF(cross_check_response_hash, '')
FROM rpc_cache_import;

CREATE TEMP TABLE replay_checkpoint_import (
  name TEXT,
  last_processed_block BIGINT
);
\copy replay_checkpoint_import FROM '$FIXTURE_DIR/replay_checkpoints.csv' CSV HEADER
INSERT INTO indexer_checkpoints (name, chain_id, last_processed_block, updated_at)
SELECT name, 42161, last_processed_block, now()
FROM replay_checkpoint_import
ON CONFLICT (name) DO UPDATE
SET last_processed_block = EXCLUDED.last_processed_block,
    updated_at = now();
SQL
