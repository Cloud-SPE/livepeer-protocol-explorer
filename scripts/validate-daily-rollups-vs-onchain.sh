#!/usr/bin/env bash
# scripts/validate-daily-rollups-vs-onchain.sh
#
# Daily-window audit: for a finalized UTC day, compare API rollup totals
# against on-chain `eth_getLogs` aggregates. Catches indexer-missed events
# AND rollup mis-computations in one sweep.
#
# Coverage (single `cast logs` call per event type, one UTC day fits under
# Chainstack's 500k block range limit on Arbitrum):
#
#   WinningTicketRedeemed  →  count + sum(faceValue)
#                            compared to /tickets/timeseries/daily (count)
#                                     and /payouts/summary/daily/<date> (sum)
#   Reward                 →  count + sum(LPT minted)
#                            compared to /rewards/summary/daily/<date>
#                                                            (reward_event_count, sum_total_tokens)
#
# Requires: bash, curl, jq, python3, foundry's `cast`
#
# Block range discovery for the UTC day:
#   --from-block N --to-block N    explicit override (preferred when no DB access)
#   $POSTGRES_CONTAINER set        auto-discover via `docker exec ... psql`
#                                  on `raw_protocol_events.block_timestamp`
#
# Usage:
#   bash scripts/validate-daily-rollups-vs-onchain.sh                              # yesterday
#   bash scripts/validate-daily-rollups-vs-onchain.sh --date 2026-05-10
#   bash scripts/validate-daily-rollups-vs-onchain.sh --date 2026-05-10 --from-block 461317276 --to-block 461623949
#   bash scripts/validate-daily-rollups-vs-onchain.sh --api-url https://livepeer-network-api.cloudspe.com
#
# Env:
#   INFURA_RPC_URL / ARCHIVE_RPC_URL / CHAINSTACK_RPC_URL  same precedence as validate-vs-onchain.sh
#   POSTGRES_CONTAINER     name of running postgres container for auto block-range (defaults to
#                          livepeer-valuation-postgres). Pair with POSTGRES_USER/PASSWORD/DB.
#
# Exit codes:
#   0  all comparisons within tolerance
#   1  any mismatch
#   2  invocation error (missing dependency, no block range, API unreachable)

set -uo pipefail

# ── Defaults ─────────────────────────────────────────────────────────
API_URL="https://livepeer-network-api.cloudspe.com"
DATE=""
FROM_BLOCK=""
TO_BLOCK=""
POSTGRES_CONTAINER="${POSTGRES_CONTAINER:-livepeer-valuation-postgres}"

TICKET_BROKER="0xa8bB618B1520E284046F3dFc448851A1Ff26e41B"
BONDING_MANAGER="0x35Bcf3c30594191d53231E4FF333E8A770453e40"
# Event topic0s (precomputed via cast keccak)
TOPIC_WINNING_TICKET="0xc389eb51ed006dbf2528507f010efdf5225ea596e1e1741d74f550dab1925ee7"
TOPIC_REWARD="0x619caafabdd75649b302ba8419e48cccf64f37f1983ac4727cfb38b57703ffc9"

# Tolerances. Counts are deterministic — no tolerance. Face value sums can
# rounding-drift in the last wei, allow 0.0001 ETH / 0.01 LPT.
TOL_ETH="0.0001"
TOL_LPT="0.01"

# ── Arg parsing ─────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --date)           DATE="$2"; shift 2 ;;
    --from-block)     FROM_BLOCK="$2"; shift 2 ;;
    --to-block)       TO_BLOCK="$2"; shift 2 ;;
    --api-url)        API_URL="$2"; shift 2 ;;
    --primary-rpc)    PRIMARY_RPC_OVERRIDE="$2"; shift 2 ;;
    --fallback-rpc)   FALLBACK_RPC_OVERRIDE="$2"; shift 2 ;;
    --postgres-container) POSTGRES_CONTAINER="$2"; shift 2 ;;
    -h|--help)
      sed -n '2,/^$/p' "$0" | sed 's/^# //; s/^#$//'
      exit 0
      ;;
    *) echo "unknown flag: $1" >&2; exit 2 ;;
  esac
done

# Default DATE = yesterday UTC.
if [[ -z "$DATE" ]]; then
  DATE=$(date -u -d 'yesterday' +%Y-%m-%d)
fi

# RPC resolution — identical priority to validate-vs-onchain.sh.
PRIMARY_RPC_URL="${PRIMARY_RPC_OVERRIDE:-${INFURA_RPC_URL:-${ARCHIVE_RPC_URL:-${CHAINSTACK_RPC_URL:-}}}}"
FALLBACK_RPC_URL="${FALLBACK_RPC_OVERRIDE:-${CHAINSTACK_RPC_URL:-${ARCHIVE_RPC_URL:-${INFURA_RPC_URL:-}}}}"
if [[ -z "$PRIMARY_RPC_URL" ]]; then
  echo "ERROR: set INFURA_RPC_URL, ARCHIVE_RPC_URL, or CHAINSTACK_RPC_URL." >&2
  exit 2
