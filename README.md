# Claude Usage Widget

A frameless, always-on-top macOS **menu-bar widget** showing live Claude Code
usage — today's messages/sessions/tools, a 14-day activity sparkline, and an
all-time token breakdown by model. Built with Tauri 2 + React + TypeScript.

The Rust backend reads `~/.claude/stats-cache.json` and scans session JSONL
files directly, so the app is fully self-contained — no Node runtime, no shell
scripts, no external data process.

## Behavior

- **Background agent** (`LSUIElement`): no Dock icon, not in Cmd+Tab. Lives in
  the menu bar via a tray icon.
- **Tray menu**: *Show / Hide*, *Start at Login* (synced), *Quit*.
- Closing the window **hides** it to the menu bar instead of quitting.
- **Launch at login** via `SMAppService` (macOS 13+): the app registers itself
  as a login item on first run, so it appears under *System Settings → Login
  Items → Open at Login* with its real icon and a toggle that stays in sync with
  the tray. Auto-registration happens only once (a marker file at
  `~/Library/Application Support/com.chulheong.claudeusage/.login-registered`),
  so disabling it later is respected.
- Window auto-sizes to its content, anchors top-right of the primary display,
  and is draggable anywhere on the card.
- Stats refresh every 60 seconds. Dates are bucketed in UTC to match Claude
  Code's own stats cache.

## Prerequisites

- Node + pnpm
- Rust toolchain:
  ```
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  source "$HOME/.cargo/env"
  ```

## Develop

```
pnpm install
pnpm tauri dev
```

In dev, login-at-launch auto-registration is disabled (it only runs in release
builds), so testing never touches your real Login Items.

## Build & install

```
pnpm tauri build --bundles app

# Install to the per-user Applications folder (preserves the code signature):
ditto "src-tauri/target/release/bundle/macos/Claude Usage.app" \
      ~/Applications/"Claude Usage.app"

# Re-sign ad-hoc so the signature is self-consistent:
codesign --force --deep --sign - ~/Applications/"Claude Usage.app"

open ~/Applications/"Claude Usage.app"
```

The first launch registers the app as a login item automatically.

## Project layout

- `src/` — React + TypeScript frontend (Vite)
  - `App.tsx` — widget UI: stats, sparkline (inline SVG), token bars
  - `types.ts` — `UsageStats` shape mirroring the Rust command output
- `src-tauri/src/lib.rs`
  - `get_usage_stats` — reads the stats cache + session JSONLs, merges, returns stats
  - `login_item` — `SMAppService` registration (login-at-launch)
  - tray icon, window positioning, close-to-hide, agent activation policy
- `src-tauri/Info.plist` — `LSUIElement` (background agent)
- `scripts/make-icon.cjs` — regenerates the source icon (`pnpm tauri icon icon-source.png`)
