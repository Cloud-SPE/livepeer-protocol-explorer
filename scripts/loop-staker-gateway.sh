#!/usr/bin/env bash
# Loop staker gateway-backfill in three independently-checkpointed phases
# (flow / claimant / balance). Idempotent + resumable via indexer_checkpoints.
#
# Stable detection requires BOTH:
#   - the binary exited 0 (no transient RPC failure / panic)
#   - all three candidate counters returned 0
# A non-zero exit (HTTP 429 from Chainstack, network blip, etc.) is treated
# as a transient error: sleep RETRY_SLEEP_SECS, then loop. Without this
# distinction the wrapper used to interpret a crashed iteration's empty
# output as "stable" and exit, leaving gateway-backfill stuck. (Observed
# on iter 787 of the 2026-05-07/08 run.)
#
# Once truly stable, the wrapper sleeps IDLE_SLEEP_SECS and re-checks
# rather than exiting outright (matches the profile-follow / rollup
# follow-mode pattern so live operation keeps tracking new events).
set -uo pipefail
cd /home/mazup/git-repos/crypto-price-feed
set -a; source .env; set +a
export DATABASE_URL="postgres://livepeer:changeme@127.0.0.1:5432/livepeer_indexer"
export STATIC_CONFIG="$PWD/config/arbitrum.yaml"
export ENV_CONFIG="$PWD/config/env/dev.yaml"

IDLE_SLEEP_SECS="${IDLE_SLEEP_SECS:-300}"
RETRY_SLEEP_SECS="${RETRY_SLEEP_SECS:-30}"

iter=0
while true; do
  iter=$((iter + 1))
  echo "-- iter $iter $(date -u +%H:%M:%SZ)"
  out=$(target/release/livepeer-staker gateway-backfill 2>&1)
  rc=$?
  echo "$out" | tail -3

  if [ $rc -ne 0 ]; then
    echo "-- iter $iter staker exit=$rc — transient error; sleeping ${RETRY_SLEEP_SECS}s and retrying"
    sleep "$RETRY_SLEEP_SECS"
    continue
  fi

  flow=$(echo "$out" | grep -oE 'flow_candidates_seen":[0-9]+' | grep -oE '[0-9]+' | tail -1)
  claim=$(echo "$out" | grep -oE 'claimant_candidates_seen":[0-9]+' | grep -oE '[0-9]+' | tail -1)
  bal=$(echo "$out" | grep -oE 'balance_candidates_seen":[0-9]+' | grep -oE '[0-9]+' | tail -1)
  flow=${flow:-0}
  claim=${claim:-0}
  bal=${bal:-0}
  echo "-- iter $iter flow=$flow claim=$claim bal=$bal"
  if [ "$flow" = "0" ] && [ "$claim" = "0" ] && [ "$bal" = "0" ]; then
    echo "-- gateway-backfill stable at iter $iter; sleeping ${IDLE_SLEEP_SECS}s before recheck"
    sleep "$IDLE_SLEEP_SECS"
  fi
done
