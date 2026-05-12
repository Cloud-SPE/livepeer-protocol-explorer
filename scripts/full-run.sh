#!/usr/bin/env bash
# scripts/full-run.sh
#
# End-to-end backfill of all Livepeer protocol events on Arbitrum One,
# from genesis (block 6,072,093, Feb 2022) to current finalized head.
#
# Cleanup level A: truncates work tables only — preserves seed prices,
# ABI registry, and rpc_call_cache (the deterministic input set).
#
# Resource budget (rough): ~600K events, ~700K RPC calls, $75-400 on
# Chainstack, multi-hour wall-clock. See SPEC §13.6.
#
# Usage:   bash scripts/full-run.sh
# Each phase can be re-invoked individually by uncommenting / sourcing.

set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

# Load secrets.
set -a; source .env; set +a
# Override DATABASE_URL for host-side use (the Docker-internal hostname won't resolve here).
export DATABASE_URL="postgres://livepeer:changeme@127.0.0.1:5432/livepeer_indexer"
export STATIC_CONFIG="$ROOT/config/arbitrum.yaml"
export ENV_CONFIG="$ROOT/config/env/dev.yaml"
export SOURCE_SQLITE="$ROOT/sqlite-4.0.db"

GENESIS_BLOCK=6072093

phase() { echo; echo "============================================================"; echo "  $*  ($(date -u +%H:%M:%S))"; echo "============================================================"; }
psql_q() { psql "$DATABASE_URL" -tA -c "$1"; }

# ---------- Phase 0: confirm release builds are current ----------
phase "0. Build release binaries"
cargo build --release --bins --quiet
echo "  binaries built"

# ---------- Phase 1: pick TO_BLOCK ----------
phase "1. Determine TO_BLOCK"
HEAD_HEX=$(curl -s --max-time 10 -X POST -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
  "$CHAINSTACK_RPC_URL" | python3 -c "import sys, json; print(json.load(sys.stdin)['result'])")
HEAD=$((HEAD_HEX))
TO_BLOCK=$((HEAD - 50))
SPAN=$((TO_BLOCK - GENESIS_BLOCK))
echo "  L2 head:    $HEAD"
echo "  TO_BLOCK:   $TO_BLOCK  (head − 50 safety margin)"
echo "  Span:       $SPAN blocks ($((SPAN / 14400)) hours of activity)"

# ---------- Phase 2: cleanup A — truncate work tables ----------
phase "2. Truncate work tables (cleanup level A)"
psql "$DATABASE_URL" <<'SQL' >/dev/null
TRUNCATE
    raw_protocol_events,
    event_valuations,
    valuation_attempts,
    decode_failures,
    stake_balances_by_block,
    delegator_registry,
    gateway_balances_by_block,
    gateway_flows,
    gateway_claimants_by_block,
    orchestrator_profile,
    broadcaster_profile,
    orch_payouts_daily,
    orch_rewards_daily,
    tickets_daily,
    reorg_events,
    reorg_mutations,
    indexer_checkpoints,
    rpc_divergence_failures,
    token_prices_by_block
RESTART IDENTITY CASCADE;
SQL
echo "  preserved: seeded_event_prices, contract_abi_registry, rpc_call_cache, classifications, overrides, ens"

# ---------- Phase 3: verify-rpc (sanity) ----------
phase "3. verify-rpc (RPC connectivity + cache row write)"
target/release/livepeer-seed-migrator verify-rpc 2>&1 \
  | grep -E '"message":"(both providers|block heads|cross-check passed|verify-rpc complete|.*cardinality.*)"' \
  | tail -6

# ---------- Phase 4: re-seed ABI registry (idempotent) ----------
phase "4. seed-abi-registry (idempotent)"
target/release/livepeer-seed-migrator seed-abi-registry 2>&1 \
  | grep '"message":"abi registry seed complete"' || true

# ---------- Phase 5: re-import seed prices (no-op since preserved) ----------
phase "5. import seed prices (idempotent — should be 0 inserted)"
target/release/livepeer-seed-migrator import --source-sqlite "$SOURCE_SQLITE" 2>&1 \
  | grep '"message":"seed import complete"'

