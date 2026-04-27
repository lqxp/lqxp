#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TURN_DOMAIN=""
LIVE_DIR=""

usage() {
  cat <<'EOF'
Usage:
  ./scripts/certbot-turn-deploy-hook.sh --turn-domain turn.qxp.example.com [--live-dir /etc/letsencrypt/live/turn.qxp.example.com]
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --turn-domain) TURN_DOMAIN="${2:?}"; shift 2 ;;
    --live-dir) LIVE_DIR="${2:?}"; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *)
      echo "Unknown option: $1" >&2
      usage
      exit 1
      ;;
  esac
done

if [[ -z "$TURN_DOMAIN" ]]; then
  echo "--turn-domain is required" >&2
  exit 1
fi

if [[ -z "$LIVE_DIR" ]]; then
  LIVE_DIR="/etc/letsencrypt/live/$TURN_DOMAIN"
fi

DEST_DIR="$ROOT_DIR/deploy/turn/certs"
mkdir -p "$DEST_DIR"

install -m 600 "$LIVE_DIR/fullchain.pem" "$DEST_DIR/fullchain.pem"
install -m 600 "$LIVE_DIR/privkey.pem" "$DEST_DIR/privkey.pem"

echo "Copied TURN certificates to $DEST_DIR"
