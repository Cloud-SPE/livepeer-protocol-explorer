#!/usr/bin/env bash
# scripts/validate-vs-onchain.sh — TD-030 Phase A
#
# Sample N orchestrators + N gateways from the API, eth_call the
# corresponding contract methods, report any drift.
#
# IMPORTANT — about "drift":
#   Orchestrator total_stake in the API is a snapshot taken when the latest
#   round started (matview source: orch_stake_by_round). The on-chain value
#   from BondingManager.transcoderTotalStake is LIVE: as the orch earns
#   rewards mid-round (or delegators bond/unbond), the chain value drifts
#   above/below the snapshot by ~0.1-0.5% per day until the next round
#   triggers a fresh snapshot.
#
#   This script uses PERCENTAGE-based tolerance for stake (default 1.0%) so
#   normal in-round drift passes. Cuts and gateway balances stay strict
#   (cuts barely change; gateway deposits/reserves move on explicit user tx
#   so should match exactly).
#
# Requires: bash, curl, jq, python3, foundry's `cast`
#   (install foundry: `curl -L https://foundry.paradigm.xyz | bash && foundryup`)
#
# Usage:
#   bash scripts/validate-vs-onchain.sh
#   bash scripts/validate-vs-onchain.sh --orchs 50 --gateways 25
#   bash scripts/validate-vs-onchain.sh --api-url https://livepeer-api.xode.app
#   bash scripts/validate-vs-onchain.sh --orch-addr 0x525419...
#   bash scripts/validate-vs-onchain.sh --all-active   # validate every active orch + every gateway
#   bash scripts/validate-vs-onchain.sh --tolerance-stake-pct 0.5  # tighten stake tolerance to 0.5%
#
# Env:
#   ARCHIVE_RPC_URL   archive Arbitrum RPC (preferred)
#   CHAINSTACK_RPC_URL  fallback if ARCHIVE_RPC_URL unset
#
# Exit codes:
#   0  all sampled entities passed within tolerance
#   1  one or more DRIFT (out-of-tolerance) findings
#   2  invocation error (missing dependency, bad flag, API unreachable)

set -uo pipefail

# ── Defaults ─────────────────────────────────────────────────────────
API_URL="https://livepeer-api.xode.app"
ORCH_SAMPLE=20
GATEWAY_SAMPLE=10
TOLERANCE_STAKE_PCT="1.0"   # in-round reward accumulation can drift this much
TOLERANCE_ETH="0.000001"     # gateway balances move on explicit tx; should match
ALL_ACTIVE=false
SPECIFIC_ORCHS=()
SPECIFIC_GATEWAYS=()

# Arbitrum One contract addresses (from config/arbitrum.yaml)
BONDING_MANAGER="0x35Bcf3c30594191d53231E4FF333E8A770453e40"
TICKET_BROKER="0xa8bB618B1520E284046F3dFc448851A1Ff26e41B"

# ── Arg parsing ─────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --orchs)               ORCH_SAMPLE="$2"; shift 2 ;;
    --gateways)            GATEWAY_SAMPLE="$2"; shift 2 ;;
    --tolerance-stake-pct) TOLERANCE_STAKE_PCT="$2"; shift 2 ;;
    --tolerance-eth)       TOLERANCE_ETH="$2"; shift 2 ;;
    --api-url)       API_URL="$2"; shift 2 ;;
    --rpc-url)       ARCHIVE_RPC_URL="$2"; shift 2 ;;
    --orch-addr)     SPECIFIC_ORCHS+=("$2"); shift 2 ;;
    --gateway-addr)  SPECIFIC_GATEWAYS+=("$2"); shift 2 ;;
    --all-active)    ALL_ACTIVE=true; shift ;;
    -h|--help)
      sed -n '2,/^$/p' "$0" | sed 's/^# //; s/^#$//'
      exit 0
      ;;
    *) echo "unknown flag: $1" >&2; exit 2 ;;
  esac
done

RPC_URL="${ARCHIVE_RPC_URL:-${CHAINSTACK_RPC_URL:-}}"
if [[ -z "$RPC_URL" ]]; then
  echo "ERROR: set ARCHIVE_RPC_URL or CHAINSTACK_RPC_URL (e.g. via .env)." >&2
  exit 2
fi

# ── Dependency checks ───────────────────────────────────────────────
for tool in curl jq python3 cast; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "ERROR: '$tool' not found in PATH." >&2
    exit 2
  }
