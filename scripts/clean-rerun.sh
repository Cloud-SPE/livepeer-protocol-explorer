#!/usr/bin/env bash
# scripts/clean-rerun.sh
#
# Wipe runtime-derived tables and relaunch the parallel-5 indexer for a
# clean real-data run. Keeps rpc_call_cache, seeded_event_prices,
# contract_abi_registry, classifications / overrides, ENS tables, and _sqlx_migrations.
#
# Refuses to run if any runtime worker process is alive.
# Refuses to run without --confirm.
#
# Usage:
#     bash scripts/snapshot-baseline.sh                  # capture baseline first
#     bash scripts/clean-rerun.sh --confirm              # do the wipe + launch
#     bash scripts/run-status.sh                         # monitor
#     # when all 5 finish:
#     bash scripts/full-run-post-indexer.sh              # post pipeline
#     bash scripts/validate-vs-baseline.sh <baseline-dir>  # diff vs snapshot

set -uo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

if [[ "${1:-}" != "--confirm" ]]; then
  cat <<EOF
ERROR: this is a destructive operation. To proceed, re-run with:
    bash scripts/clean-rerun.sh --confirm

Will TRUNCATE:
    raw_protocol_events
    decode_failures
    event_valuations
    valuation_attempts
    stake_balances_by_block
    delegator_registry
    token_prices_by_block
    gateway_balances_by_block
    gateway_flows
    gateway_claimants_by_block
    orchestrator_profile
    broadcaster_profile
    orch_payouts_daily
    orch_rewards_daily
    tickets_daily
    reorg_events
    reorg_mutations
    rpc_divergence_failures
    indexer_checkpoints

Will KEEP:
    rpc_call_cache         (deterministic backbone — replay accelerator)
    seeded_event_prices    (static SQLite overlay)
    contract_abi_registry  (boot validation)
    broadcaster_classifications
    name_avatar_overrides
    orchestrator_ens
    broadcaster_ens
    _sqlx_migrations

Logs will move to logs.<UTC-stamp>/
EOF
  exit 1
fi

# Safety gate: refuse if any service is alive.
ALIVE=$(pgrep -af 'livepeer-(indexer|valuator|staker|reorg|finality|seed-migrator|rollups|enricher|orchestrator)' \
        | grep -vE 'service-registry|payment-daemon|openai-byoc' || true)
if [[ -n "$ALIVE" ]]; then
  echo "REFUSING: indexer/valuator/staker processes still alive:"
  echo "$ALIVE"
  echo "Wait for them to finish (or kill them deliberately) before clean-rerun."
  exit 2
fi

set -a; source .env; set +a
export DATABASE_URL="postgres://livepeer:changeme@127.0.0.1:5432/livepeer_indexer"

STAMP=$(date -u +%Y%m%dT%H%M%SZ)

echo "=== $(date -u +%Y-%m-%dT%H:%M:%SZ) — clean re-run starting ==="
echo

if [[ -d logs ]]; then
  echo "Moving logs/ → logs.$STAMP/"
  mv logs "logs.$STAMP"
fi
mkdir -p logs

echo "Truncating derived tables…"
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 <<'SQL'
TRUNCATE TABLE
  raw_protocol_events,
  decode_failures,
  event_valuations,
  valuation_attempts,
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
  token_prices_by_block,
  reorg_events,
  reorg_mutations,
  rpc_divergence_failures,
  indexer_checkpoints
RESTART IDENTITY CASCADE;
SQL
echo

echo "Post-truncate row counts (sanity check):"
psql "$DATABASE_URL" -c "
SELECT 'raw_protocol_events'   AS t, COUNT(*) FROM raw_protocol_events
UNION ALL SELECT 'event_valuations',          COUNT(*) FROM event_valuations
UNION ALL SELECT 'gateway_balances_by_block', COUNT(*) FROM gateway_balances_by_block
UNION ALL SELECT 'orchestrator_profile',      COUNT(*) FROM orchestrator_profile
UNION ALL SELECT 'orch_payouts_daily',        COUNT(*) FROM orch_payouts_daily
UNION ALL SELECT 'rpc_call_cache (kept)',     COUNT(*) FROM rpc_call_cache
UNION ALL SELECT 'seeded_event_prices (kept)',COUNT(*) FROM seeded_event_prices
UNION ALL SELECT 'broadcaster_classifications (kept)', COUNT(*) FROM broadcaster_classifications
UNION ALL SELECT 'name_avatar_overrides (kept)', COUNT(*) FROM name_avatar_overrides
UNION ALL SELECT 'orchestrator_ens (kept)', COUNT(*) FROM orchestrator_ens
UNION ALL SELECT 'broadcaster_ens (kept)', COUNT(*) FROM broadcaster_ens
UNION ALL SELECT 'contract_abi_registry (kept)', COUNT(*) FROM contract_abi_registry
ORDER BY t;"
echo

echo "Launching parallel-5 indexer…"
bash scripts/full-run-parallel.sh

echo
echo "=== $(date -u +%Y-%m-%dT%H:%M:%SZ) — clean re-run launched ==="
echo "Monitor: bash scripts/run-status.sh"
echo "When all 5 finish: bash scripts/full-run-post-indexer.sh"
