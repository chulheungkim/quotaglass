# QuotaGlass Provider Integration Design

## Summary

Rename the Claude Usage widget to QuotaGlass and evolve it into a
provider-neutral macOS usage monitor. The first additional provider is OpenAI
Codex. The existing visual design remains intact; this change adds provider
switching, Codex usage data, and global view switching without a broader UI
redesign.

QuotaGlass uses the tagline: **AI agent usage at a glance.**

## Goals

- Support Claude Code and OpenAI Codex through one normalized frontend model.
- Add a global shortcut and an in-widget interaction for switching providers.
- Add a global shortcut for cycling all three existing view modes.
- Use Codex's stable app-server interface for live account data.
- Preserve useful local activity when live provider data is unavailable.
- Rename the app, package, repository, bundle, support directory, and local
  project directory to QuotaGlass.
- Preserve existing user preferences and cached data where they remain valid.

## Non-Goals

- Redesigning the widget's layout, colors, typography, or component styling.
- Adding a settings window or configurable shortcut editor.
- Supporting providers other than Claude and Codex in this iteration.
- Reading, copying, or persisting Codex OAuth credentials.
- Estimating monetary cost from public model pricing.

## Product Identity

| Surface                       | Value                       |
| ----------------------------- | --------------------------- |
| App name                      | QuotaGlass                  |
| Project/package name          | `quotaglass`                |
| Repository name               | `quotaglass`                |
| Bundle identifier             | `com.chulheong.quotaglass`  |
| Application support directory | `com.chulheong.quotaglass`  |
| Tagline                       | AI agent usage at a glance. |

The implementation updates the visible product name and internal identifier in
the same release. On first launch, QuotaGlass copies compatible preferences and
last-known usage data from the legacy `com.chulheong.claudeusage` support
directory. The installed legacy application is replaced by the deployment
script after a successful QuotaGlass build.

## Provider-Neutral Data Model

The frontend must not know provider-specific rate-limit field names. Rust
adapters normalize provider data into two response families.

### Limits

Each limit window contains:

- Stable identifier
- Display label
- Utilization percentage
- Reset timestamp
- Optional window duration
- Optional stale/cached state

The response also carries optional provider metadata such as plan type, credit
balance, unlimited-credit state, or reached-limit classification. The current
UI only renders metadata that fits its existing layout; unavailable fields are
omitted.

Claude produces its current three windows. Codex produces the live primary and
secondary windows returned by its account interface, normally the five-hour and
weekly windows. Codex window labels derive from `windowDurationMins`, rather
than assuming that a primary window is session-based. The UI renders the
collection dynamically and never inserts a placeholder third Codex bar.

### Statistics

Statistics contain provider-selected collections instead of fixed
Claude-specific fields:

- Three summary metrics with labels and formatted numeric values
- A dated activity series with its metric label
- Model/token breakdown rows
- Footer totals
- A short data-scope label when account and local data differ
- Last update timestamp

Claude maps its existing messages, sessions, tools, activity, and token data
without changing the visible output.

Codex combines account token activity with local session metadata. Account
activity is authoritative for quota-level trends. Local state and rollout files
provide session, tool, model, cached-input, output, and reasoning-token detail
when available. Provider-specific labels make the scope explicit.

## Backend Architecture

### Generic Tauri commands

Replace the Claude-specific frontend contract with:

- `get_provider_limits(provider)`
- `get_provider_stats(provider)`

Keeping limits and statistics separate preserves their different refresh
cadences. Local file updates refresh statistics without forcing a network
request.

### Claude adapter

Move the current Claude parsing and usage API behavior behind a Claude adapter.
Its output is normalized at the command boundary. Existing last-known-good
behavior remains intact.

### Codex adapter

The Codex adapter lazily launches one persistent `codex app-server` subprocess
over stdio and performs the documented JSON-RPC initialization handshake.

It calls:

- `account/rateLimits/read` for live quota windows and optional account
  metadata.
- `account/usage/read` for account token-activity summary and daily buckets.

Reference:
<https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md>

The process manager:

