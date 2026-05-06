#!/usr/bin/env bash
set -euo pipefail

git pull --recurse-submodules
git submodule update --init --recursive

cargo build -r

cd web
bun install
bun run build
cd ..

pm2 restart qxp-app

# git submodule sync
# git submodule update --remote web
