# Docker Deployment For lqxp

## What this package includes

- multi-stage Docker build for the Rust server
- Git and Bun in the runtime image for startup-managed web builds
- `docker-compose.yml` for local or single-host deployment
- persistent mounts for `files/config.custom.toml` and `files/qxp.sqlite`

## Prerequisites

Install:

- Docker Engine
- Docker Compose plugin

## 1. Prepare the config

Copy the example config and edit it:

```bash
cp files/config.example.toml files/config.custom.toml
```

Set at least:

- `api.publicDomain`
- `web.repo`, `web.directory`, and either fresh or tag mode
- `rtc.turnUrls`
- `rtc.turnUsername`
- `rtc.turnCredential`
- `security.adminIds`

The container starts with `PRODUCTION=1`, so the server prefers `files/config.custom.toml` when present.

## 2. Build and start

```bash
docker compose up -d --build
```

## 3. Check logs

```bash
docker compose logs -f lqxp
```

## 4. Stop or update

```bash
docker compose down
docker compose up -d --build
```

## Notes

- The app listens on port `4560` in the container.
- The image does not contain a prebuilt web client. Every container startup fetches and rebuilds it before the server listens.
- The container needs outbound access to the configured GitHub repository and the Bun package registry.
- `/app` must be writable so LQXP can replace the disposable checkout configured by `web.directory`.
- `fresh = true` with an empty tag follows `origin/HEAD`; `fresh = false` requires an exact tag.
- Local checkout corruption is recloned once. Network/authentication, permissions/storage, missing Git/Bun, install/build, invalid configuration, or missing output stop the container.
- SQLite and JSON data remain on the host through the mounted files.
- If you also want TURN relay in production, keep using the documented host-level setup in `docs/turn-deployment.md`.

## Installing on a machine

Example on Debian or Ubuntu:

```bash
sudo apt update
sudo apt install -y docker.io docker-compose-plugin
sudo systemctl enable --now docker
```

Clone the repository, then:

```bash
cp files/config.example.toml files/config.custom.toml
nano files/config.custom.toml
docker compose up -d --build
```

## Building an image manually

```bash
docker build -t lqxp-server:latest .
```

Run it without Compose:

```bash
docker run -d \
  --name lqxp \
  -p 4560:4560 \
  -e PRODUCTION=1 \
  -v $(pwd)/files/config.custom.toml:/app/files/config.custom.toml:ro \
  -v $(pwd)/files/qxp.sqlite:/app/files/qxp.sqlite \
  lqxp-server:latest
```
