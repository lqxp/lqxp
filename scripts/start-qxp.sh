#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_PATH="${QXP_BIN:-$ROOT_DIR/target/release/qxprotocol}"

if [[ ! -x "$BIN_PATH" ]]; then
  echo "qxp binary not found or not executable: $BIN_PATH" >&2
  echo "Build it first with: cargo build --release" >&2
  exit 1
fi

cd "$ROOT_DIR"
exec "$BIN_PATH"