fi
[[ -z "$FALLBACK_RPC_URL" ]] && FALLBACK_RPC_URL="$PRIMARY_RPC_URL"

# Dep checks.
for tool in curl jq python3 cast; do
  command -v "$tool" >/dev/null 2>&1 || { echo "ERROR: '$tool' not found." >&2; exit 2; }
done

API_BASE="${API_URL%/}/api/v1"

# ── Block range discovery ────────────────────────────────────────────
if [[ -z "$FROM_BLOCK" || -z "$TO_BLOCK" ]]; then
  if [[ -z "${POSTGRES_USER:-}" || -z "${POSTGRES_DB:-}" ]]; then
    echo "ERROR: --from-block/--to-block not provided AND POSTGRES_USER/POSTGRES_DB not set." >&2
    echo "       Either provide explicit block range or source .env so we can auto-discover via psql." >&2
    exit 2
  fi
  start_ts="${DATE}T00:00:00Z"
  end_ts="${DATE}T23:59:59Z"
  echo "Discovering block range for $DATE via psql on $POSTGRES_CONTAINER..."
  range=$(docker exec -e PGPASSWORD="${POSTGRES_PASSWORD:-}" "$POSTGRES_CONTAINER" \
    psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -At -F'|' -c "
      SELECT MIN(block_number), MAX(block_number)
      FROM raw_protocol_events
      WHERE block_timestamp >= '$start_ts'::timestamptz
        AND block_timestamp <  ('$DATE'::date + interval '1 day')::timestamptz" 2>/dev/null) || {
    echo "ERROR: psql block-range query failed. Provide --from-block/--to-block explicitly." >&2
    exit 2
  }
  FROM_BLOCK="${range%|*}"
  TO_BLOCK="${range#*|}"
  if [[ -z "$FROM_BLOCK" || "$FROM_BLOCK" == "|" ]]; then
    echo "ERROR: no events found in raw_protocol_events for $DATE. Either the date is wrong or the indexer hasn't ingested it." >&2
    exit 2
  fi
fi

# ── Helpers ─────────────────────────────────────────────────────────
hr() { printf '%.0s─' {1..72}; echo; }

within_tol() {
  python3 -c "import sys; a=float('$1' or '0'); b=float('$2' or '0'); t=float('$3'); sys.exit(0 if abs(a-b)<=t else 1)"
}

# Counts. Cheap to track.
RPC_STATS_DIR=$(mktemp -d -t validate-daily.XXXXXX)
trap 'rm -rf "$RPC_STATS_DIR"' EXIT

is_fallback_worthy_error() {
  grep -qiE 'too many requests|rate limit|429|-32005|-32016|project id|daily request count|timeout|timed out|connection refused|temporarily unavailable|service unavailable|^.*5[0-9][0-9].*$|-32603|exhaust' <<<"$1"
}

cast_logs_with_fallback() {
  local tmp_out tmp_err err rc
  tmp_out=$(mktemp); tmp_err=$(mktemp)
  cast logs "$@" --rpc-url "$PRIMARY_RPC_URL" >"$tmp_out" 2>"$tmp_err"
  rc=$?
  if [[ $rc -eq 0 ]]; then
    cat "$tmp_out"; rm -f "$tmp_out" "$tmp_err"; return 0
  fi
  err=$(cat "$tmp_err")
  if [[ "$PRIMARY_RPC_URL" != "$FALLBACK_RPC_URL" ]] && is_fallback_worthy_error "$err"; then
    cast logs "$@" --rpc-url "$FALLBACK_RPC_URL" >"$tmp_out" 2>"$tmp_err"
    rc=$?
    if [[ $rc -eq 0 ]]; then
      cat "$tmp_out"; rm -f "$tmp_out" "$tmp_err"; return 0
    fi
  fi
  rm -f "$tmp_out" "$tmp_err"; return 1
}

# Sum the first 32-byte chunk of each event's `data` field as an unsigned int,
# divide by 10^18 to convert wei → ETH or wei → LPT.
sum_first_data_word_to_decimal() {
  python3 -c "
import sys
total = 0
for line in sys.stdin:
    line = line.strip()
    if not line: continue
    h = line[2:]  # strip 0x
    total += int(h[:64], 16)
print(f'{total / 10**18:.18f}'.rstrip('0').rstrip('.') or '0')
"
}

# ── Main ────────────────────────────────────────────────────────────
started_at=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
echo "=== validate-daily-rollups-vs-onchain @ $started_at ==="
echo "Date:        $DATE"
echo "Block range: $FROM_BLOCK → $TO_BLOCK"
echo "API:         $API_URL"
echo "RPC primary: ${PRIMARY_RPC_URL%/*}/<key-redacted>"
echo "RPC fallback: ${FALLBACK_RPC_URL%/*}/<key-redacted>"
echo

