#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WEB_DIR="$ROOT_DIR/web"
IOS_TARGETS=(
  aarch64-apple-ios
  x86_64-apple-ios
  aarch64-apple-ios-sim
)

die() {
  echo "error: $*" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || die "$1 is required"
}

if [[ "$(uname -s)" != "Darwin" ]]; then
  die "iOS builds require macOS with the full Xcode app installed"
fi

need node
need npm
need rustup
need cargo
need xcodebuild

xcodebuild -version >/dev/null

if ! command -v pod >/dev/null 2>&1; then
  if command -v brew >/dev/null 2>&1; then
    echo "CocoaPods is missing; installing it with Homebrew..."
    brew install cocoapods
  else
    die "CocoaPods is required. Install Homebrew, then run: brew install cocoapods"
  fi
fi

for target in "${IOS_TARGETS[@]}"; do
  rustup target add "$target"
done

if [[ -f "$WEB_DIR/package-lock.json" ]]; then
  npm ci --prefix "$WEB_DIR"
else
  npm install --prefix "$WEB_DIR"
fi

if [[ ! -d "$WEB_DIR/src-tauri/gen/apple" || "${QXP_FORCE_IOS_INIT:-}" == "1" ]]; then
  (cd "$WEB_DIR" && npm run ios:init)
fi

(cd "$WEB_DIR" && npm run ios:build -- "$@")
