#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

OUT_DIR="$ROOT/run-logs/full-reload"
mkdir -p "$OUT_DIR"

DB="$(
  source .env >/dev/null 2>&1
  printf '%s' "$DATABASE_URL" | sed 's/@postgres:/@127.0.0.1:/'
)"

ts_local="$(date '+%Y-%m-%d %H:%M:%S %Z (%z)')"
ts_utc="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
valuations="$(psql "$DB" -Atqc "select count(*) from event_valuations;")"
monetary_total="$(psql "$DB" -Atqc "select count(*) from raw_protocol_events where is_canonical = true and finality = 'finalized' and event_name in ('Bond','Unbond','Rebond','WithdrawStake','Reward','EarningsClaimed','WinningTicketRedeemed','WinningTicketTransfer','Transfer','TransferBond','WithdrawFees','DepositFunded','ReserveFunded','ReserveClaimed','Withdrawal');")"

cat > "$OUT_DIR/valuation-benchmark-5am.txt" <<EOF
timestamp_local=$ts_local
timestamp_utc=$ts_utc
event_valuations=$valuations
monetary_total=$monetary_total
remaining=$(( monetary_total - valuations ))
EOF

