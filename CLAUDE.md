# CLAUDE.md

This file provides project guidance for AI coding agents working in this
repository.

## What This Is

QuotaGlass is a macOS menu-bar widget (Tauri 2 + React + TypeScript) for live
AI agent usage. It supports Claude Code and Codex through provider-neutral
frontend and Rust command contracts. It runs as an `LSUIElement` background
agent with no Dock or Cmd+Tab presence.

## Commands

```bash
pnpm tauri dev
pnpm build
pnpm ship
pnpm tauri build --bundles app
cargo test --manifest-path src-tauri/Cargo.toml
```

## Version Control

GitButler is not configured for this repository. Use plain git.

## Architecture

Two provider-neutral Tauri commands serve the frontend:

- `get_provider_limits(provider)` returns a dynamic list of quota windows.
- `get_provider_stats(provider)` returns summary metrics, 14-day activity,
  per-model breakdown rows, and footer metadata.

Claude uses its local stats cache, session JSONL files, and the Anthropic OAuth
usage endpoint. Claude Code stores its OAuth blob in either the macOS Keychain
(`Claude Code-credentials`) or `$CLAUDE_CONFIG_DIR/.credentials.json`, depending
on the install — read both and take the token valid longest, or the widget goes
silently stale when the CLI switches storage.

That token lasts eight hours and only a running Claude Code session renews it,
so a gap in usage leaves the widget on cache until the next session. When a
request is rejected and the stored token was already expired, the widget shells
out to `claude doctor`, which renews and stores a new token as a side effect,
then retries once. Never perform the OAuth refresh here: Anthropic rotates
refresh tokens, and a widget writing the rotated blob back races the CLI and can
sign the user out. The delegation is throttled to one attempt per ten minutes
and is verified rather than assumed — the retry only happens if the stored
expiry actually moved forward, so a CLI release that drops this undocumented
side effect degrades to the old cache-fallback behaviour instead of breaking.

A successful usage response is authoritative: a window the API omits has no
usage, so it is dropped rather than backfilled from cache. Only a failed fetch
falls back to cache, and expired cached windows are discarded instead of shown.

Claude's windows come from the response's self-describing `limits` array —
`session`, `weekly_all`, and a `weekly_scoped` entry naming a model through
`scope.model.display_name` (currently Fable). Parse that array, not the legacy
top-level `seven_day_<model>` fields, which are null for every model now; they
remain only as a fallback for older responses. A model scoped or retired
server-side then needs no code change here.

Codex keeps a persistent `codex app-server --stdio` child and calls
`account/rateLimits/read` plus `account/usage/read`. A cached, metadata-only
scan of local Codex session JSONL files supplies session, prompt, tool, and
per-model output-token details. Do not read Codex authentication files.

The React frontend has three persisted views: `superCompact`, `compact`, and
`detailed`. Provider and view cycles are also exposed as global shortcuts:

- Control+Option+P — provider
- Control+Option+V — view
- Command+Shift+U — show/hide

The window remains fixed at 300px wide and content-driven in height. It snaps
to and persists the nearest display corner.

Bundle identifier: `com.chulheong.quotaglass`.

## Adding a Provider

Add a new `ProviderId`, implement its normalized limits/stats adapter under
`src-tauri/src/providers/`, then add its label and icon in `src/App.tsx`.

## Adding a Model Label

Add optional display names and colors to `NAMES` and `COLORS` in `src/App.tsx`.
Unknown model names already have a readable fallback.
