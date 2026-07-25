#!/usr/bin/env bash
set -euo pipefail

git pull --recurse-submodules
git submodule update --init --recursive

cd web
bun install
bun run build
cd ..

cargo build --release

if pm2 describe qxp-app >/dev/null 2>&1; then
    pm2 restart qxp-app
else
    pm2 start pm2.config.cjs
fi