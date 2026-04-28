#!/usr/bin/env bash
# scripts/snapshot-baseline.sh
#
# Capture a content snapshot of the indexer-derived tables before a clean
# re-run, so the re-run's output can be byte-compared for determinism.
# Writes to baselines/<timestamp>/.
#
# Usage:
#     bash scripts/snapshot-baseline.sh
#
# What we hash:
#   - row counts per table
#   - sha256 of raw_protocol_events ordered by (chain_id, tx_hash, log_index)
#     selecting only the deterministic columns (excludes updated_at etc.)
#   - same for event_valuations, stake_balances_by_block, token_prices_by_block

set -euo pipefail

ROOT=/home/mazup/git-repos/crypto-price-feed
cd "$ROOT"
export DATABASE_URL="postgres://livepeer:changeme@127.0.0.1:5432/livepeer_indexer"

STAMP=$(date -u +%Y%m%dT%H%M%SZ)
OUT="baselines/$STAMP"
mkdir -p "$OUT"

echo "Snapshotting → $OUT/"

psql "$DATABASE_URL" -At -c "
SELECT 'raw_protocol_events',     COUNT(*) FROM raw_protocol_events UNION ALL
SELECT 'event_valuations',        COUNT(*) FROM event_valuations UNION ALL
SELECT 'stake_balances_by_block', COUNT(*) FROM stake_balances_by_block UNION ALL
SELECT 'delegator_registry',      COUNT(*) FROM delegator_registry UNION ALL
SELECT 'token_prices_by_block',   COUNT(*) FROM token_prices_by_block UNION ALL
SELECT 'reorg_events',            COUNT(*) FROM reorg_events UNION ALL
SELECT 'decode_failures',         COUNT(*) FROM decode_failures UNION ALL
SELECT 'rpc_call_cache',          COUNT(*) FROM rpc_call_cache UNION ALL
SELECT 'seeded_event_prices',     COUNT(*) FROM seeded_event_prices
ORDER BY 1;" > "$OUT/row_counts.txt"

psql "$DATABASE_URL" -At -c "
SELECT contract_name, event_name, COUNT(*)
  FROM raw_protocol_events
  GROUP BY 1,2 ORDER BY 1,2;" > "$OUT/events_by_contract.txt"

# Deterministic-column hash via md5() (built-in to Postgres; no pgcrypto needed).
# Excludes ephemeral metadata (created_at, updated_at). raw_event JSONB is
# included via jsonb_strip_nulls so key ordering normalizes.
# We md5() each row, then md5() the ordered concatenation — bounded memory.
psql "$DATABASE_URL" -At -c "
SELECT md5(string_agg(row_md5, '' ORDER BY chain_id, tx_hash, log_index))
  FROM (
    SELECT chain_id, tx_hash, log_index,
           md5(concat_ws('§',
             chain_id::text, tx_hash, log_index::text, block_number::text,
             block_hash, block_timestamp::text, contract_address, contract_name,
             event_name, event_signature, COALESCE(asset,''),
             COALESCE(amount_raw::text,''), COALESCE(amount_normalized::text,''),
             is_valuable::text, COALESCE(from_address,''), COALESCE(to_address,''),
             finality, is_canonical::text, abi_hash_used,
             COALESCE(jsonb_strip_nulls(raw_event)::text, '')
           )) AS row_md5
      FROM raw_protocol_events
  ) s;" > "$OUT/raw_protocol_events.md5"

psql "$DATABASE_URL" -At -c "
SELECT md5(string_agg(row_md5, '' ORDER BY chain_id, tx_hash, log_index, asset))
  FROM (
    SELECT chain_id, tx_hash, log_index, asset,
           md5(concat_ws('§',
             chain_id::text, tx_hash, log_index::text, asset,
             COALESCE(usd_value::text,''), COALESCE(amount_native::text,''),
             COALESCE(price_at_block::text,''), valuation_version,
             COALESCE(degraded_reason::text,'')
           )) AS row_md5
      FROM event_valuations
  ) s;" > "$OUT/event_valuations.md5"

# Save schema fingerprint too — if migrations changed between snapshot & rerun,
# divergence is expected.
psql "$DATABASE_URL" -At -c "SELECT version, description FROM _sqlx_migrations ORDER BY version;" > "$OUT/migrations.txt"

# Save current binary's git rev so we know which version produced the baseline.
git rev-parse HEAD > "$OUT/git_rev.txt" 2>&1 || echo "(no git)" > "$OUT/git_rev.txt"

echo
echo "Wrote:"
ls -la "$OUT/"
echo
echo "Use this as input to scripts/validate-vs-baseline.sh after the clean re-run."
