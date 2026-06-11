#!/usr/bin/env bash
set -euo pipefail

APP_NAME="Claude Usage"
INSTALL_DIR="$HOME/Applications"
INSTALLED_APP="$INSTALL_DIR/$APP_NAME.app"
BUILD_APP="$(dirname "$0")/../src-tauri/target/release/bundle/macos/$APP_NAME.app"

# ── 1. Kill all running instances ────────────────────────────────────────────
echo "→ Stopping $APP_NAME if running..."
pkill -x "$APP_NAME" 2>/dev/null || true
sleep 1

# Remove stale copy from /Applications if present (prevents two instances)
if [ -d "/Applications/$APP_NAME.app" ]; then
  echo "→ Removing old /Applications/$APP_NAME.app..."
  rm -rf "/Applications/$APP_NAME.app"
fi

# ── 2. Build ──────────────────────────────────────────────────────────────────
echo "→ Building..."
cd "$(dirname "$0")/.."
pnpm tauri build --bundles app 2>&1

# ── 3. Install ────────────────────────────────────────────────────────────────
echo "→ Installing to $INSTALL_DIR..."
rm -rf "$INSTALLED_APP"
cp -R "$BUILD_APP" "$INSTALL_DIR/"

# ── 4. Launch ─────────────────────────────────────────────────────────────────
echo "→ Launching $APP_NAME..."
open "$INSTALLED_APP"

echo "✓ Done"
