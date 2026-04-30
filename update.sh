#!/usr/bin/env bash
set -euo pipefail

git pull --recurse-submodules
git submodule update --init --recursive

cargo build -r

cd web
bun install
bun vite build
node ./scripts/sync-runtime-config.mjs --out dist/runtime-config.js
cd ..

pm2 restart qxp-app

# git submodule sync
# git submodule update --remote web
