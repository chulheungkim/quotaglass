#!/usr/bin/env bash
set -euo pipefail

APP_NAME="QuotaGlass"
LEGACY_APP_NAME="Claude Usage"
INSTALL_DIR="$HOME/Applications"
INSTALLED_APP="$INSTALL_DIR/$APP_NAME.app"
LEGACY_INSTALLED_APP="$INSTALL_DIR/$LEGACY_APP_NAME.app"
SYSTEM_LEGACY_APP="/Applications/$LEGACY_APP_NAME.app"
BUILD_APP="$(dirname "$0")/../src-tauri/target/release/bundle/macos/$APP_NAME.app"

# ── 1. Build before touching the installed app ───────────────────────────────
echo "→ Building..."
cd "$(dirname "$0")/.."
pnpm tauri build --bundles app 2>&1

# ── 2. Stop old and new product names ────────────────────────────────────────
echo "→ Stopping running widgets..."
pkill -x "$APP_NAME" 2>/dev/null || true
pkill -x "$LEGACY_APP_NAME" 2>/dev/null || true
sleep 1

# ── 3. Replace the user-local installation ───────────────────────────────────
echo "→ Installing to $INSTALL_DIR..."
mkdir -p "$INSTALL_DIR"
rm -rf "$INSTALLED_APP"
rm -rf "$LEGACY_INSTALLED_APP"
rm -rf "$SYSTEM_LEGACY_APP"
ditto "$BUILD_APP" "$INSTALLED_APP"
codesign --force --deep --sign - "$INSTALLED_APP"

# ── 4. Launch ─────────────────────────────────────────────────────────────────
echo "→ Launching $APP_NAME..."
open "$INSTALLED_APP"

echo "✓ Done"
