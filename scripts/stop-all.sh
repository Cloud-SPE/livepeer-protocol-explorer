#!/usr/bin/env bash
# Inverse of resume-catchup-all.sh: graceful takedown of the full fleet.
#
# Sends SIGTERM to all livepeer-* binaries plus the loop-staker-gateway.sh
# wrapper, polls up to 60 s for clean exit, then escalates to SIGKILL on
# anything still alive. Designed for clean DB snapshots / upgrades.
#
# Use: bash scripts/stop-all.sh
set -uo pipefail

# Match all livepeer binaries AND the bash loop wrapper.
PATTERN='livepeer-(api|daemon|staker|rollups|enricher|orchestrator|valuator|indexer|reorg|finality|seed-migrator|alert-bot)|loop-staker-gateway'

list() { pgrep -af "$PATTERN" 2>/dev/null | grep -v "$$" | grep -v stop-all; }

echo "=== BEFORE ==="
list || echo "  (none)"

echo
echo "=== SIGTERM ==="
pkill -TERM -f "$PATTERN" 2>/dev/null || true

for i in $(seq 1 60); do
  remaining=$(pgrep -f "$PATTERN" 2>/dev/null | grep -v "$$" | grep -v stop-all | wc -l)
  if [ "$remaining" -eq 0 ]; then
    echo "  all stopped after ${i}s"
    break
  fi
  sleep 1
done

# Anything still alive after 60s gets SIGKILL.
remaining=$(pgrep -f "$PATTERN" 2>/dev/null | grep -v "$$" | grep -v stop-all | wc -l)
if [ "$remaining" -gt 0 ]; then
  echo
  echo "=== SIGKILL holdouts ==="
  list
  pkill -KILL -f "$PATTERN" 2>/dev/null || true
  sleep 1
fi

echo
echo "=== AFTER ==="
list || echo "  (none)"
