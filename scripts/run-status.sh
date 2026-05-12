#!/usr/bin/env bash
# scripts/run-status.sh — quick check of detached run progress (works for
# both sequential and parallel-5 launches).

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"
export DATABASE_URL="postgres://livepeer:changeme@127.0.0.1:5432/livepeer_indexer"

echo "=== active processes ==="
pgrep -af 'livepeer-(indexer|valuator|staker|reorg|finality|seed-migrator|rollups|enricher|orchestrator)' \
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
UNION ALL SELECT 'gateway_balances_by_block', COUNT(*) FROM gateway_balances_by_block
UNION ALL SELECT 'gateway_flows',             COUNT(*) FROM gateway_flows
UNION ALL SELECT 'gateway_claimants_by_block',COUNT(*) FROM gateway_claimants_by_block
UNION ALL SELECT 'orchestrator_profile',      COUNT(*) FROM orchestrator_profile
UNION ALL SELECT 'broadcaster_profile',       COUNT(*) FROM broadcaster_profile
UNION ALL SELECT 'orch_payouts_daily',        COUNT(*) FROM orch_payouts_daily
UNION ALL SELECT 'orch_rewards_daily',        COUNT(*) FROM orch_rewards_daily
UNION ALL SELECT 'tickets_daily',             COUNT(*) FROM tickets_daily
UNION ALL SELECT 'orchestrator_ens',          COUNT(*) FROM orchestrator_ens
UNION ALL SELECT 'broadcaster_ens',           COUNT(*) FROM broadcaster_ens
UNION ALL SELECT 'token_prices_by_block',     COUNT(*) FROM token_prices_by_block
UNION ALL SELECT 'rpc_call_cache',            COUNT(*) FROM rpc_call_cache
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

if [[ -f run-logs/full-reload/status.txt ]]; then
    echo
    echo "=== detached full-reload snapshot ==="
    sed -n '1,80p' run-logs/full-reload/status.txt
fi
