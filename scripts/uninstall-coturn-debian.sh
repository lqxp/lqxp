#!/usr/bin/env bash
set -euo pipefail

PURGE_DATA=false

usage() {
  cat <<'EOF'
Usage:
  sudo ./scripts/uninstall-coturn-debian.sh [options]

Options:
  --purge-data   Remove common coturn config/data/log directories too
  --help         Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --purge-data) PURGE_DATA=true; shift ;;
    --help|-h) usage; exit 0 ;;
    *)
      echo "Unknown option: $1" >&2
      usage
      exit 1
      ;;
  esac
done

if [[ "${EUID}" -ne 0 ]]; then
  echo "Run this script with sudo/root." >&2
  exit 1
fi

if command -v systemctl >/dev/null 2>&1; then
  for service in coturn turnserver; do
    systemctl stop "$service" 2>/dev/null || true
    systemctl disable "$service" 2>/dev/null || true
    systemctl reset-failed "$service" 2>/dev/null || true
  done
fi

if command -v apt-get >/dev/null 2>&1; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get purge -y coturn || true
  apt-get autoremove -y --purge || true
else
  echo "apt-get not found; remove the coturn package manually." >&2
fi

rm -f /etc/systemd/system/coturn.service /etc/systemd/system/turnserver.service
rm -f /etc/default/coturn /etc/logrotate.d/coturn

if [[ "$PURGE_DATA" == true ]]; then
  rm -rf /etc/turnserver.conf /etc/coturn /var/lib/coturn /var/log/coturn /run/coturn /var/run/coturn
fi

if command -v systemctl >/dev/null 2>&1; then
  systemctl daemon-reload
fi

if command -v deluser >/dev/null 2>&1; then
  deluser --remove-home turnserver 2>/dev/null || true
  deluser --remove-home coturn 2>/dev/null || true
fi

if command -v delgroup >/dev/null 2>&1; then
  delgroup turnserver 2>/dev/null || true
  delgroup coturn 2>/dev/null || true
fi

cat <<EOF
coturn uninstall complete.

Removed:
  - coturn package via apt purge, if present
  - coturn/turnserver systemd units, if present
  - common coturn defaults/logrotate files

Data purge enabled: $PURGE_DATA
EOF
