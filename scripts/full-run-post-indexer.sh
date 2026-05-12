#!/usr/bin/env bash
# scripts/full-run-post-indexer.sh
#
# Run after all 5 parallel indexers complete. Drives finality → valuator →
# staker deterministic/materialized phases → rollups → ENS enricher → final summary.
#
# Idempotent throughout; safe to re-run if something fails partway.

set -uo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

set -a; source .env; set +a
export DATABASE_URL="postgres://livepeer:changeme@127.0.0.1:5432/livepeer_indexer"
export STATIC_CONFIG="$ROOT/config/arbitrum.yaml"
export ENV_CONFIG="$ROOT/config/env/dev.yaml"
export SOURCE_SQLITE="$ROOT/sqlite-4.0.db"

phase() {
  echo
  echo "============================================================"
  echo "  $*  ($(date -u +%Y-%m-%dT%H:%M:%SZ))"
  echo "============================================================"
}

phase "finality-watcher (single pass)"
target/release/livepeer-finality-watcher --once 2>&1 | tail -3 || true

phase "valuator (backfill-all)"
# Keep the valuator's full output in the main log. The previous grep-only
# view hid slow progress and early failures, making the stage look dead.
target/release/livepeer-valuator backfill-all 2>&1 || true

phase "staker (flow + refresh-pending + gateway + profile)"
target/release/livepeer-staker backfill 2>&1 || true
target/release/livepeer-staker refresh-pending 2>&1 || true
target/release/livepeer-staker gateway-backfill 2>&1 || true
target/release/livepeer-staker profile-backfill 2>&1 || true

phase "rollups (payouts + rewards + tickets)"
target/release/livepeer-rollups orch-payouts-daily 2>&1 || true
target/release/livepeer-rollups orch-rewards-daily 2>&1 || true
target/release/livepeer-rollups tickets-daily 2>&1 || true

phase "enricher (ENS backfill)"
target/release/livepeer-enricher backfill 2>&1 || true

phase "cross-check"
target/release/livepeer-seed-migrator cross-check --source-sqlite "$SOURCE_SQLITE" 2>&1 || true

phase "final summary"
psql "$DATABASE_URL" -c "
SELECT 'raw_protocol_events'   AS t, COUNT(*) FROM raw_protocol_events
UNION ALL SELECT 'event_valuations',          COUNT(*) FROM event_valuations
UNION ALL SELECT 'stake_balances_by_block',   COUNT(*) FROM stake_balances_by_block
UNION ALL SELECT 'delegator_registry',        COUNT(*) FROM delegator_registry
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
UNION ALL SELECT 'decode_failures',           COUNT(*) FROM decode_failures
ORDER BY t;
"
psql "$DATABASE_URL" -c "
SELECT contract_name, event_name, COUNT(*) AS rows
  FROM raw_protocol_events GROUP BY 1,2 ORDER BY rows DESC LIMIT 25;
"
psql "$DATABASE_URL" -c "
SELECT name, last_processed_block FROM indexer_checkpoints ORDER BY name;
"

phase "DONE"
