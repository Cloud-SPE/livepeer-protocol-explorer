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
#   bash scripts/validate-vs-onchain.sh --per-call-delay 0.1       # ~10 req/s self-throttle (Infura free tier)
#   bash scripts/validate-vs-onchain.sh --validate-tickets         # per-gateway ticket count vs chain (last ~8h)
#   bash scripts/validate-vs-onchain.sh --validate-tickets --tickets-window-blocks 200000  # widen to ~16h window
#
# Env (priority order: PRIMARY = INFURA_RPC_URL → ARCHIVE_RPC_URL → CHAINSTACK_RPC_URL;
# FALLBACK = CHAINSTACK_RPC_URL → ARCHIVE_RPC_URL → INFURA_RPC_URL):
#   INFURA_RPC_URL      preferred archive (low RPS quota — falls back on 429 / transient errors)
#   ARCHIVE_RPC_URL     generic archive RPC
#   CHAINSTACK_RPC_URL  legacy / fallback archive RPC
#
# Behavior: every cast call tries PRIMARY first. If the response matches a
# rate-limit / transient error pattern, the same call is retried once
# against FALLBACK. Counts of (primary_ok, fallback_used, failed) are
# printed in the summary so you can see how often Infura got pushed back.
#
# Exit codes:
#   0  all sampled entities passed within tolerance
#   1  one or more DRIFT (out-of-tolerance) findings
#   2  invocation error (missing dependency, bad flag, API unreachable)

set -uo pipefail

# ── Defaults ─────────────────────────────────────────────────────────
# Versioned business endpoints live at $API_URL/api/v1/...
# Pass `--api-url` to override the host; the /api/v1 prefix is appended below.
API_URL="https://livepeer-api.xode.app"
ORCH_SAMPLE=20
GATEWAY_SAMPLE=10
TOLERANCE_STAKE_PCT="1.0"   # in-round reward accumulation can drift this much
TOLERANCE_ETH="0.000001"     # gateway balances move on explicit tx; should match
VALIDATE_TICKETS=false       # opt-in: per-gateway ticket-count sanity vs chain
TICKETS_WINDOW_BLOCKS=100000 # ~8h on Arbitrum; single chunk under Chainstack's 500k cap
# Inter-call self-throttle (seconds). Default 0 = no sleep. Set to e.g.
# `0.1` to cap at ~10 req/s against the PRIMARY RPC, useful when Infura's
# free-tier per-second budget is the bottleneck on `--all-active` runs.
PER_CALL_DELAY="0"
ALL_ACTIVE=false
SPECIFIC_ORCHS=()
SPECIFIC_GATEWAYS=()

# Arbitrum One contract addresses (from config/arbitrum.yaml)
BONDING_MANAGER="0x35Bcf3c30594191d53231E4FF333E8A770453e40"
TICKET_BROKER="0xa8bB618B1520E284046F3dFc448851A1Ff26e41B"
ROUNDS_MANAGER="0xdd6f56DcC28D3F5f27084381fE8Df634985cc39f"
LIVEPEER_TOKEN="0x289ba1701C2F088cf0faf8B3705246331cB8A839"
TOPIC_WINNING_TICKET="0xc389eb51ed006dbf2528507f010efdf5225ea596e1e1741d74f550dab1925ee7"

# ── Arg parsing ─────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --orchs)               ORCH_SAMPLE="$2"; shift 2 ;;
    --gateways)            GATEWAY_SAMPLE="$2"; shift 2 ;;
    --tolerance-stake-pct) TOLERANCE_STAKE_PCT="$2"; shift 2 ;;
    --tolerance-eth)       TOLERANCE_ETH="$2"; shift 2 ;;
    --api-url)       API_URL="$2"; shift 2 ;;
    --rpc-url|--primary-rpc) PRIMARY_RPC_OVERRIDE="$2"; shift 2 ;;
    --fallback-rpc)          FALLBACK_RPC_OVERRIDE="$2"; shift 2 ;;
    --per-call-delay)        PER_CALL_DELAY="$2"; shift 2 ;;
    --validate-tickets)      VALIDATE_TICKETS=true; shift ;;
    --tickets-window-blocks) TICKETS_WINDOW_BLOCKS="$2"; shift 2 ;;
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

# Strip trailing slash on the host, then append the versioned prefix.
API_URL="${API_URL%/}"
API_BASE="$API_URL/api/v1"

