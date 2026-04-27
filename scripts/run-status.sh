#!/usr/bin/env bash
# scripts/run-status.sh — quick check of detached run progress (works for
# both sequential and parallel-5 launches).

ROOT=/home/mazup/git-repos/crypto-price-feed
cd "$ROOT"
export DATABASE_URL="postgres://livepeer:changeme@127.0.0.1:5432/livepeer_indexer"

echo "=== indexer processes running ==="
pgrep -af 'livepeer-(indexer|valuator|staker|reorg|finality|seed-migrator)' \
  | grep -vE 'service-registry|payment-daemon|openai-byoc' || echo "  none"

echo
echo "=== per-contract checkpoints ==="
psql "$DATABASE_URL" -c "
SELECT name,
       last_processed_block,
       to_char(updated_at, 'YYYY-MM-DD HH24:MI:SS') AS updated_at,
       AGE(now(), updated_at) AS since_update
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
echo "=== events by (contract, event) ==="
psql "$DATABASE_URL" -c "
SELECT contract_name, event_name, COUNT(*) AS rows
  FROM raw_protocol_events GROUP BY 1,2 ORDER BY rows DESC LIMIT 15;"

echo
echo "=== per-contract log tails (last commit/error per file) ==="
for f in logs/indexer-*.log; do
    [ -f "$f" ] && {
        echo "--- $f ---"
        grep -E '"message":"(chunk committed|backfill complete|transient RPC error|chunk failed|halting)"' "$f" \
          | tail -2 \
          | python3 -c "
import sys, json
for line in sys.stdin:
    try:
        d = json.loads(line)
        f = d.get('fields', {})
        ts = d.get('timestamp', '?')[:19]
        msg = f.get('message', '?')
        cs = f.get('chunk_start', '')
        ce = f.get('chunk_end', '')
        ev = f.get('events_inserted', '')
        print(f'  {ts}  {msg}  start={cs}  end={ce}  events={ev}')
    except Exception:
        pass
"
    }
done
