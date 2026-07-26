#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TURN_BIN="${TURN_BIN:-$(command -v turn-server || true)}"
TURN_CONF="${TURN_CONF:-$ROOT_DIR/deploy/turn/turn-server.toml}"

if [[ -z "$TURN_BIN" ]]; then
  for candidate in \
    "/home/${USER:-}/.cargo/bin/turn-server" \
    "/usr/local/bin/turn-server" \
    "/usr/bin/turn-server"; do
    if [[ -n "$candidate" && -x "$candidate" ]]; then
      TURN_BIN="$candidate"
      break
    fi
  done
fi

if [[ -z "$TURN_BIN" ]]; then
  echo "turn-server binary not found. Install turn-rs first." >&2
  echo "Example: cargo install turn-server" >&2
  exit 1
fi

if [[ ! -f "$TURN_CONF" ]]; then
  echo "TURN config not found: $TURN_CONF" >&2
  echo "Generate it first with: ./scripts/bootstrap-turn-prod.sh ..." >&2
  exit 1
fi

cd "$ROOT_DIR"
exec "$TURN_BIN" --config="$TURN_CONF"