# Resolve the two RPC endpoints. PRIMARY runs first; FALLBACK is the safety
# net used only when PRIMARY returns a rate-limit / transient error.
PRIMARY_RPC_URL="${PRIMARY_RPC_OVERRIDE:-${INFURA_RPC_URL:-${ARCHIVE_RPC_URL:-${CHAINSTACK_RPC_URL:-}}}}"
FALLBACK_RPC_URL="${FALLBACK_RPC_OVERRIDE:-${CHAINSTACK_RPC_URL:-${ARCHIVE_RPC_URL:-${INFURA_RPC_URL:-}}}}"

if [[ -z "$PRIMARY_RPC_URL" ]]; then
  echo "ERROR: set INFURA_RPC_URL, ARCHIVE_RPC_URL, or CHAINSTACK_RPC_URL (e.g. via .env)." >&2
  exit 2
fi
# If only one URL is configured, both pointers resolve to it — fallback is a no-op.
[[ -z "$FALLBACK_RPC_URL" ]] && FALLBACK_RPC_URL="$PRIMARY_RPC_URL"

# Per-call counters live on disk so they survive the `$(...)` subshells that
# wrap every cast invocation. Single-byte appends are atomic on POSIX.
RPC_STATS_DIR=$(mktemp -d -t validate-vs-onchain.XXXXXX)
RPC_PRIMARY_HITS="$RPC_STATS_DIR/primary"
RPC_FALLBACK_HITS="$RPC_STATS_DIR/fallback"
RPC_FAILED_HITS="$RPC_STATS_DIR/failed"
: > "$RPC_PRIMARY_HITS" > "$RPC_FALLBACK_HITS" > "$RPC_FAILED_HITS"
trap 'rm -rf "$RPC_STATS_DIR"' EXIT

# Rate-limit / transient signatures across major archive providers (Infura,
# Chainstack, Alchemy, generic). Match against stderr from the failed call.
is_fallback_worthy_error() {
  grep -qiE 'too many requests|rate limit|429|-32005|-32016|project id|daily request count|timeout|timed out|connection refused|temporarily unavailable|service unavailable|^.*5[0-9][0-9].*$|-32603|exhaust' <<<"$1"
}

