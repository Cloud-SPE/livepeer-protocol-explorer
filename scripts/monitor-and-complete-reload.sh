#!/usr/bin/env bash
set -euo pipefail

ROOT=/home/mazup/git-repos/crypto-price-feed
cd "$ROOT"

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <bootstrap-pid>" >&2
  exit 1
fi

BOOTSTRAP_PID="$1"
GENESIS_BLOCK=6072093
STATUS_INTERVAL_SECS=60

set -a
source .env
set +a

export DATABASE_URL="${DATABASE_URL/@postgres:/@127.0.0.1:}"
export STATIC_CONFIG="$ROOT/config/arbitrum.yaml"
export ENV_CONFIG="$ROOT/config/env/dev.yaml"

LOG_DIR="$ROOT/run-logs/full-reload"
mkdir -p "$LOG_DIR"
STATUS_FILE="$LOG_DIR/status.txt"
SUMMARY_FILE="$LOG_DIR/summary.txt"

phase() {
  printf '\n[%s] %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*"
}

write_status() {
  local tmp
  tmp="$(mktemp)"
  {
    echo "timestamp=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "bootstrap_pid=$BOOTSTRAP_PID"
    echo
    echo "=== processes ==="
    pgrep -af 'livepeer-(indexer|valuator|staker|orchestrator|finality|rollups|enricher)' || true
    echo
    echo "=== checkpoints ==="
    psql "$DATABASE_URL" -Atc \
      "select name || E'\t' || last_processed_block || E'\t' || to_char(updated_at, 'YYYY-MM-DD HH24:MI:SS') from indexer_checkpoints order by 1;" || true
    echo
    echo "=== table counts ==="
    psql "$DATABASE_URL" -Atc "
      select 'raw_protocol_events' || E'\t' || count(*) from raw_protocol_events
      union all select 'event_valuations' || E'\t' || count(*) from event_valuations
      union all select 'stake_balances_by_block' || E'\t' || count(*) from stake_balances_by_block
      union all select 'gateway_balances_by_block' || E'\t' || count(*) from gateway_balances_by_block
      union all select 'gateway_flows' || E'\t' || count(*) from gateway_flows
      union all select 'gateway_claimants_by_block' || E'\t' || count(*) from gateway_claimants_by_block
      union all select 'orchestrator_profile' || E'\t' || count(*) from orchestrator_profile
      union all select 'broadcaster_profile' || E'\t' || count(*) from broadcaster_profile
      union all select 'orch_payouts_daily' || E'\t' || count(*) from orch_payouts_daily
      union all select 'orch_rewards_daily' || E'\t' || count(*) from orch_rewards_daily
      union all select 'tickets_daily' || E'\t' || count(*) from tickets_daily
      union all select 'orchestrator_ens' || E'\t' || count(*) from orchestrator_ens
      union all select 'broadcaster_ens' || E'\t' || count(*) from broadcaster_ens
      order by 1;" || true
  } >"$tmp"
  mv "$tmp" "$STATUS_FILE"
}

run_stage() {
  local name="$1"
  shift
  local log_file="$LOG_DIR/${name}.log"
  phase "stage $name starting"
  {
    echo "[$(date -u +%Y-%m-%dT%H:%M:%SZ)] START $name"
    "$@"
    echo "[$(date -u +%Y-%m-%dT%H:%M:%SZ)] DONE $name"
  } >>"$log_file" 2>&1
  phase "stage $name finished"
  write_status
}

phase "monitoring bootstrap pid $BOOTSTRAP_PID"
while kill -0 "$BOOTSTRAP_PID" 2>/dev/null; do
  write_status
  sleep "$STATUS_INTERVAL_SECS"
done
write_status

phase "bootstrap pid exited; starting resumable completion"

HEAD="$(cast block-number --rpc-url "$CHAINSTACK_RPC_URL")"
TO_BLOCK=$((HEAD - 50))
echo "to_block=$TO_BLOCK" >"$SUMMARY_FILE"

for contract in bonding-manager ticket-broker livepeer-token rounds-manager governor; do
  run_stage "indexer-${contract}" \
    target/debug/livepeer-indexer \
    --env-config "$ENV_CONFIG" \
    backfill \
    --contract "$contract" \
    --from-block "$GENESIS_BLOCK" \
    --to-block "$TO_BLOCK"
done

run_stage "finality-once" \
  target/debug/livepeer-finality-watcher \
  --env-config "$ENV_CONFIG" \
  --once

run_stage "valuator-backfill-all" \
  target/debug/livepeer-valuator \
  --env-config "$ENV_CONFIG" \
  backfill-all

run_stage "staker-flow-backfill" \
  target/debug/livepeer-staker \
  --env-config "$ENV_CONFIG" \
  backfill

run_stage "staker-refresh-pending" \
  target/debug/livepeer-staker \
  --env-config "$ENV_CONFIG" \
  refresh-pending

run_stage "staker-gateway-backfill" \
  target/debug/livepeer-staker \
  --env-config "$ENV_CONFIG" \
  gateway-backfill

run_stage "staker-profile-backfill" \
  target/debug/livepeer-staker \
  --env-config "$ENV_CONFIG" \
  profile-backfill

run_stage "rollups-orch-payouts-daily" \
  target/debug/livepeer-rollups \
  --env-config "$ENV_CONFIG" \
  orch-payouts-daily

run_stage "rollups-orch-rewards-daily" \
  target/debug/livepeer-rollups \
  --env-config "$ENV_CONFIG" \
  orch-rewards-daily

run_stage "rollups-tickets-daily" \
  target/debug/livepeer-rollups \
  --env-config "$ENV_CONFIG" \
  tickets-daily

run_stage "enricher-backfill" \
  target/debug/livepeer-enricher \
  --env-config "$ENV_CONFIG" \
  backfill

phase "full reload complete"
write_status
