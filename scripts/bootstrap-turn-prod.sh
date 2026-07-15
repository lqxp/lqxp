#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG_OUT="$ROOT_DIR/files/config.custom.toml"
TURN_DIR="$ROOT_DIR/deploy/turn"
TURN_CONF_OUT="$TURN_DIR/turn-server.toml"
CREDENTIALS_OUT="$TURN_DIR/credentials.env"
CERTS_DIR="$TURN_DIR/certs"

BIND_HOST="0.0.0.0"
PUBLIC_DOMAIN=""
APP_PORT="4560"
ADMIN_PASSWORD=""
TURN_DOMAIN=""
TURN_USERNAME="qxp-turn"
TURN_CREDENTIAL=""
TURN_PORT="3478"
TURNS_PORT="5349"
EXTERNAL_IP=""
LISTEN_IP="0.0.0.0"
MIN_PORT="49152"
MAX_PORT="65535"

usage() {
  cat <<'EOF'
Usage:
  ./scripts/bootstrap-turn-prod.sh --public-domain qxp.example.com --turn-domain turn.qxp.example.com [options]

Options:
  --bind-host <host>          TCP bind host for qxp (default: 0.0.0.0)
  --public-domain <domain>    Public qxp domain used for runtime metadata
  --app-port <port>           qxp port (default: 4560)
  --admin-password <value>    Optional qxp admin password
  --turn-domain <domain>      TURN realm / DNS name (required)
  --turn-username <value>     TURN long-term auth username (default: qxp-turn)
  --turn-credential <value>   TURN long-term auth password (generated if omitted)
  --turn-port <port>          TURN UDP/TCP port (default: 3478)
  --turns-port <port>         TURN TLS TCP port (default: 5349)
  --external-ip <ip>          Public IP advertised by turn-rs (required if behind NAT)
  --listen-ip <ip>            Local IP bind address for turn-rs (default: 0.0.0.0)
  --min-port <port>           Relay virtual port range start (default: 49152)
  --max-port <port>           Relay virtual port range end (default: 65535)
  --help                      Show this help
EOF
}

random_secret() {
  if command -v openssl >/dev/null 2>&1; then
    openssl rand -hex 24
    return
  fi
  python - <<'PY'
import secrets
print(secrets.token_hex(24))
PY
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --bind-host) BIND_HOST="${2:?}"; shift 2 ;;
    --public-domain) PUBLIC_DOMAIN="${2:?}"; shift 2 ;;
    --app-port) APP_PORT="${2:?}"; shift 2 ;;
    --admin-password) ADMIN_PASSWORD="${2:?}"; shift 2 ;;
    --turn-domain) TURN_DOMAIN="${2:?}"; shift 2 ;;
    --turn-username) TURN_USERNAME="${2:?}"; shift 2 ;;
    --turn-credential) TURN_CREDENTIAL="${2:?}"; shift 2 ;;
    --turn-port) TURN_PORT="${2:?}"; shift 2 ;;
    --turns-port) TURNS_PORT="${2:?}"; shift 2 ;;
    --external-ip) EXTERNAL_IP="${2:?}"; shift 2 ;;
    --listen-ip) LISTEN_IP="${2:?}"; shift 2 ;;
    --relay-ip)
      echo "--relay-ip was a coturn option and is no longer used with turn-rs." >&2
      exit 1
      ;;
    --min-port) MIN_PORT="${2:?}"; shift 2 ;;
    --max-port) MAX_PORT="${2:?}"; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *)
      echo "Unknown option: $1" >&2
      usage
      exit 1
      ;;
  esac
done

if [[ -z "$PUBLIC_DOMAIN" ]]; then
  echo "--public-domain is required" >&2
  exit 1
fi

if [[ -z "$TURN_DOMAIN" ]]; then
  echo "--turn-domain is required" >&2
  exit 1
fi

if [[ -z "$TURN_CREDENTIAL" ]]; then
  TURN_CREDENTIAL="$(random_secret)"