# Wrap `cast call` with primary→fallback retry on rate-limit / transient
# errors. Forwards all args verbatim; the helper appends `--rpc-url`.
cast_call_with_fallback() {
  local tmp_out tmp_err err rc
  tmp_out=$(mktemp); tmp_err=$(mktemp)
  cast call "$@" --rpc-url "$PRIMARY_RPC_URL" >"$tmp_out" 2>"$tmp_err"
  rc=$?
  if [[ $rc -eq 0 ]]; then
    printf x >>"$RPC_PRIMARY_HITS"
    cat "$tmp_out"; rm -f "$tmp_out" "$tmp_err"
    [[ "$PER_CALL_DELAY" != "0" ]] && sleep "$PER_CALL_DELAY"
    return 0
  fi
  err=$(cat "$tmp_err")
  if [[ "$PRIMARY_RPC_URL" != "$FALLBACK_RPC_URL" ]] && is_fallback_worthy_error "$err"; then
    cast call "$@" --rpc-url "$FALLBACK_RPC_URL" >"$tmp_out" 2>"$tmp_err"
    rc=$?
    if [[ $rc -eq 0 ]]; then
      printf x >>"$RPC_FALLBACK_HITS"
      cat "$tmp_out"; rm -f "$tmp_out" "$tmp_err"
      [[ "$PER_CALL_DELAY" != "0" ]] && sleep "$PER_CALL_DELAY"
      return 0
    fi
  fi
  printf x >>"$RPC_FAILED_HITS"
  rm -f "$tmp_out" "$tmp_err"
  return 1
}

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
    query="$API_BASE/orchestrators?active_only=true&limit=200"
  else
    query="$API_BASE/orchestrators?active_only=true&limit=200"
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
  local query="$API_BASE/gateways?limit=100"
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
  api_json=$(curl -sSf -m 10 "$API_BASE/orchestrators/$addr" 2>/dev/null) || {
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
  stake_raw=$(cast_call_with_fallback "$BONDING_MANAGER" "transcoderTotalStake(address)(uint256)" "$addr") || {
    printf '  ERROR  %s  rpc transcoderTotalStake failed\n' "$label"
    return 2
  }
  # Strip cast's optional `[1.23e21]` suffix on integer outputs
  stake_raw="${stake_raw%% *}"

  # getTranscoder returns 10 uints; we need fields 1 (rewardCut) and 2 (feeShare)
  transcoder_raw=$(cast_call_with_fallback "$BONDING_MANAGER" \
    "getTranscoder(address)(uint256,uint256,uint256,uint256,uint256,uint256,uint256,uint256,uint256,uint256)" \
    "$addr") || {
    printf '  ERROR  %s  rpc getTranscoder failed\n' "$label"
    return 2
  }
  # cast returns one value per line for tuple returns
  local reward_cut_raw fee_share_raw
  reward_cut_raw=$(echo "$transcoder_raw" | sed -n '2p' | awk '{print $1}')
  fee_share_raw=$(echo  "$transcoder_raw" | sed -n '3p' | awk '{print $1}')

  # transcoderStatus returns enum: 0=NotRegistered, 1=Registered
  status_raw=$(cast_call_with_fallback "$BONDING_MANAGER" "transcoderStatus(address)(uint8)" "$addr") || {
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
  api_json=$(curl -sSf -m 10 "$API_BASE/gateways/$addr/profile" 2>/dev/null) || {
    printf '  ERROR  %s  api fetch failed\n' "$label"
    return 2
  }
  local api_deposit api_reserve api_unlock
  api_deposit=$(echo "$api_json" | jq -r '.latest_deposit')
  api_reserve=$(echo "$api_json" | jq -r '.latest_reserve')
  api_unlock=$(echo "$api_json"  | jq -r '.unlock_in_progress')

  # On-chain side
  # getSenderInfo returns ((uint256 deposit, uint256 withdrawRound), (uint256 fundsRemaining, uint256 claimedInCurrentRound))
  # Use --json so the output is unambiguous (no scientific-notation annotations to misparse).
  local sender_info
  sender_info=$(cast_call_with_fallback "$TICKET_BROKER" \
    "getSenderInfo(address)((uint256,uint256),(uint256,uint256))" \
    "$addr" --json) || {
    printf '  ERROR  %s  rpc getSenderInfo failed\n' "$label"
    return 2
  }
  # --json output: [[deposit, withdrawRound], [fundsRemaining, claimedInCurrentRound]]
  local parsed
  parsed=$(python3 -c "
import json
[[deposit, withdraw], [reserve, _claimed]] = json.loads('''$sender_info''')
print(deposit, withdraw, reserve)
")
  read -r deposit_raw withdraw_round_raw reserve_raw <<< "$parsed"

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

# ── Tier 3 helper: cast logs with primary→fallback retry ────────────
cast_logs_with_fallback() {
  local tmp_out tmp_err err rc
  tmp_out=$(mktemp); tmp_err=$(mktemp)
  cast logs "$@" --rpc-url "$PRIMARY_RPC_URL" >"$tmp_out" 2>"$tmp_err"
  rc=$?
  if [[ $rc -eq 0 ]]; then
    printf x >>"$RPC_PRIMARY_HITS"
    cat "$tmp_out"; rm -f "$tmp_out" "$tmp_err"
    [[ "$PER_CALL_DELAY" != "0" ]] && sleep "$PER_CALL_DELAY"
    return 0
  fi
  err=$(cat "$tmp_err")
  if [[ "$PRIMARY_RPC_URL" != "$FALLBACK_RPC_URL" ]] && is_fallback_worthy_error "$err"; then
    cast logs "$@" --rpc-url "$FALLBACK_RPC_URL" >"$tmp_out" 2>"$tmp_err"
    rc=$?
    if [[ $rc -eq 0 ]]; then
      printf x >>"$RPC_FALLBACK_HITS"
      cat "$tmp_out"; rm -f "$tmp_out" "$tmp_err"
      [[ "$PER_CALL_DELAY" != "0" ]] && sleep "$PER_CALL_DELAY"
      return 0
    fi
  fi
  printf x >>"$RPC_FAILED_HITS"
  rm -f "$tmp_out" "$tmp_err"
  return 1
}

# ── Tier 3: per-gateway ticket-count vs chain ────────────────────────
# Validates ticket-ingestion path beyond the deposit-balance check. Walks
# WinningTicketRedeemed events sourced by the gateway over a recent window
# and compares to API's gateway-tickets endpoint filtered to the same range.
#
# Window: gateway's `as_of_block` minus TICKETS_WINDOW_BLOCKS (~8h default).
# Bounded so the single `cast logs` call stays under provider range limits.
validate_gateway_tickets() {
  local addr="$1"
  local label
  label=$(short_addr "$addr")

  local api_profile as_of_block
  api_profile=$(curl -sSf -m 10 "$API_BASE/gateways/$addr/profile" 2>/dev/null) || {
    printf '  ERROR  %s  tickets: api profile fetch failed\n' "$label"
    return 2
  }
  as_of_block=$(echo "$api_profile" | jq -r '.as_of_block // empty')
  if [[ -z "$as_of_block" || "$as_of_block" == "null" ]]; then
    printf '  ERROR  %s  tickets: gateway has no as_of_block\n' "$label"
    return 2
  fi

  local from_block=$((as_of_block - TICKETS_WINDOW_BLOCKS))
  local padded="0x000000000000000000000000${addr#0x}"

  # Chain: count WinningTicketRedeemed events with sender=gateway in window.
  local chain_logs chain_count
  chain_logs=$(cast_logs_with_fallback --from-block "$from_block" --to-block "$as_of_block" \
    --address "$TICKET_BROKER" "$TOPIC_WINNING_TICKET" "$padded" 2>/dev/null) || {
    printf '  ERROR  %s  tickets: cast logs failed\n' "$label"
    return 2
  }
  if [[ -z "$chain_logs" ]]; then
    chain_count=0
  else
    chain_count=$(grep -c '^- address' <<<"$chain_logs")
  fi

  # API: fetch up to 1000 latest tickets, filter to block window client-side.
  local api_json api_count
  api_json=$(curl -sSf -m 10 "$API_BASE/gateways/$addr/tickets?limit=1000" 2>/dev/null) || {
    printf '  ERROR  %s  tickets: api fetch failed\n' "$label"
    return 2
  }
  api_count=$(echo "$api_json" | jq --argjson f "$from_block" --argjson t "$as_of_block" \
    '[.data[] | select((.block_number|tonumber) >= $f and (.block_number|tonumber) <= $t)] | length')

  if [[ "$chain_count" == "$api_count" ]]; then
    printf '  PASS   %s  tickets[%d-%d] chain=%d api=%d\n' \
      "$label" "$from_block" "$as_of_block" "$chain_count" "$api_count"
    return 0
  else
    printf '  FAIL   %s  tickets[%d-%d] chain=%d api=%d (Δ=%d)\n' \
      "$label" "$from_block" "$as_of_block" "$chain_count" "$api_count" $((chain_count - api_count))
    return 1
  fi
}

# ── Main ────────────────────────────────────────────────────────────

started_at=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
echo "=== validate-vs-onchain @ $started_at ==="
echo "API:  $API_URL"
echo "RPC:  primary  = ${PRIMARY_RPC_URL%/*}/<key-redacted>"
echo "      fallback = ${FALLBACK_RPC_URL%/*}/<key-redacted>"
echo "Tolerance: stake ±${TOLERANCE_STAKE_PCT}% (in-round reward drift expected), gateway balance ±${TOLERANCE_ETH} ETH"
echo

orch_pass=0; orch_fail=0; orch_err=0
gw_pass=0;   gw_fail=0;   gw_err=0
agg_pass=0;  agg_fail=0;  agg_err=0
tx_pass=0;   tx_fail=0;   tx_err=0   # ticket-history (Tier 3)

# ── Tier 1: Aggregate sanity ────────────────────────────────────────
# Chain ↔ API totals. Four cheap eth_calls catch indexer-stalled,
# matview-stuck, and total-bonded drift in one pass. Always runs.
echo "--- AGGREGATES ---"
network_stats=$(curl -sSf -m 10 "$API_BASE/network/stats" 2>/dev/null) || network_stats=""
if [[ -z "$network_stats" ]]; then
  echo "  ERROR  /network/stats fetch failed; skipping aggregate checks"
  ((agg_err++))
else
  api_latest_round=$(echo "$network_stats" | jq -r '.latest_round')
  api_latest_round_block=$(echo "$network_stats" | jq -r '.latest_round_started_block')
  api_total_staked=$(echo "$network_stats" | jq -r '.total_lpt_staked')
  api_active_orchs=$(echo "$network_stats" | jq -r '.active_orchestrators')

  chain_last_init=$(cast_call_with_fallback "$ROUNDS_MANAGER" "lastInitializedRound()(uint256)" 2>/dev/null)
  chain_last_init="${chain_last_init%% *}"
  chain_current_round=$(cast_call_with_fallback "$ROUNDS_MANAGER" "currentRound()(uint256)" 2>/dev/null)
  chain_current_round="${chain_current_round%% *}"
  # Pin getTotalBonded to the round-start block so we compare apples-to-apples
  # against the matview (which is itself frozen at that block). Without this
  # pin, the live chain value diverges by net mid-round Bond/Unbond traffic.
  chain_total_bonded_raw=$(cast_call_with_fallback "$BONDING_MANAGER" "getTotalBonded()(uint256)" --block "$api_latest_round_block" 2>/dev/null)
  chain_total_bonded_raw="${chain_total_bonded_raw%% *}"
  chain_total_bonded=$(wei_to_decimal "$chain_total_bonded_raw")
  chain_pool_size=$(cast_call_with_fallback "$BONDING_MANAGER" "getTranscoderPoolSize()(uint256)" 2>/dev/null)
  chain_pool_size="${chain_pool_size%% *}"

  agg_check() {
    local label="$1" api_val="$2" chain_val="$3" mode="$4" tol="$5"
    local ok=0
    case "$mode" in
      exact) [[ "$api_val" == "$chain_val" ]] && ok=1 ;;
      pct)   within_pct "$api_val" "$chain_val" "$tol" && ok=1 ;;
    esac
    if [[ $ok -eq 1 ]]; then
      printf '  PASS   %-26s api=%s chain=%s\n' "$label" "$api_val" "$chain_val"
      ((agg_pass++))
    else
      printf '  FAIL   %-26s api=%s chain=%s\n' "$label" "$api_val" "$chain_val"
      ((agg_fail++))
    fi
  }

  # latest_round must equal lastInitializedRound exactly. If currentRound is
  # ahead by 1+, someone just hasn't called initializeRound() yet — chain
  # advancement, not an indexer bug. We report both for context.
  agg_check "latest_round (vs lastInit)" "$api_latest_round" "$chain_last_init"   exact ""
  if [[ "$chain_current_round" != "$chain_last_init" ]]; then
    printf '  INFO   currentRound=%s lastInitializedRound=%s (uninitialized round on chain — not an indexer bug)\n' \
      "$chain_current_round" "$chain_last_init"
  fi
  agg_check "active_orchestrators"      "$api_active_orchs"  "$chain_pool_size"    exact ""
  # `total_lpt_staked` is matview-sum of per-orch *latest* snapshots from
  # `orch_stake_by_round`. Inactive orchs aren't re-snapshotted, so the sum
  # accumulates stale stake values across many rounds — it DOES NOT match
  # `getTotalBonded()` even at the same block, by design. Report the
  # divergence for monitoring but don't fail on it.
  drift=$(drift_pct "$api_total_staked" "$chain_total_bonded")
  printf '  INFO   total_lpt_staked         api=%s chain=%s (Δ%s, matview-vs-live, expected)\n' \
    "$api_total_staked" "$chain_total_bonded" "$drift"
fi
hr

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
    if $VALIDATE_TICKETS; then
      validate_gateway_tickets "$addr"
      case $? in
        0) ((tx_pass++)) ;;
        1) ((tx_fail++)) ;;
        2) ((tx_err++)) ;;
      esac
    fi
  done < <(select_gateways "$GATEWAY_SAMPLE" || echo "")
  hr
