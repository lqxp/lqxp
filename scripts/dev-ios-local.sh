#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WEB_DIR="$ROOT_DIR/web"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: iOS development requires macOS with the full Xcode app installed" >&2
  exit 1
fi

command -v npm >/dev/null 2>&1 || {
  echo "error: npm is required" >&2
  exit 1
}

rustup target add aarch64-apple-ios x86_64-apple-ios aarch64-apple-ios-sim

if [[ -f "$WEB_DIR/package-lock.json" ]]; then
  npm ci --prefix "$WEB_DIR"
else
  npm install --prefix "$WEB_DIR"
fi

if [[ ! -d "$WEB_DIR/src-tauri/gen/apple" || "${QXP_FORCE_IOS_INIT:-}" == "1" ]]; then
  (cd "$WEB_DIR" && npm run ios:init)
fi

(cd "$WEB_DIR" && npm run ios:dev -- "$@")