fi

if [[ -z "$EXTERNAL_IP" ]]; then
  EXTERNAL_IP="$TURN_DOMAIN"
fi

mkdir -p "$TURN_DIR" "$CERTS_DIR"

if [[ -f "$CONFIG_OUT" ]]; then
  cp "$CONFIG_OUT" "$CONFIG_OUT.bak.$(date +%Y%m%d%H%M%S)"
fi
if [[ -f "$TURN_CONF_OUT" ]]; then
  cp "$TURN_CONF_OUT" "$TURN_CONF_OUT.bak.$(date +%Y%m%d%H%M%S)"
fi

cat > "$CONFIG_OUT" <<EOF
[api]
domain = "$BIND_HOST"
publicDomain = "$PUBLIC_DOMAIN"
port = $APP_PORT
adminPassword = "$ADMIN_PASSWORD"

[network]
heartbeatInterval = 3000
maxConnectionsPerIp = 3
latestVersion = ""
publicDir = "web/dist"
webchatIndex = "index.html"

[rtc]
relayOnly = true
turnUrls = [
  "turn:$TURN_DOMAIN:$TURN_PORT?transport=udp",
  "turn:$TURN_DOMAIN:$TURN_PORT?transport=tcp",
  "turns:$TURN_DOMAIN:$TURNS_PORT?transport=tcp",
]
turnUsername = "$TURN_USERNAME"
turnCredential = "$TURN_CREDENTIAL"
EOF

cat > "$TURN_CONF_OUT" <<EOF
[server]
realm = "$TURN_DOMAIN"
port-range = "$MIN_PORT..$MAX_PORT"

[[server.interfaces]]
transport = "udp"
listen = "$LISTEN_IP:$TURN_PORT"
external = "$EXTERNAL_IP:$TURN_PORT"

[[server.interfaces]]
transport = "tcp"
listen = "$LISTEN_IP:$TURN_PORT"
external = "$EXTERNAL_IP:$TURN_PORT"

[[server.interfaces]]
transport = "tcp"
listen = "$LISTEN_IP:$TURNS_PORT"
external = "$EXTERNAL_IP:$TURNS_PORT"

[server.interfaces.ssl]
private-key = "$CERTS_DIR/privkey.pem"
certificate-chain = "$CERTS_DIR/fullchain.pem"

[auth.static-credentials]
"$TURN_USERNAME" = "$TURN_CREDENTIAL"

[log]
level = "info"
EOF

cat > "$CREDENTIALS_OUT" <<EOF
QXP_PUBLIC_DOMAIN=$PUBLIC_DOMAIN
QXP_TURN_DOMAIN=$TURN_DOMAIN
QXP_TURN_USERNAME=$TURN_USERNAME
QXP_TURN_CREDENTIAL=$TURN_CREDENTIAL
QXP_TURN_PORT=$TURN_PORT
QXP_TURNS_PORT=$TURNS_PORT
QXP_TURN_LISTEN_IP=$LISTEN_IP
QXP_TURN_EXTERNAL_IP=$EXTERNAL_IP
QXP_TURN_CONFIG=$TURN_CONF_OUT
EOF

chmod 600 "$CONFIG_OUT" "$TURN_CONF_OUT" "$CREDENTIALS_OUT"

cat <<EOF
TURN bootstrap complete for turn-rs.

Generated:
  - $CONFIG_OUT
  - $TURN_CONF_OUT
  - $CREDENTIALS_OUT

Next steps:
  1. Install turn-rs / turn-server on the VPS.
  2. Obtain certificates for $TURN_DOMAIN with certbot.
  3. Copy certificates into $CERTS_DIR using:
     ./scripts/certbot-turn-deploy-hook.sh --turn-domain $TURN_DOMAIN
  4. Build qxp:
     cargo build --release
  5. Install TURN as a systemd service:
     sudo ./scripts/install-turn-systemd.sh --enable
  6. Start the qxp app with your preferred supervisor.
EOF
