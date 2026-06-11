# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What This Is

A macOS menu-bar widget (Tauri 2 + React + TypeScript) that displays live Claude Code usage stats. It runs as an `LSUIElement` background agent — no Dock icon, not in Cmd+Tab. The Rust backend reads `~/.claude/stats-cache.json` and scans session JSONL files directly; no external processes or Node runtime in production.

## Commands

```bash
# Development (hot-reload frontend + Rust backend)
pnpm tauri dev

# Type-check only
pnpm build          # tsc + vite build (no Tauri)

# Build, install to ~/Applications, and relaunch (one-step ship)
pnpm ship           # runs scripts/deploy.sh

# Build release bundle manually
pnpm tauri build --bundles app
```

There are no tests in this project.

## Version Control

GitButler is **not configured** for this repo. Use plain `git` for all commits, pushes, and branches — do not invoke the `/gitbutler` skill or `but` CLI here.

## Architecture

### Data flow

Two Tauri commands provide all data to the React frontend:

- **`get_usage_stats`** (`lib.rs:172`) — pure local reads. Merges `~/.claude/stats-cache.json` (pre-computed totals) with a delta scan of `~/.claude/projects/**/*.jsonl` for dates newer than `lastComputedDate`. Returns today's stats, 14-day sparkline, all-time totals, and output tokens by model.

- **`get_rate_limits`** (`lib.rs:337`) — network call. Reads the OAuth token from the macOS Keychain (`security find-generic-password -s "Claude Code-credentials"`), then `curl`s `https://api.anthropic.com/api/oauth/usage` to get `fiveHour`, `sevenDay`, and `sevenDaySonnet` utilization windows.

### Frontend (`src/`)

- `App.tsx` — single component. Three display modes:
  - **Ultra-compact** (`ultra` state, persisted in `localStorage`): three unlabeled utilization bars
  - **Normal compact** (default): labeled rate-limit bars with reset time
  - **Expanded** (`expanded` state): adds today's message/session/tool counts, 14-day inline SVG sparkline, and per-model token bars
- `types.ts` — TypeScript interfaces mirroring the Rust `#[derive(Serialize)]` structs

### Rust backend (`src-tauri/src/lib.rs`)

- Window is fixed at 300px wide; height is content-driven. The frontend measures `cardRef` height and calls `getCurrentWindow().setSize()` + `invoke("reanchor")` on every data change, and via `requestAnimationFrame` during CSS transitions (420ms window).
- **Corner-snap**: after the user stops dragging for 350ms, the window snaps to the nearest display corner and persists the preference to `~/Library/Application Support/com.chulheong.claudeusage/.corner`.
- **Login item**: `SMAppService.mainAppService()` (macOS 13+). Auto-registers on first release-build launch (guarded by a marker file). The tray "Start at Login" toggle stays in sync with System Settings.
- Date math uses Howard Hinnant's civil-from-days algorithm (no `chrono` dependency) — UTC bucketing to match Claude Code's own stats cache.

### Key Tauri config (`src-tauri/tauri.conf.json`)

- `decorations: false`, `transparent: true`, `alwaysOnTop: true`
- `macOSPrivateApi: true` — required for the accessory activation policy
- Bundle identifier: `com.chulheong.claudeusage`

## Adding a New Model

Add an entry to `NAMES` and `COLORS` in `src/App.tsx:9-23`. The `modelLabel()` fallback will handle unknown models gracefully.

## Refreshing OAuth Token Logic

`read_oauth_token()` (`lib.rs:313`) runs `security find-generic-password` synchronously inside the Tauri command. If the keychain item name ever changes in Claude Code, update the `-s` argument there.
