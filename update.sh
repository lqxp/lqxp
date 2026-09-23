#!/usr/bin/env bash
set -euo pipefail

export PRODUCTION=1
export QXP_ROOT="$PWD"

echo "Racine de déploiement: $QXP_ROOT"
if [[ ! -f files/qxp.sqlite ]]; then
    echo "ATTENTION: files/qxp.sqlite est absent de $QXP_ROOT" >&2
    echo "Le serveur va refuser de démarrer au lieu de créer une base vide." >&2
    echo "Premier déploiement : ajoute createIfMissing = true dans" >&2
    echo "[database] de files/config.custom.toml, puis retire-le." >&2
else
    ls -la files/qxp.sqlite
fi

git pull --recurse-submodules
git submodule update --init --recursive

cd web
bun install
bun run build
cd ..

cargo build --release

if pm2 describe qxp-app >/dev/null 2>&1; then
    pm2 restart qxp-app --update-env
else
    pm2 start pm2.config.cjs
fi
