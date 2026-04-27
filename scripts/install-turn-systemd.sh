#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SERVICE_NAME="qxp-turn"
SERVICE_PATH="/etc/systemd/system/${SERVICE_NAME}.service"
SERVICE_USER="${SUDO_USER:-${USER}}"
SERVICE_GROUP="$SERVICE_USER"
ENABLE_NOW=false

usage() {
  cat <<'EOF'
Usage:
  sudo ./scripts/install-turn-systemd.sh [options]

Options:
  --user <name>      Unix user that should run coturn
  --group <name>     Unix group that should run coturn
  --enable           Run systemctl enable --now after installing the unit
  --help             Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --user) SERVICE_USER="${2:?}"; shift 2 ;;
    --group) SERVICE_GROUP="${2:?}"; shift 2 ;;
    --enable) ENABLE_NOW=true; shift ;;
    --help|-h) usage; exit 0 ;;
    *)
      echo "Unknown option: $1" >&2
      usage
      exit 1
      ;;
  esac
done

if [[ "${EUID}" -ne 0 ]]; then
  echo "Run this script with sudo/root so it can install the systemd unit." >&2
  exit 1
fi

install -d -m 755 "$(dirname "$SERVICE_PATH")"

cat > "$SERVICE_PATH" <<EOF
[Unit]
Description=QxProtocol TURN relay
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$SERVICE_USER
Group=$SERVICE_GROUP
WorkingDirectory=$ROOT_DIR
ExecStart=$ROOT_DIR/scripts/start-turn.sh
Restart=on-failure
RestartSec=3

NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=$ROOT_DIR/deploy/turn

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload

if [[ "$ENABLE_NOW" == true ]]; then
  systemctl enable --now "$SERVICE_NAME"
else
  systemctl reenable "$SERVICE_NAME"
fi

cat <<EOF
Installed $SERVICE_PATH

Service user:  $SERVICE_USER
Service group: $SERVICE_GROUP
Repo root:     $ROOT_DIR

Useful commands:
  sudo systemctl status $SERVICE_NAME --no-pager
  sudo journalctl -u $SERVICE_NAME -n 100 --no-pager
  sudo ss -ltnup | grep -E ':3478|:5349'
EOF