- Starts only when Codex is selected or refreshed.
- Serializes requests and correlates responses by request ID.
- Applies bounded startup and request timeouts.
- Ignores unrelated notifications while awaiting a response.
- Restarts the child once after a transport failure and retries the request.
- Terminates the child when QuotaGlass exits.

QuotaGlass never reads Codex's authentication file. Codex remains responsible
for authentication and token refresh.

### Local Codex data

Local detail is read from Codex's supported state and session locations. The
scanner extracts metadata and usage events only; it does not retain prompt,
assistant, command-output, or file-content text.

To avoid repeated full-history scans, QuotaGlass keeps a usage-summary cache
keyed by file path and modification metadata. Changed files are reparsed and
unchanged summaries are reused. Malformed or concurrently written final lines
are skipped and retried on the next refresh.

### File watching

Watch both provider roots:

- `~/.claude/projects`
- `~/.codex/sessions`

Debounced change events include the provider identifier. The frontend refreshes
statistics only for the provider affected by the event.

### Caching

Last-known-good normalized limits and statistics are stored separately per
provider under the QuotaGlass support directory. Cache records contain usage
summaries only and never credentials or conversation content.

## Frontend Interaction

Store one `ProviderId` and one `ViewMode`:

- Providers: `claude`, `codex`
- Views: `superCompact`, `compact`, `detailed`

This replaces the existing independent `ultra` and `expanded` booleans and
prevents invalid combinations.

### Provider switching

- `Control+Option+P` cycles Claude and Codex globally.
- A distinct switch button in the widget header performs the same action and
  uses the shared header-button styling.
- The selected provider persists across launches.
- Switching triggers an immediate provider-specific refresh.
- The widget height transitions smoothly when the provider contents have
  different intrinsic heights.
- A hidden widget remains hidden; `Cmd+Shift+U` remains the explicit Show/Hide
  shortcut.

### View switching

- `Control+Option+V` cycles Super compact, Compact, and Detailed globally.
- Existing header controls keep their familiar visible behavior.
- The selected view persists across launches.
- Window resizing and corner anchoring continue to animate as they do now.

### Shortcut lifecycle

Shortcuts are registered only while QuotaGlass runs and are released on exit.
Registration failure is logged but does not prevent startup. Tooltips expose
the shortcut combinations without adding new visible controls.

## Icons

Claude retains its current inline icon. Codex uses the supplied
`codex-color-no-bg.svg`, copied into the project as a frontend asset. The icon
changes with the selected provider, including in super-compact mode.

The app and tray icons remain visually unchanged in this feature to avoid
expanding into a brand-identity redesign.

## Error Handling

- Codex subprocess startup and RPC calls use bounded timeouts.
- A transport failure gets one automatic restart and retry.
- Live failures fall back to last-known-good data and mark it cached.
- Local Codex activity remains available when account usage fails.
- Optional windows, credits, monthly limits, and token fields are omitted when
  absent.
- Missing Codex installation or authentication produces a concise
  provider-specific error without affecting Claude.
- One provider failure never blocks switching to the other provider.

## Verification

Automated checks:

- Rust parser tests for current Codex rate-limit, account-usage, null-window,
  and rollout token-event shapes.
- Provider normalization and three-state cycling checks.
- TypeScript/Vite production build.
- Rust formatting, tests, and checks.

Runtime checks:

- Live `codex app-server` rate-limit and usage smoke requests.
- Provider switching through the header and global shortcut.
- All three view transitions through controls and global shortcut.
- Provider/view persistence across relaunch.
- Dynamic two-window Codex and three-window Claude rendering.
- Window sizing and corner anchoring in each provider/view combination.
- Release build and installation under the QuotaGlass name.

## Rename and Release Sequence

1. Implement and verify from the current workspace path.
2. Build and install QuotaGlass.
3. Confirm legacy preference migration and login-item behavior.
4. Rename the GitHub repository to `quotaglass` and update the remote URL.
5. Rename the local project directory to `quotaglass`.

Keeping filesystem and remote renames until the end prevents workspace movement
from interrupting implementation or invalidating active build paths.
