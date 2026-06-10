#!/usr/bin/env bash
set -euo pipefail

APP_NAME="Claude Usage"
INSTALL_DIR="$HOME/Applications"
INSTALLED_APP="$INSTALL_DIR/$APP_NAME.app"
BUILD_APP="$(dirname "$0")/../src-tauri/target/release/bundle/macos/$APP_NAME.app"

# ── 1. Kill running instance ──────────────────────────────────────────────────
echo "→ Stopping $APP_NAME if running..."
if pgrep -x "Claude Usage" > /dev/null 2>&1; then
  pkill -x "Claude Usage"
  sleep 1
fi

# ── 2. Build ──────────────────────────────────────────────────────────────────
echo "→ Building..."
cd "$(dirname "$0")/.."
pnpm tauri build 2>&1

# ── 3. Install ────────────────────────────────────────────────────────────────
echo "→ Installing to $INSTALL_DIR..."
rm -rf "$INSTALLED_APP"
cp -R "$BUILD_APP" "$INSTALL_DIR/"

# ── 4. Launch ─────────────────────────────────────────────────────────────────
echo "→ Launching $APP_NAME..."
open "$INSTALLED_APP"

echo "✓ Done"
