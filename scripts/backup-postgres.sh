#!/usr/bin/env bash
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

if [[ ! -f .env ]]; then
  echo ".env not found in $ROOT" >&2
  exit 1
fi

set -a
source .env
set +a

: "${DATABASE_URL:?DATABASE_URL is required in .env}"

BACKUP_DIR="${1:-$ROOT/backups}"
mkdir -p "$BACKUP_DIR"

ts="$(date -u +%Y%m%dT%H%M%SZ)"
out="$BACKUP_DIR/livepeer_${ts}.dump"

pg_dump "$DATABASE_URL" -Fc -f "$out"
echo "$out"
