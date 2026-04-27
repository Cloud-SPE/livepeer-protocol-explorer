#!/usr/bin/env bash
# scripts/full-run-parallel.sh
#
# Launch 5 indexer processes in parallel, one per contract. Each writes to
# its own log file under logs/. Per-contract checkpoints prevent collisions
# (indexer_BondingManager, indexer_TicketBroker, ...). Each process resumes
# from its own checkpoint independently.
#
# Resource impact (with 5 parallel):
#   ~5-10 RPS to Chainstack peak (well under your 400 RPS cap)
#   memory: trivial × 5
#   wall-clock: roughly the slowest single contract instead of the sum
#
# Usage:
#     bash scripts/full-run-parallel.sh
#     # then use scripts/run-status.sh to monitor; tail logs/indexer-<contract>.log
#
# When ALL 5 finish (status script will show "DONE" on each), invoke:
#     bash scripts/full-run-post-indexer.sh

set -uo pipefail

ROOT=/home/mazup/git-repos/crypto-price-feed
cd "$ROOT"
mkdir -p logs

set -a; source .env; set +a
export DATABASE_URL="postgres://livepeer:changeme@127.0.0.1:5432/livepeer_indexer"
export STATIC_CONFIG="$ROOT/config/arbitrum.yaml"
export ENV_CONFIG="$ROOT/config/env/dev.yaml"

GENESIS_BLOCK=6072093

# Pick TO_BLOCK from current chain head.
HEAD_HEX=$(curl -s --max-time 10 -X POST -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
  "$CHAINSTACK_RPC_URL" | python3 -c "import sys, json; print(json.load(sys.stdin)['result'])")
HEAD=$((HEAD_HEX))
TO_BLOCK=$((HEAD - 50))

echo "L2 head: $HEAD"
echo "TO_BLOCK: $TO_BLOCK"
echo "Span:    $((TO_BLOCK - GENESIS_BLOCK)) blocks"
echo

for contract in bonding-manager ticket-broker livepeer-token rounds-manager governor; do
    LOG="logs/indexer-${contract}.log"
    echo "  launching $contract  (log: $LOG)"
    nohup target/release/livepeer-indexer backfill \
        --contract "$contract" \
        --from-block "$GENESIS_BLOCK" \
        --to-block "$TO_BLOCK" \
        >> "$LOG" 2>&1 < /dev/null &
    disown
    echo "    PID $!"
done

echo
echo "all 5 indexers launched. Monitor with:"
echo "  bash scripts/run-status.sh"
echo "  tail -F logs/indexer-bonding-manager.log"
echo
echo "When all 5 finish, run scripts/full-run-post-indexer.sh"
