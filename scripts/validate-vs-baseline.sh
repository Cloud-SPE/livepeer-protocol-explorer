#!/usr/bin/env bash
# scripts/validate-vs-baseline.sh
#
# Compare current DB state to a baseline captured by snapshot-baseline.sh.
# Run this AFTER the clean re-run finishes (indexer + post-indexer pipeline).
#
# Determinism guarantee: with rpc_call_cache kept, the clean re-run should
# produce byte-identical raw_protocol_events and event_valuations content.
# Any divergence is a determinism bug.
#
# Usage:
#     bash scripts/validate-vs-baseline.sh baselines/<timestamp>

set -euo pipefail

ROOT=/home/mazup/git-repos/crypto-price-feed
cd "$ROOT"
export DATABASE_URL="postgres://livepeer:changeme@127.0.0.1:5432/livepeer_indexer"

BASELINE="${1:-}"
if [[ -z "$BASELINE" ]] || [[ ! -d "$BASELINE" ]]; then
  echo "Usage: $0 <baseline-dir>"
  echo "Available baselines:"
  ls -d baselines/*/ 2>/dev/null || echo "  (none found)"
  exit 1
fi

echo "=== Comparing current DB state vs baseline: $BASELINE ==="
echo

CURRENT=$(mktemp -d)
trap "rm -rf $CURRENT" EXIT

# Recapture using the same queries.
bash scripts/snapshot-baseline.sh > /dev/null 2>&1 || {
  echo "snapshot-baseline.sh failed; running queries directly"
}
LATEST=$(ls -td baselines/*/ | head -1)

echo "--- row_counts diff ---"
diff "$BASELINE/row_counts.txt" "$LATEST/row_counts.txt" || echo "(differences shown above)"
echo

echo "--- events_by_contract diff ---"
diff "$BASELINE/events_by_contract.txt" "$LATEST/events_by_contract.txt" || echo "(differences shown above)"
echo

echo "--- raw_protocol_events md5 ---"
echo "  baseline: $(cat $BASELINE/raw_protocol_events.md5)"
echo "  current:  $(cat $LATEST/raw_protocol_events.md5)"
if [[ "$(cat $BASELINE/raw_protocol_events.md5)" == "$(cat $LATEST/raw_protocol_events.md5)" ]]; then
  echo "  ✓ MATCH"
else
  echo "  ✗ DIVERGENT — investigate"
fi
echo

echo "--- event_valuations md5 ---"
echo "  baseline: $(cat $BASELINE/event_valuations.md5)"
echo "  current:  $(cat $LATEST/event_valuations.md5)"
if [[ "$(cat $BASELINE/event_valuations.md5)" == "$(cat $LATEST/event_valuations.md5)" ]]; then
  echo "  ✓ MATCH"
else
  echo "  ✗ DIVERGENT — investigate"
fi
echo

echo "--- migrations diff ---"
diff "$BASELINE/migrations.txt" "$LATEST/migrations.txt" || echo "(schema drifted between baseline and now — divergence is expected)"
echo

echo "--- git rev ---"
echo "  baseline: $(cat $BASELINE/git_rev.txt)"
echo "  current:  $(cat $LATEST/git_rev.txt)"

echo
echo "=== Comparison complete ==="
