#!/usr/bin/env bash
# Idempotent resume of all catch-up processes after gateway-flow finishes.
#
# Safe to run multiple times — checks aliveness before starting each.
# Skips any process already running by checking for a known cmdline match.
#
# Use: bash scripts/resume-catchup-all.sh
set -uo pipefail

cd "$(git rev-parse --show-toplevel)"
set -a; source .env; set +a
export DATABASE_URL="postgres://livepeer:changeme@127.0.0.1:5432/livepeer_indexer"
export STATIC_CONFIG="$PWD/config/arbitrum.yaml"
export ENV_CONFIG="$PWD/config/env/dev.yaml"
export FE_STATIC_DIR="$PWD/frontend-ui/dist"

TS=$(date -u +%Y%m%dT%H%M%SZ)
mkdir -p run-logs/live

is_running() {
  # arg 1 = pgrep pattern
  pgrep -af "$1" 2>/dev/null | grep -v "$$" | grep -v resume-catchup | grep -q .
}

start_if_missing() {
  local label="$1"
  local pattern="$2"
  local cmd="$3"
  local logfile="run-logs/live/${label}-${TS}.log"
  if is_running "$pattern"; then
    echo "  ${label}: already running, skipping"
  else
    nohup bash -c "$cmd" >> "$logfile" 2>&1 &
    local pid=$!
    disown
    sleep 1
    if kill -0 "$pid" 2>/dev/null; then
      echo "  ${label}: started pid=$pid log=$logfile"
    else
      echo "  ${label}: ❌ FAILED to start, check $logfile"
    fi
  fi
}

echo "=== resume-catchup-all @ $TS ==="

start_if_missing api \
  "target/release/livepeer-api" \
  "target/release/livepeer-api"

start_if_missing enricher-follow \
  "target/release/livepeer-enricher follow" \
  "target/release/livepeer-enricher follow"

start_if_missing daemon-follow \
  "target/release/livepeer-daemon follow" \
  "target/release/livepeer-daemon follow"

start_if_missing rollup-payouts \
  "target/release/livepeer-rollups orch-payouts-daily" \
  "target/release/livepeer-rollups orch-payouts-daily --batch-limit 50000 --follow"

start_if_missing rollup-rewards \
  "target/release/livepeer-rollups orch-rewards-daily" \
  "target/release/livepeer-rollups orch-rewards-daily --follow"

start_if_missing rollup-tickets \
  "target/release/livepeer-rollups tickets-daily" \
  "target/release/livepeer-rollups tickets-daily --follow"

start_if_missing rollup-event-metrics \
  "target/release/livepeer-rollups event-metrics-daily" \
  "target/release/livepeer-rollups event-metrics-daily --follow"

start_if_missing gateway-loop \
  "scripts/loop-staker-gateway.sh" \
  "bash scripts/loop-staker-gateway.sh"

start_if_missing profile-follow \
  "target/release/livepeer-staker profile-follow" \
  "target/release/livepeer-staker profile-follow"

start_if_missing tx-receipts-follow \
  "target/release/livepeer-staker tx-receipts-follow" \
  "target/release/livepeer-staker tx-receipts-follow --batch-limit 5000 --concurrency 8 --cadence-secs 30"

echo ""
echo "=== one-shot enricher backfill (cold ENS sweep, runs in background) ==="
nohup target/release/livepeer-enricher backfill >> "run-logs/live/enricher-backfill-${TS}.log" 2>&1 &
echo "  enricher backfill pid=$! log=run-logs/live/enricher-backfill-${TS}.log"
disown

echo ""
echo "=== final aliveness ==="
sleep 3
ps -ef | grep -E "livepeer-(api|enricher|daemon|rollups|staker)|loop-staker" | grep -v grep | awk '{print "  pid=" $2, "uptime=" $7, $8, $9, $10}'

echo ""
echo "=== checkpoint snapshot ==="
PGPASSWORD=changeme psql -h 127.0.0.1 -U livepeer -d livepeer_indexer -A -F'|' \
  -c "SELECT name, last_processed_block FROM indexer_checkpoints ORDER BY name;" 2>&1
