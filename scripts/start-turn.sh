#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TURN_BIN="${TURN_BIN:-$(command -v turnserver || true)}"
TURN_CONF="${TURN_CONF:-$ROOT_DIR/deploy/turn/turnserver.conf}"

if [[ -z "$TURN_BIN" ]]; then
  echo "turnserver binary not found. Install coturn first." >&2
  exit 1
fi

if [[ ! -f "$TURN_CONF" ]]; then
  echo "TURN config not found: $TURN_CONF" >&2
  echo "Generate it first with: ./scripts/bootstrap-turn-prod.sh ..." >&2
  exit 1
fi

cd "$ROOT_DIR"
exec "$TURN_BIN" -c "$TURN_CONF"