done

# ── Helpers ─────────────────────────────────────────────────────────

# Print a header bar
hr() { printf '%.0s─' {1..72}; echo; }

# Compare two decimal strings with an absolute tolerance. PASS=0, FAIL=1.
within_tol() {
  python3 -c "
import sys
api  = float('$1' or '0')
chain = float('$2' or '0')
tol  = float('$3')
sys.exit(0 if abs(api - chain) <= tol else 1)
"
}

# Compare two decimal strings with a percentage tolerance against the chain value.
# PASS=0, FAIL=1. Used for stake (which drifts in-round from rewards).
within_pct() {
  python3 -c "
import sys
api  = float('$1' or '0')
chain = float('$2' or '0')
pct  = float('$3')
if chain == 0:
    sys.exit(0 if api == 0 else 1)
diff_pct = abs(api - chain) / chain * 100
sys.exit(0 if diff_pct <= pct else 1)
"
}

# Compute drift % for display
drift_pct() {
  python3 -c "
api  = float('$1' or '0')
chain = float('$2' or '0')
if chain == 0: print('n/a'); exit()
print(f'{(api - chain) / chain * 100:+.3f}%')
"
}

# Convert a wei (uint256) string to a decimal LPT/ETH string with 18-dec precision.
wei_to_decimal() {
  python3 -c "
v = int('$1' or '0')
print(f'{v / 10**18:.18f}'.rstrip('0').rstrip('.') or '0')
"
}

# Convert raw cuts (millionths, e.g. 500000) to a percent string.
millionths_to_percent() {
  python3 -c "
v = int('$1' or '0')
print(f'{(v / 10000):.6f}'.rstrip('0').rstrip('.') or '0')
"
}

# Truncate address for display
short_addr() { echo "${1:0:8}…${1: -6}"; }

# ── Sample selection ────────────────────────────────────────────────

