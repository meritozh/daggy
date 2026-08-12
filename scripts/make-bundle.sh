#!/usr/bin/env bash
# Assemble a daggy.app bundle from a freshly built release binary.
#
# Usage:
#   cargo build --release -p daggy-app
#   ./scripts/make-bundle.sh
#
# Produces target/release/daggy.app, ad-hoc signed.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET_DIR="$ROOT/target/release"
APP="$TARGET_DIR/daggy.app"
BIN="$TARGET_DIR/daggy"

if [[ ! -x "$BIN" ]]; then
    echo "error: release binary not found at $BIN" >&2
    echo "run: cargo build --release -p daggy-app" >&2
    exit 1
fi

echo "Assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
mkdir -p "$APP/Contents/Resources"

cp "$BIN" "$APP/Contents/MacOS/daggy"
cp "$ROOT/crates/daggy-app/Info.plist" "$APP/Contents/Info.plist"

# Ad-hoc sign so Gatekeeper allows a local launch.
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || \
    echo "warning: codesign failed (continue without signing)"

echo "Done: $APP"
