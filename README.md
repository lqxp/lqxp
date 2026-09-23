# lqxp protocol

TURN deployment packaging is documented in [docs/turn-deployment.md](docs/turn-deployment.md).

## Web client lifecycle

The Rust server owns the web checkout and rebuilds it on every startup. The checkout directory is disposable: never store manual changes or unrelated files in it. The host must provide `git`, `bun`, a writable `QXP_ROOT`, and outbound access to GitHub and the Bun package registry.

Configure the source in every active TOML file:

```toml
[web]
repo = "https://github.com/lqxp/client.git"
directory = "web"
fresh = true
tag = ""
```

Fresh mode follows the default branch advertised by `origin/HEAD`. To pin a release, use exactly one tag mode:

```toml
[web]
repo = "git@github.com:lqxp/client.git"
directory = "web"
fresh = false
tag = "v1.20.5"
```

At startup LQXP fetches the selected revision, hard-resets and cleans the checkout, runs `bun install` and `bun run build`, then verifies `[network].publicDir/webchatIndex`. Local Git corruption triggers one deletion and clean clone. Configuration, network/authentication, permission/storage, missing-tool, dependency, build, and missing-output errors stop the server. HTTPS credentials are not accepted in `repo`; use the host Git credential configuration or SSH agent.

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
bun run build
```

On CI/CD, these values can be provided as environment secrets for the web build and the Tauri desktop/mobile build.
