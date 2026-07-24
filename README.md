# QuotaGlass

QuotaGlass is a frameless, always-on-top macOS menu-bar widget for monitoring
AI agent usage. It currently supports Claude Code and Codex while keeping the
provider boundary open for future agents.

**AI agent usage at a glance.**

The widget shows live quota windows in compact views and adds daily activity,
local sessions, tool calls, and per-model output tokens in its detailed view.
It is built with Tauri 2, React, and TypeScript.

## Shortcuts

The shortcuts are macOS-wide while QuotaGlass is running and are unregistered
when it quits:

- `Control+Option+P` — switch provider
- `Control+Option+V` — cycle super compact, compact, and detailed views
- `Command+Shift+U` — show or hide QuotaGlass

The provider name in the header is also clickable.

## Data sources

### Claude Code

- Reads the local Claude stats cache and session JSONL files.
- Fetches quota windows from Anthropic's OAuth usage endpoint using the Claude
  Code credential stored in macOS Keychain.

### Codex

- Keeps one local `codex app-server` process and uses the stable
  `account/rateLimits/read` and `account/usage/read` methods.
- Reads local Codex session JSONL files for metadata-only details: prompt,
  session, and tool counts plus output tokens by model.
- Never reads or stores prompt text, response text, or Codex authentication
  files.

Both providers retain last-known-good quota data so transient network or
account errors do not immediately blank the widget.

## Behavior

- Runs as an `LSUIElement` background agent: no Dock icon and no Cmd+Tab entry.
- Provides Show / Hide, Start at Login, and Quit in the menu-bar tray.
- Closing the window hides it rather than quitting.
- Auto-sizes, snaps to the nearest display corner, and remembers that corner.
- Registers with `SMAppService` on the first installed release launch.
- Migrates state from the previous Claude Usage bundle identifier on first run.

## Prerequisites

- macOS 13 or newer
- Node.js and pnpm
- Rust toolchain
- Claude Code and/or Codex authenticated locally for the corresponding provider

## Develop

```bash
pnpm install
pnpm tauri dev
```

## Build and install

```bash
pnpm ship
```

This builds the release app, installs it at
`~/Applications/QuotaGlass.app`, removes the previous user-local
`Claude Usage.app`, and launches QuotaGlass.

To build only:

```bash
pnpm tauri build --bundles app
```

## Project layout

- `src/` — provider-neutral React UI, persistent provider/view state, assets
- `src-tauri/src/providers/` — provider integrations and shared response types
- `src-tauri/src/codex_rpc.rs` — persistent Codex app-server JSON-RPC client
- `src-tauri/src/lib.rs` — Claude adapter, Tauri commands, shortcuts, tray,
  window placement, session watchers, and login item
- `scripts/deploy.sh` — one-step build, install, and relaunch
