#!/usr/bin/env bash
# scripts/full-run-parallel-patch.sh
#
# Launch a parallel "patch" pass that re-scans genesis -> head with the
# expanded event set (TranscoderSlashed, ReserveClaimed, UnlockCancelled,
# VoteCastWithParams, ProposalCanceled/Queued, ParameterUpdate, SetController).
# Uses --checkpoint-suffix patch so its checkpoints (indexer_<C>_patch) never
# collide with the live-run checkpoints (indexer_<C>). Inserts go through
# ON CONFLICT DO NOTHING so existing live-run rows stay canonical and only
# new event-type rows get added.
#
# Skipped contracts:
#   - livepeer-token: only missing events are MintFinished/OwnershipTransferred,
#     both pure admin — not worth re-scanning the densest contract for.
#
# Usage:
#     bash scripts/full-run-parallel-patch.sh

set -uo pipefail

ROOT=/home/mazup/git-repos/crypto-price-feed
cd "$ROOT"
mkdir -p logs

set -a; source .env; set +a
export DATABASE_URL="postgres://livepeer:changeme@127.0.0.1:5432/livepeer_indexer"
export STATIC_CONFIG="$ROOT/config/arbitrum.yaml"
export ENV_CONFIG="$ROOT/config/env/dev.yaml"

GENESIS_BLOCK=6072093

HEAD_HEX=$(curl -s --max-time 10 -X POST -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
  "$CHAINSTACK_RPC_URL" | python3 -c "import sys, json; print(json.load(sys.stdin)['result'])")
HEAD=$((HEAD_HEX))
TO_BLOCK=$((HEAD - 50))

echo "L2 head: $HEAD"
echo "TO_BLOCK: $TO_BLOCK"
echo "Span:    $((TO_BLOCK - GENESIS_BLOCK)) blocks"
echo

for contract in bonding-manager ticket-broker rounds-manager governor; do
    LOG="logs/indexer-${contract}-patch.log"
    echo "  launching $contract patch  (log: $LOG)"
    nohup target/release/livepeer-indexer backfill \
        --contract "$contract" \
        --from-block "$GENESIS_BLOCK" \
        --to-block "$TO_BLOCK" \
        --checkpoint-suffix patch \
        >> "$LOG" 2>&1 < /dev/null &
    disown
    echo "    PID $!"
done

echo
echo "patch indexers launched. Monitor with:"
echo "  bash scripts/run-status.sh"
echo "  tail -F logs/indexer-bonding-manager-patch.log"
echo
echo "Patch checkpoints: indexer_<Contract>_patch (separate from live)"
