#!/usr/bin/env bash
# scripts/full-run-detached.sh
#
# Long-running, resumable, fully-detached backfill from Livepeer Arbitrum
# genesis (block 6,072,093, Feb 2022) to current finalized head.
#
# Designed to survive session end — launch via:
#     nohup bash scripts/full-run-detached.sh > full-run.log 2>&1 < /dev/null &
#     disown
#
# Then check progress / status / stop with the helpers at the bottom of this file.
#
# Resumability: each indexer contract has its own checkpoint name
# (`indexer_<ContractName>`). Killing the script and restarting picks up where
# each contract left off. Valuator + staker are also idempotent (LEFT JOIN
# candidate filter).
#
# Cleanup level A is assumed to be done already (work tables truncated, seed
# preserved). Re-running this script after a crash WILL NOT re-truncate.

set -uo pipefail   # no -e: we want to keep going even if a phase has retriable errors

ROOT=/home/mazup/git-repos/crypto-price-feed
cd "$ROOT"

# Load secrets.
set -a; source .env; set +a
export DATABASE_URL="postgres://livepeer:changeme@127.0.0.1:5432/livepeer_indexer"
export STATIC_CONFIG="$ROOT/config/arbitrum.yaml"
export ENV_CONFIG="$ROOT/config/env/dev.yaml"
export SOURCE_SQLITE="$ROOT/sqlite-4.0.db"

GENESIS_BLOCK=6072093

phase() {
  echo
  echo "============================================================"
  echo "  $*  ($(date -u +%Y-%m-%dT%H:%M:%SZ))"
  echo "============================================================"
}

# ---------- pick TO_BLOCK ----------
phase "pick TO_BLOCK"
HEAD_HEX=$(curl -s --max-time 10 -X POST -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
  "$CHAINSTACK_RPC_URL" | python3 -c "import sys, json; print(json.load(sys.stdin)['result'])")
HEAD=$((HEAD_HEX))
TO_BLOCK=$((HEAD - 50))
SPAN=$((TO_BLOCK - GENESIS_BLOCK))
echo "  L2 head:   $HEAD"
echo "  TO_BLOCK:  $TO_BLOCK"
echo "  Span:      $SPAN blocks ≈ $((SPAN / 14400)) hours of activity"

# ---------- 5 indexer contracts (resumable) ----------
for contract in bonding-manager ticket-broker livepeer-token rounds-manager governor; do
  phase "indexer: $contract"
  target/release/livepeer-indexer backfill \
      --contract "$contract" \
      --from-block "$GENESIS_BLOCK" \
      --to-block "$TO_BLOCK" \
      2>&1 || echo "  $contract failed; continuing — restart will resume from checkpoint"
done

# ---------- finality watcher ----------
phase "finality-watcher (single pass)"
target/release/livepeer-finality-watcher --once 2>&1 || true

# ---------- valuator ----------
phase "valuator (backfill-all: seed → ETH on-chain → LPT on-chain → multi-asset)"
target/release/livepeer-valuator backfill-all 2>&1 || true

# ---------- staker ----------
phase "staker (flow + refresh-pending + gateway + profile)"
target/release/livepeer-staker backfill 2>&1 || true
target/release/livepeer-staker refresh-pending 2>&1 || true
target/release/livepeer-staker gateway-backfill 2>&1 || true
target/release/livepeer-staker profile-backfill 2>&1 || true

# ---------- rollups ----------
phase "rollups (payouts + rewards + tickets)"
target/release/livepeer-rollups orch-payouts-daily 2>&1 || true
target/release/livepeer-rollups orch-rewards-daily 2>&1 || true
target/release/livepeer-rollups tickets-daily 2>&1 || true

# ---------- enricher ----------
phase "enricher (ENS backfill)"
target/release/livepeer-enricher backfill 2>&1 || true

# ---------- cross-check ----------
phase "cross-check (TD-004 / SPEC §24.1)"
target/release/livepeer-seed-migrator cross-check --source-sqlite "$SOURCE_SQLITE" 2>&1 || true

# ---------- final summary ----------
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
ORDER BY t;
"
psql "$DATABASE_URL" -c "
SELECT contract_name, event_name, COUNT(*) AS rows
  FROM raw_protocol_events GROUP BY 1,2 ORDER BY rows DESC LIMIT 20;
"
psql "$DATABASE_URL" -c "
SELECT name, last_processed_block FROM indexer_checkpoints ORDER BY name;
"

phase "DONE"