# ---------- Phase 6: indexer — 5 contracts ----------
phase "6. Indexer backfill (5 contracts × $SPAN blocks)"
for contract in bonding-manager ticket-broker livepeer-token rounds-manager governor; do
    echo
    echo "  --- $contract ---"
    target/release/livepeer-indexer backfill \
        --contract "$contract" \
        --from-block "$GENESIS_BLOCK" \
        --to-block "$TO_BLOCK" \
        --no-resume 2>&1 \
      | grep -E '"message":"(chunk committed|backfill complete|transient RPC error)"' \
      | tail -10
done

EVT=$(psql_q "SELECT COUNT(*) FROM raw_protocol_events")
echo "  raw_protocol_events: $EVT rows"

# ---------- Phase 7: finality watcher (single pass) ----------
phase "7. finality-watcher (single pass — promotes events to finalized)"
target/release/livepeer-finality-watcher --once 2>&1 | tail -2

# ---------- Phase 8: valuator (seed → ETH on-chain → LPT on-chain → multi-asset) ----------
phase "8. Valuator (backfill-all)"
target/release/livepeer-valuator backfill-all 2>&1 \
  | grep -E '"message":"(seed pass summary|on-chain ETH pass summary|on-chain LPT pass summary|multi-asset pass summary)"'

# ---------- Phase 9: staker ----------
phase "9. Staker — flow + refresh-pending + gateway + profile"
target/release/livepeer-staker backfill 2>&1 \
  | grep '"message":"staker flow backfill summary"'
target/release/livepeer-staker refresh-pending 2>&1 \
  | grep '"message":"staker pending refresh summary"'
target/release/livepeer-staker gateway-backfill 2>&1 || true
target/release/livepeer-staker profile-backfill 2>&1 || true

# ---------- Phase 10: rollups ----------
phase "10. Rollups — payouts + rewards + tickets"
target/release/livepeer-rollups orch-payouts-daily 2>&1 || true
target/release/livepeer-rollups orch-rewards-daily 2>&1 || true
target/release/livepeer-rollups tickets-daily 2>&1 || true

# ---------- Phase 11: enricher ----------
phase "11. Enricher — ENS backfill"
target/release/livepeer-enricher backfill 2>&1 || true

# ---------- Phase 12: cross-check ----------
phase "12. Cross-check (TD-004 / SPEC §24.1)"
target/release/livepeer-seed-migrator cross-check --source-sqlite "$SOURCE_SQLITE" 2>&1 \
  | grep '"message":"cross-check report"'

# ---------- Phase 13: final summary ----------
phase "13. Final state summary"
psql "$DATABASE_URL" -c "
SELECT 'raw_protocol_events'   AS t, COUNT(*) AS rows FROM raw_protocol_events
UNION ALL SELECT 'event_valuations',          COUNT(*) FROM event_valuations
UNION ALL SELECT 'valuation_attempts',        COUNT(*) FROM valuation_attempts
UNION ALL SELECT 'decode_failures',           COUNT(*) FROM decode_failures
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
UNION ALL SELECT 'rpc_divergence_failures',   COUNT(*) FROM rpc_divergence_failures
UNION ALL SELECT 'seeded_event_prices',       COUNT(*) FROM seeded_event_prices
UNION ALL SELECT 'contract_abi_registry',     COUNT(*) FROM contract_abi_registry
UNION ALL SELECT 'indexer_checkpoints',       COUNT(*) FROM indexer_checkpoints
ORDER BY t;
"

psql "$DATABASE_URL" -c "
SELECT contract_name, event_name, COUNT(*) AS rows
  FROM raw_protocol_events GROUP BY 1,2 ORDER BY rows DESC LIMIT 20;
"

psql "$DATABASE_URL" -c "
SELECT source, COUNT(*) FROM event_valuations GROUP BY 1 ORDER BY 2 DESC;
"

phase "DONE"
