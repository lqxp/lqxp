# lqxp protocol

TURN deployment packaging is documented in [docs/turn-deployment.md](docs/turn-deployment.md).

## Runtime config builds

The web, desktop, and mobile clients can now be built with runtime values injected at build time through environment variables.

Supported variables:
- `QXP_SERVER_ORIGIN`
- `QXP_API_BASE_URL`
- `QXP_WS_URL`
- `QXP_RELAY_ONLY`
- `QXP_TURN_URLS` comma-separated
- `QXP_TURN_USERNAME`
- `QXP_TURN_CREDENTIAL`
- `QXP_CALLS_ENABLED`
- `QXP_CALLS_UNAVAILABLE_REASON`
- `QXP_RUNTIME_CONFIG_URL` optional source HTML to merge from

Example:

```bash README.md
cd web
QXP_SERVER_ORIGIN=https://chat.example.com \
QXP_TURN_URLS=turn:turn.example.com:3478?transport=udp,turns:turn.example.com:5349?transport=tcp \
QXP_TURN_USERNAME=example \
QXP_TURN_CREDENTIAL=secret \
QXP_CALLS_ENABLED=true \
npm run build
```

On CI/CD, these values can be provided as environment secrets for the web build and the Tauri desktop/mobile build.