pass=0; fail=0; err=0

# ─── WinningTicketRedeemed ────────────────────────────────────────────
echo "--- WinningTicketRedeemed (TicketBroker) ---"
ticket_logs=$(cast_logs_with_fallback --from-block "$FROM_BLOCK" --to-block "$TO_BLOCK" --address "$TICKET_BROKER" "$TOPIC_WINNING_TICKET") || {
  echo "  ERROR  cast logs WinningTicketRedeemed failed"
  ((err++))
}
if [[ -n "$ticket_logs" ]]; then
  chain_ticket_count=$(grep -c '^- address' <<<"$ticket_logs")
  chain_face_value=$(grep -oP 'data: 0x[0-9a-f]+' <<<"$ticket_logs" | awk '{print $2}' | sum_first_data_word_to_decimal)
else
  chain_ticket_count=0
  chain_face_value=0
fi

api_tickets_json=$(curl -sSf -m 10 "$API_BASE/tickets/timeseries/daily?start=$DATE&end=$DATE") || api_tickets_json=""
api_ticket_count=$(jq -r '([.ai[]?.count|tonumber] + [.transcoding[]?.count|tonumber]) | add // 0' <<<"$api_tickets_json")

api_payouts_json=$(curl -sSf -m 10 "$API_BASE/payouts/summary/daily/$DATE") || api_payouts_json=""
api_face_value=$(jq -r '.sum_face_value_native // "0"' <<<"$api_payouts_json")

# count compare (exact)
if [[ "$chain_ticket_count" == "$api_ticket_count" ]]; then
  printf '  PASS   count          chain=%d api=%d\n' "$chain_ticket_count" "$api_ticket_count"
  ((pass++))
else
  printf '  FAIL   count          chain=%d api=%d (Δ=%d)\n' "$chain_ticket_count" "$api_ticket_count" $((chain_ticket_count - api_ticket_count))
  ((fail++))
fi
# face-value compare (small tolerance)
if within_tol "$chain_face_value" "$api_face_value" "$TOL_ETH"; then
  printf '  PASS   faceValue ETH  chain=%s api=%s\n' "$chain_face_value" "$api_face_value"
  ((pass++))
else
  printf '  FAIL   faceValue ETH  chain=%s api=%s\n' "$chain_face_value" "$api_face_value"
  ((fail++))
fi

# ─── Reward ───────────────────────────────────────────────────────────
echo "--- Reward (BondingManager) ---"
reward_logs=$(cast_logs_with_fallback --from-block "$FROM_BLOCK" --to-block "$TO_BLOCK" --address "$BONDING_MANAGER" "$TOPIC_REWARD") || {
  echo "  ERROR  cast logs Reward failed"
  ((err++))
}
if [[ -n "$reward_logs" ]]; then
  chain_reward_count=$(grep -c '^- address' <<<"$reward_logs")
  chain_reward_total=$(grep -oP 'data: 0x[0-9a-f]+' <<<"$reward_logs" | awk '{print $2}' | sum_first_data_word_to_decimal)
else
  chain_reward_count=0
  chain_reward_total=0
fi

api_rewards_json=$(curl -sSf -m 10 "$API_BASE/rewards/summary/daily/$DATE") || api_rewards_json=""
api_reward_count=$(jq -r '.reward_event_count // "0"' <<<"$api_rewards_json")
api_reward_total=$(jq -r '.sum_total_tokens // "0"' <<<"$api_rewards_json")

if [[ "$chain_reward_count" == "$api_reward_count" ]]; then
  printf '  PASS   count          chain=%d api=%d\n' "$chain_reward_count" "$api_reward_count"
  ((pass++))
else
  printf '  FAIL   count          chain=%d api=%d (Δ=%d)\n' "$chain_reward_count" "$api_reward_count" $((chain_reward_count - api_reward_count))
  ((fail++))
fi
if within_tol "$chain_reward_total" "$api_reward_total" "$TOL_LPT"; then
  printf '  PASS   total LPT      chain=%s api=%s\n' "$chain_reward_total" "$api_reward_total"
  ((pass++))
else
  printf '  FAIL   total LPT      chain=%s api=%s\n' "$chain_reward_total" "$api_reward_total"
  ((fail++))
fi
hr

ended_at=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
echo "=== SUMMARY @ $ended_at ==="
printf '  Date:  %s (blocks %d → %d)\n' "$DATE" "$FROM_BLOCK" "$TO_BLOCK"
printf '  Result: %d PASS, %d FAIL, %d ERROR\n' "$pass" "$fail" "$err"
if [[ $((fail + err)) -gt 0 ]]; then exit 1; fi
exit 0