fi

ended_at=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
echo "=== SUMMARY @ $ended_at ==="
printf '  Aggregates:    %d PASS, %d FAIL, %d ERROR\n' "$agg_pass" "$agg_fail" "$agg_err"
printf '  Orchestrators: %d PASS, %d FAIL, %d ERROR\n' "$orch_pass" "$orch_fail" "$orch_err"
printf '  Gateways:      %d PASS, %d FAIL, %d ERROR\n' "$gw_pass" "$gw_fail" "$gw_err"
if $VALIDATE_TICKETS; then
  printf '  GW Tickets:    %d PASS, %d FAIL, %d ERROR\n' "$tx_pass" "$tx_fail" "$tx_err"
fi
rpc_primary=$(wc -c < "$RPC_PRIMARY_HITS" 2>/dev/null || echo 0)
rpc_fallback=$(wc -c < "$RPC_FALLBACK_HITS" 2>/dev/null || echo 0)
rpc_failed=$(wc -c < "$RPC_FAILED_HITS" 2>/dev/null || echo 0)
printf '  RPC calls:     %d primary_ok, %d fallback_used, %d failed\n' \
  "$rpc_primary" "$rpc_fallback" "$rpc_failed"

# Exit non-zero on any DRIFT or invocation error
if [[ $((agg_fail + agg_err + orch_fail + gw_fail + orch_err + gw_err + tx_fail + tx_err)) -gt 0 ]]; then
  exit 1
fi
exit 0