select_orchs() {
  local sample="$1"
  if [[ ${#SPECIFIC_ORCHS[@]} -gt 0 ]]; then
    printf '%s\n' "${SPECIFIC_ORCHS[@]}"
    return
  fi
  local query
  if $ALL_ACTIVE; then
    query="$API_URL/orchestrators?active_only=true&limit=200"
  else
    query="$API_URL/orchestrators?active_only=true&limit=200"
  fi
  local addrs
  addrs=$(curl -sSf -m 10 "$query" | jq -r '.data[].address')
  if [[ -z "$addrs" ]]; then
    echo "ERROR: no orchestrators returned from $query" >&2
    return 1
  fi
  if $ALL_ACTIVE; then
    echo "$addrs"
  else
    echo "$addrs" | shuf | head -n "$sample"
  fi
}

select_gateways() {
  local sample="$1"
  if [[ ${#SPECIFIC_GATEWAYS[@]} -gt 0 ]]; then
    printf '%s\n' "${SPECIFIC_GATEWAYS[@]}"
    return
  fi
  local query="$API_URL/gateways?limit=100"
  local addrs
  addrs=$(curl -sSf -m 10 "$query" | jq -r '.data[].address')
  if [[ -z "$addrs" ]]; then
    echo "ERROR: no gateways returned from $query" >&2
    return 1
  fi
  if $ALL_ACTIVE; then
    echo "$addrs"
  else
    echo "$addrs" | shuf | head -n "$sample"
  fi
}

# ── Per-entity validators ───────────────────────────────────────────

# Returns 0 if all checks pass, 1 if any DRIFT, 2 if API/RPC error.
# Prints one PASS or FAIL line.
validate_orch() {
  local addr="$1"
  local label
  label=$(short_addr "$addr")

  # API side
  local api_json
  api_json=$(curl -sSf -m 10 "$API_URL/orchestrators/$addr" 2>/dev/null) || {
    printf '  ERROR  %s  api fetch failed\n' "$label"
    return 2
  }
  local api_stake api_reward_cut api_fee_share api_active
  api_stake=$(echo "$api_json"      | jq -r '.total_stake')
  api_reward_cut=$(echo "$api_json" | jq -r '.reward_cut_percent')
  api_fee_share=$(echo "$api_json"  | jq -r '.fee_share_percent')
  api_active=$(echo "$api_json"     | jq -r '.is_active')

  # On-chain side
  local stake_raw transcoder_raw status_raw
  stake_raw=$(cast call "$BONDING_MANAGER" "transcoderTotalStake(address)(uint256)" "$addr" --rpc-url "$RPC_URL" 2>/dev/null) || {
    printf '  ERROR  %s  rpc transcoderTotalStake failed\n' "$label"
    return 2
  }
  # Strip cast's optional `[1.23e21]` suffix on integer outputs
  stake_raw="${stake_raw%% *}"

  # getTranscoder returns 10 uints; we need fields 1 (rewardCut) and 2 (feeShare)
  transcoder_raw=$(cast call "$BONDING_MANAGER" \
    "getTranscoder(address)(uint256,uint256,uint256,uint256,uint256,uint256,uint256,uint256,uint256,uint256)" \
    "$addr" --rpc-url "$RPC_URL" 2>/dev/null) || {
    printf '  ERROR  %s  rpc getTranscoder failed\n' "$label"
    return 2
  }
  # cast returns one value per line for tuple returns
  local reward_cut_raw fee_share_raw
  reward_cut_raw=$(echo "$transcoder_raw" | sed -n '2p' | awk '{print $1}')
  fee_share_raw=$(echo  "$transcoder_raw" | sed -n '3p' | awk '{print $1}')

  # transcoderStatus returns enum: 0=NotRegistered, 1=Registered
  status_raw=$(cast call "$BONDING_MANAGER" "transcoderStatus(address)(uint8)" "$addr" --rpc-url "$RPC_URL" 2>/dev/null) || {
    printf '  ERROR  %s  rpc transcoderStatus failed\n' "$label"
    return 2
  }
  status_raw="${status_raw%% *}"

  # Convert
  local chain_stake chain_reward_cut chain_fee_share chain_active
  chain_stake=$(wei_to_decimal "$stake_raw")
  chain_reward_cut=$(millionths_to_percent "$reward_cut_raw")
  chain_fee_share=$(millionths_to_percent "$fee_share_raw")
  if [[ "$status_raw" == "1" ]]; then chain_active="true"; else chain_active="false"; fi

  # Compare
  local fail=0
  local notes=()

  if ! within_pct "$api_stake" "$chain_stake" "$TOLERANCE_STAKE_PCT"; then
    local pct
    pct=$(drift_pct "$api_stake" "$chain_stake")
    notes+=("stake api=$api_stake chain=$chain_stake (Δ$pct)")
    fail=1
  fi
  if ! within_tol "$api_reward_cut" "$chain_reward_cut" "0.001"; then
    notes+=("reward_cut api=$api_reward_cut chain=$chain_reward_cut")
    fail=1
  fi
  if ! within_tol "$api_fee_share" "$chain_fee_share" "0.001"; then
    notes+=("fee_share api=$api_fee_share chain=$chain_fee_share")
    fail=1
  fi
  if [[ "$api_active" != "$chain_active" ]]; then
    notes+=("is_active api=$api_active chain=$chain_active")
    fail=1
  fi

  if [[ $fail -eq 0 ]]; then
    printf '  PASS   %s  stake=%s cuts=%s/%s active=%s\n' \
      "$label" "$chain_stake" "$chain_reward_cut" "$chain_fee_share" "$chain_active"
    return 0
  else
    printf '  FAIL   %s  DRIFT: %s\n' "$label" "$(IFS='; '; echo "${notes[*]}")"
    return 1
  fi
}

# Returns 0 if all checks pass, 1 if any DRIFT, 2 if API/RPC error.
validate_gateway() {
  local addr="$1"
  local label
  label=$(short_addr "$addr")

  # API side
  local api_json
  api_json=$(curl -sSf -m 10 "$API_URL/gateways/$addr/profile" 2>/dev/null) || {
    printf '  ERROR  %s  api fetch failed\n' "$label"
    return 2
  }
  local api_deposit api_reserve api_unlock
  api_deposit=$(echo "$api_json" | jq -r '.latest_deposit')
  api_reserve=$(echo "$api_json" | jq -r '.latest_reserve')
  api_unlock=$(echo "$api_json"  | jq -r '.unlock_in_progress')

  # On-chain side
  # getSenderInfo returns ((uint256 deposit, uint256 withdrawRound), (uint256 fundsRemaining, uint256 claimedInCurrentRound))
  local sender_info
  sender_info=$(cast call "$TICKET_BROKER" \
    "getSenderInfo(address)((uint256,uint256),(uint256,uint256))" \
    "$addr" --rpc-url "$RPC_URL" 2>/dev/null) || {
    printf '  ERROR  %s  rpc getSenderInfo failed\n' "$label"
    return 2
  }
  # cast returns nested structs as: "(deposit, withdrawRound)" then "(fundsRemaining, claimedInCurrentRound)"
  # Parse with python for robustness
  local parsed
  parsed=$(python3 -c "
import re
s = '''$sender_info'''
nums = re.findall(r'\d+', s)
# Expect 4 numbers: deposit, withdrawRound, fundsRemaining, claimedInCurrentRound
print(' '.join(nums[:4]))
")
  read -r deposit_raw withdraw_round_raw reserve_raw _ <<< "$parsed"

  local chain_deposit chain_reserve chain_unlock
  chain_deposit=$(wei_to_decimal "$deposit_raw")
  chain_reserve=$(wei_to_decimal "$reserve_raw")
  if [[ "$withdraw_round_raw" == "0" ]] || [[ -z "$withdraw_round_raw" ]]; then
    chain_unlock="false"
  else
    chain_unlock="true"
  fi

  local fail=0
  local notes=()

  if ! within_tol "$api_deposit" "$chain_deposit" "$TOLERANCE_ETH"; then
    notes+=("deposit api=$api_deposit chain=$chain_deposit")
    fail=1
  fi
  if ! within_tol "$api_reserve" "$chain_reserve" "$TOLERANCE_ETH"; then
    notes+=("reserve api=$api_reserve chain=$chain_reserve")
    fail=1
  fi
  if [[ "$api_unlock" != "$chain_unlock" ]]; then
    notes+=("unlock_in_progress api=$api_unlock chain=$chain_unlock")
    fail=1
  fi

  if [[ $fail -eq 0 ]]; then
    printf '  PASS   %s  deposit=%s reserve=%s unlock=%s\n' \
      "$label" "$chain_deposit" "$chain_reserve" "$chain_unlock"
    return 0
  else
    printf '  FAIL   %s  DRIFT: %s\n' "$label" "$(IFS='; '; echo "${notes[*]}")"
    return 1
  fi
}

# ── Main ────────────────────────────────────────────────────────────

started_at=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
echo "=== validate-vs-onchain @ $started_at ==="
echo "API:  $API_URL"
echo "RPC:  ${RPC_URL%/*}/<key-redacted>"
echo "Tolerance: stake ±${TOLERANCE_STAKE_PCT}% (in-round reward drift expected), gateway balance ±${TOLERANCE_ETH} ETH"
echo

orch_pass=0; orch_fail=0; orch_err=0
gw_pass=0;   gw_fail=0;   gw_err=0

if [[ "$ORCH_SAMPLE" -gt 0 ]] || [[ ${#SPECIFIC_ORCHS[@]} -gt 0 ]] || $ALL_ACTIVE; then
  echo "--- ORCHESTRATORS ---"
  while IFS= read -r addr; do
    [[ -z "$addr" ]] && continue
    validate_orch "$addr"
    case $? in
      0) ((orch_pass++)) ;;
      1) ((orch_fail++)) ;;
      2) ((orch_err++)) ;;
    esac
  done < <(select_orchs "$ORCH_SAMPLE" || echo "")
  hr
fi

if [[ "$GATEWAY_SAMPLE" -gt 0 ]] || [[ ${#SPECIFIC_GATEWAYS[@]} -gt 0 ]] || $ALL_ACTIVE; then
  echo "--- GATEWAYS ---"
  while IFS= read -r addr; do
    [[ -z "$addr" ]] && continue
    validate_gateway "$addr"
    case $? in
      0) ((gw_pass++)) ;;
      1) ((gw_fail++)) ;;
      2) ((gw_err++)) ;;
    esac
  done < <(select_gateways "$GATEWAY_SAMPLE" || echo "")
  hr
fi

ended_at=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
echo "=== SUMMARY @ $ended_at ==="
printf '  Orchestrators: %d PASS, %d FAIL, %d ERROR\n' "$orch_pass" "$orch_fail" "$orch_err"
printf '  Gateways:      %d PASS, %d FAIL, %d ERROR\n' "$gw_pass" "$gw_fail" "$gw_err"

# Exit non-zero on any DRIFT or invocation error
if [[ $((orch_fail + gw_fail + orch_err + gw_err)) -gt 0 ]]; then
  exit 1
fi
exit 0
