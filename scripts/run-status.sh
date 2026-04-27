#!/usr/bin/env bash
# scripts/run-status.sh — quick check of detached run progress.
# Usage: bash scripts/run-status.sh

ROOT=/home/mazup/git-repos/crypto-price-feed
cd "$ROOT"
export DATABASE_URL="postgres://livepeer:changeme@127.0.0.1:5432/livepeer_indexer"

echo "=== process running? ==="
pgrep -af 'full-run-detached|livepeer-(indexer|valuator|staker|reorg|finality|seed-migrator)' || echo "  no detached processes running"

echo
echo "=== indexer checkpoints (per-contract resume points) ==="
psql "$DATABASE_URL" -c "
SELECT name, last_processed_block,
       to_char(updated_at, 'YYYY-MM-DD HH24:MI:SS') AS updated_at
  FROM indexer_checkpoints ORDER BY name;"

echo
echo "=== row counts ==="
psql "$DATABASE_URL" -c "
SELECT 'raw_protocol_events'   AS t, COUNT(*) AS rows FROM raw_protocol_events
UNION ALL SELECT 'event_valuations',          COUNT(*) FROM event_valuations
UNION ALL SELECT 'stake_balances_by_block',   COUNT(*) FROM stake_balances_by_block
UNION ALL SELECT 'token_prices_by_block',     COUNT(*) FROM token_prices_by_block
UNION ALL SELECT 'rpc_call_cache',            COUNT(*) FROM rpc_call_cache
UNION ALL SELECT 'reorg_events',              COUNT(*) FROM reorg_events
ORDER BY t;"

echo
echo "=== recent activity ==="
psql "$DATABASE_URL" -c "
SELECT contract_name, event_name, COUNT(*) AS rows
  FROM raw_protocol_events GROUP BY 1,2 ORDER BY rows DESC LIMIT 10;"

echo
echo "=== tail full-run.log (last 20 lines) ==="
[ -f full-run.log ] && tail -20 full-run.log || echo "  no full-run.log found"
