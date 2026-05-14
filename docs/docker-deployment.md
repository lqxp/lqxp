# Docker Deployment For lqxp

## What this package includes

- multi-stage Docker build for the Rust server and web client
- `docker-compose.yml` for local or single-host deployment
- persistent mounts for `files/config.custom.toml`, `files/qxp.sqlite`, and `files/database.json`

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
- The web client static files are built into the image and served by the Rust server.
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
  -v $(pwd)/files/database.json:/app/files/database.json \
  lqxp-server:latest
```
