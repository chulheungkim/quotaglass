# QuotaGlass Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename the widget to QuotaGlass and add provider-neutral Claude/Codex usage data with global provider and view shortcuts.

**Architecture:** Rust provider adapters normalize Claude and Codex into shared limit/statistics contracts. Codex uses one lazily managed `codex app-server` JSON-RPC child plus metadata-only local rollout summaries, while React consumes only provider-neutral arrays and a single three-state view mode.

**Tech Stack:** Tauri 2, Rust 2021, React 19, TypeScript 6, Vite 8, pnpm, Codex app-server protocol v0.145.0.

## Global Constraints

- Preserve the current layout, colors, typography, and overall visual design.
- Product name is `QuotaGlass`; package and repository name are `quotaglass`.
- Bundle identifier and support directory are `com.chulheong.quotaglass`.
- Tagline is `AI agent usage at a glance.`
- Providers in this release are exactly `claude` and `codex`.
- Global shortcuts are `Control+Option+P` for provider cycling and `Control+Option+V` for view cycling.
- Existing `Cmd+Shift+U` remains the Show/Hide shortcut.
- QuotaGlass must never read or persist Codex OAuth credentials or conversation text.
- Do not add a runtime dependency; use `std`, existing crates, and the installed `codex` executable.
- Preserve the untracked root `AGENTS.md`; do not stage or modify it.

---

## File Structure

### New files

- `src-tauri/src/providers/mod.rs` — shared serialized provider contracts and dispatch.
- `src-tauri/src/providers/claude.rs` — existing Claude limits/statistics implementation normalized to shared contracts.
- `src-tauri/src/providers/codex.rs` — Codex account-response parsing, local rollout summaries, and cache.
- `src-tauri/src/codex_rpc.rs` — persistent stdio JSON-RPC process manager.
- `src/provider.ts` — provider/view constants, persistence, and pure cycling helpers.
- `src/assets/codex-color-no-bg.svg` — supplied Codex provider icon.

### Modified files

- `src-tauri/src/lib.rs` — app wiring, generic commands, migration, shortcuts, and dual-provider watcher.
- `src-tauri/src/main.rs` — renamed Rust library entry point.
- `src-tauri/Cargo.toml` and `src-tauri/Cargo.lock` — QuotaGlass package/library identity only.
- `src/types.ts` — provider-neutral frontend contracts.
- `src/App.tsx` — provider/view state, generic rendering, provider switch, and shortcut events.
- `src/styles.css` — only button-reset/icon sizing needed for the clickable provider title.
- `package.json` — package identity only.
- `src-tauri/tauri.conf.json` — product, title, and bundle identity.
- `index.html` — document title.
- `scripts/deploy.sh` — QuotaGlass install plus legacy app replacement.
- `README.md` and `CLAUDE.md` — current identity, architecture, commands, and provider behavior.

---

### Task 1: Shared Provider Contracts and Pure Frontend State

**Files:**

- Create: `src-tauri/src/providers/mod.rs`
- Create: `src/provider.ts`
- Modify: `src/types.ts`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**

- Produces Rust `ProviderId`, `ProviderLimits`, and `ProviderStats`.
- Produces TypeScript `ProviderId`, `ViewMode`, `nextProvider`, `nextViewMode`, `readProvider`, and `readViewMode`.
- Later tasks implement `claude::get_limits`, `claude::get_stats`, `codex::get_limits`, and `codex::get_stats`.

- [ ] **Step 1: Add failing Rust contract tests**

Add tests in `src-tauri/src/providers/mod.rs` before the implementation:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_ids_deserialize_from_frontend_values() {
        assert_eq!(
            serde_json::from_str::<ProviderId>("\"claude\"").unwrap(),
            ProviderId::Claude
        );
        assert_eq!(
            serde_json::from_str::<ProviderId>("\"codex\"").unwrap(),
            ProviderId::Codex
        );
    }

    #[test]
    fn limits_serialize_as_dynamic_windows() {
        let limits = ProviderLimits {
            provider: ProviderId::Codex,
            windows: vec![ProviderLimit {
                id: "primary".into(),
                title: "Current session".into(),
                utilization: Some(25.0),
                resets_at: Some("2026-07-24T12:00:00Z".into()),
                window_minutes: Some(300),
            }],
            stale: false,
            cached_at: None,
            plan: Some("plus".into()),
            credit_balance: None,
        };
        let json = serde_json::to_value(limits).unwrap();
        assert_eq!(json["provider"], "codex");
        assert_eq!(json["windows"].as_array().unwrap().len(), 1);
        assert_eq!(json["windows"][0]["windowMinutes"], 300);
    }
}
```

- [ ] **Step 2: Run the tests and confirm the missing-type failure**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml providers::tests
```

Expected: compilation fails because `ProviderId` and the provider response types do not exist.

- [ ] **Step 3: Implement the shared Rust contracts**

Create these exact public types in `src-tauri/src/providers/mod.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Hash, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Claude,
    Codex,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderLimit {
    pub id: String,
    pub title: String,
    pub utilization: Option<f64>,
    pub resets_at: Option<String>,
    pub window_minutes: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderLimits {
    pub provider: ProviderId,
    pub windows: Vec<ProviderLimit>,
    pub stale: bool,
    pub cached_at: Option<u64>,
    pub plan: Option<String>,
    pub credit_balance: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryMetric {
    pub label: String,
    pub value: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityPoint {
    pub date: String,
    pub value: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakdownRow {
    pub key: String,
    pub value: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStats {
    pub provider: ProviderId,
    pub metrics: Vec<SummaryMetric>,
    pub activity_label: String,
    pub daily14: Vec<ActivityPoint>,
    pub breakdown_label: String,
    pub breakdown: Vec<BreakdownRow>,
    pub footer: Vec<String>,
    pub since: Option<String>,
    pub data_scope: Option<String>,
    pub last_updated: String,
}
```

Declare `mod providers;` in `lib.rs`. Task 2 adds `providers::claude`, and Task
4 adds `providers::codex` plus the final adapter dispatch functions.

- [ ] **Step 4: Implement TypeScript state and contracts**

Create `src/provider.ts`:

```ts
export const PROVIDERS = ["claude", "codex"] as const;
export type ProviderId = (typeof PROVIDERS)[number];

export const VIEW_MODES = ["superCompact", "compact", "detailed"] as const;
export type ViewMode = (typeof VIEW_MODES)[number];

export function nextProvider(provider: ProviderId): ProviderId {
  return provider === "claude" ? "codex" : "claude";
}

export function nextViewMode(view: ViewMode): ViewMode {
  const index = VIEW_MODES.indexOf(view);
  return VIEW_MODES[(index + 1) % VIEW_MODES.length];
}

export function readProvider(): ProviderId {
  return localStorage.getItem("widget-provider") === "codex"
    ? "codex"
    : "claude";
}

export function readViewMode(): ViewMode {
  const saved = localStorage.getItem("widget-view");
  if (saved === "superCompact" || saved === "compact" || saved === "detailed") {
    return saved;
  }
  return localStorage.getItem("widget-ultra") === "1"
    ? "superCompact"
    : "compact";
}

export function saveProvider(provider: ProviderId): void {
  localStorage.setItem("widget-provider", provider);
}

export function saveViewMode(view: ViewMode): void {
  localStorage.setItem("widget-view", view);
}
```

Replace `src/types.ts` with interfaces that mirror the Rust structures:

```ts
import type { ProviderId } from "./provider";

export interface ProviderLimit {
  id: string;
  title: string;
  utilization: number | null;
  resetsAt: string | null;
  windowMinutes: number | null;
}

export interface ProviderLimits {
  provider: ProviderId;
  windows: ProviderLimit[];
  stale: boolean;
  cachedAt: number | null;
  plan: string | null;
  creditBalance: string | null;
}

export interface SummaryMetric {
  label: string;
  value: number;
}

export interface ActivityPoint {
  date: string;
  value: number;
}

export interface BreakdownRow {
  key: string;
  value: number;
}

export interface ProviderStats {
  provider: ProviderId;
  metrics: SummaryMetric[];
  activityLabel: string;
  daily14: ActivityPoint[];
  breakdownLabel: string;
  breakdown: BreakdownRow[];
  footer: string[];
  since: string | null;
  dataScope: string | null;
  lastUpdated: string;
}
```

- [ ] **Step 5: Run focused checks**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml providers::tests
pnpm build
```

Expected: Rust provider contract tests pass. The frontend may still report old
`UsageStats`/`RateLimits` imports in `App.tsx`; record those expected errors for
Task 6 without weakening the new types.

- [ ] **Step 6: Commit the shared contract**

```bash
git add -- src-tauri/src/providers/mod.rs src-tauri/src/lib.rs src/provider.ts src/types.ts
git commit -m "refactor(ui): add provider-neutral usage contracts"
```

---

### Task 2: Claude Adapter Extraction

**Files:**

- Create: `src-tauri/src/providers/claude.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: `src-tauri/src/providers/claude.rs`

**Interfaces:**

- Consumes `ProviderLimit`, `ProviderLimits`, `ProviderStats`, `SummaryMetric`, `ActivityPoint`, and `BreakdownRow`.
- Produces `pub fn get_limits(app_support_dir: &Path) -> Result<ProviderLimits, String>`.
- Produces `pub fn get_stats() -> Result<ProviderStats, String>`.

- [ ] **Step 1: Write Claude normalization tests**

Add:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_all_three_claude_windows() {
        let data = serde_json::json!({
            "five_hour": {"utilization": 10.0, "resets_at": "2026-07-24T10:00:00Z"},
            "seven_day": {"utilization": 20.0, "resets_at": "2026-07-28T10:00:00Z"},
            "seven_day_sonnet": {"utilization": 30.0, "resets_at": "2026-07-28T10:00:00Z"}
        });
        let windows = parse_live_windows(&data);
        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0].id, "five-hour");
        assert_eq!(windows[2].title, "Current week (Sonnet only)");
    }
}
```

- [ ] **Step 2: Confirm the parser test initially fails**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml providers::claude::tests
```

Expected: failure because `parse_live_windows` is not defined.

- [ ] **Step 3: Move existing Claude behavior behind the adapter**

Move the existing JSONL/stat-cache parsing, keychain lookup, curl request, and
last-known-limit cache from `lib.rs` into `providers/claude.rs`. Preserve the
current algorithms and labels. Normalize output as follows:

```rust
ProviderStats {
    provider: ProviderId::Claude,
    metrics: vec![
        SummaryMetric { label: "Messages".into(), value: today.messages },
        SummaryMetric { label: "Sessions".into(), value: today.sessions },
        SummaryMetric { label: "Tools".into(), value: today.tool_calls },
    ],
    activity_label: "14-Day Activity".into(),
    daily14: daily14
        .into_iter()
        .map(|day| ActivityPoint { date: day.date, value: day.messages })
        .collect(),
    breakdown_label: "Tokens by Model".into(),
    breakdown: model_tokens
        .into_iter()
        .map(|(key, value)| BreakdownRow { key, value })
        .collect(),
    footer: vec![
        format!("{} sessions", all_time.sessions),
        format!("{} messages", all_time.messages),
    ],
    since,
    data_scope: None,
    last_updated: today_str,
}
```

Use `app_support_dir.join("claude-rate-limits-cache.json")` for the renamed
cache. `parse_live_windows` returns windows in five-hour, weekly, Sonnet order
and merges cached missing values exactly as the current code does.

- [ ] **Step 4: Keep command compatibility while using the Claude adapter**

In `lib.rs`, delete the moved Claude implementation and keep the existing Tauri
command names temporarily:

```rust
#[tauri::command]
async fn get_rate_limits() -> Result<ProviderLimits, String> {
    let support = app_support_dir();
    tauri::async_runtime::spawn_blocking(move || providers::claude::get_limits(&support))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn get_usage_stats() -> Result<ProviderStats, String> {
    tauri::async_runtime::spawn_blocking(providers::claude::get_stats)
        .await
        .map_err(|error| error.to_string())?
}
```

Add `pub mod claude;` to `providers/mod.rs`. Task 5 replaces these compatibility
commands with the generic provider commands after both adapters exist.

- [ ] **Step 5: Verify Claude tests and Rust checks**

Run:

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml providers::claude::tests
cargo check --manifest-path src-tauri/Cargo.toml
```

Expected: all pass.

- [ ] **Step 6: Commit the Claude adapter**

```bash
git add -- src-tauri/src/providers/claude.rs src-tauri/src/providers/mod.rs src-tauri/src/lib.rs
git commit -m "refactor(ui): isolate claude usage provider"
```

---

### Task 3: Persistent Codex App-Server Client

**Files:**

- Create: `src-tauri/src/codex_rpc.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: `src-tauri/src/codex_rpc.rs`

**Interfaces:**

- Produces `#[derive(Clone, Default)] pub struct CodexAppServer`.
- Produces `pub fn request(&self, method: &str) -> Result<Value, String>`.
- Request params for `account/rateLimits/read` and `account/usage/read` are JSON `null`.

- [ ] **Step 1: Write JSON-RPC response-selection tests**

Add a pure helper test:

```rust
#[test]
fn response_for_id_ignores_notifications_and_other_ids() {
    let lines = [
        r#"{"method":"account/rateLimits/updated","params":{}}"#,
        r#"{"id":8,"result":{"ignored":true}}"#,
        r#"{"id":7,"result":{"rateLimits":{"primary":null}}}"#,
    ];
    let result = find_response(lines.into_iter(), 7).unwrap();
    assert!(result["rateLimits"].is_object());
}
```

- [ ] **Step 2: Confirm the helper test fails**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml codex_rpc::tests
```

Expected: failure because `find_response` does not exist.

- [ ] **Step 3: Implement the process manager**

Implement `CodexRpc` with:

```rust
struct CodexRpc {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    lines: std::sync::mpsc::Receiver<String>,
    next_id: u64,
}

#[derive(Clone, Default)]
pub struct CodexAppServer {
    inner: std::sync::Arc<std::sync::Mutex<Option<CodexRpc>>>,
}
```

`CodexRpc::spawn()` must:

1. Launch `codex app-server --stdio` with piped stdin/stdout and null stderr.
2. Start one reader thread using `BufRead::lines()` and an mpsc channel.
3. Send:

```json
{
  "id": 1,
  "method": "initialize",
  "params": {
    "clientInfo": {
      "name": "quotaglass",
      "title": "QuotaGlass",
      "version": "0.1.0"
    },
    "capabilities": { "experimentalApi": false }
  }
}
```

4. Wait at most 8 seconds for response ID `1`.
5. Send:

```json
{ "method": "initialized" }
```

`request(method)` increments the ID, writes one newline-delimited request with
`"params": null`, flushes stdin, and waits at most 8 seconds. It skips
notifications and unrelated IDs. JSON-RPC errors return their message.

`CodexAppServer::request` locks `inner`, creates a session lazily, retries
exactly once after killing and replacing a failed session, and returns the
second error unchanged. `Drop for CodexRpc` kills and waits for the child.

- [ ] **Step 4: Manage the state in Tauri**

In `run()` add:

```rust
.manage(CodexAppServer::default())
```

Task 5 clones `CodexAppServer` itself into each `spawn_blocking` closure; the
private `CodexRpc` type never crosses the module boundary.

- [ ] **Step 5: Run unit and live protocol checks**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml codex_rpc::tests
cargo check --manifest-path src-tauri/Cargo.toml
```

Then send the four protocol lines to `codex app-server --stdio` and verify both
methods return results:

```bash
printf '%s\n' \
  '{"id":1,"method":"initialize","params":{"clientInfo":{"name":"quotaglass","title":"QuotaGlass","version":"0.1.0"},"capabilities":{"experimentalApi":false}}}' \
  '{"method":"initialized"}' \
  '{"id":2,"method":"account/rateLimits/read","params":null}' \
  '{"id":3,"method":"account/usage/read","params":null}' \
  | codex app-server --stdio
```

Expected: responses for IDs `1`, `2`, and `3`; ID `2` contains `rateLimits` and
ID `3` contains `summary`.

- [ ] **Step 6: Commit the RPC manager**

```bash
git add -- src-tauri/src/codex_rpc.rs src-tauri/src/lib.rs
git commit -m "feat(ui): add persistent codex usage client"
```

---

### Task 4: Codex Limits, Account Usage, and Local Detail

**Files:**

- Create: `src-tauri/src/providers/codex.rs`
- Modify: `src-tauri/src/providers/mod.rs`
- Test: `src-tauri/src/providers/codex.rs`

**Interfaces:**

- Consumes `CodexAppServer::request`.
- Produces `pub fn get_limits(rpc, support_dir) -> Result<ProviderLimits, String>`.
- Produces `pub fn get_stats(rpc, support_dir) -> Result<ProviderStats, String>`.
- Persists `codex-limits-cache.json` and `codex-stats-cache.json`.

- [ ] **Step 1: Add failing account-response parser tests**

Use current v0.145.0 schema shapes:

```rust
#[test]
fn parses_codex_primary_secondary_and_account_metadata() {
    let value = serde_json::json!({
        "rateLimits": {
            "limitId": "codex",
            "primary": {"usedPercent": 25, "windowDurationMins": 300, "resetsAt": 1784875200},
            "secondary": {"usedPercent": 18, "windowDurationMins": 10080, "resetsAt": 1785307200},
            "credits": {"hasCredits": true, "unlimited": false, "balance": "42.50"},
            "planType": "plus"
        }
    });
    let limits = parse_limits(&value).unwrap();
    assert_eq!(limits.windows.len(), 2);
    assert_eq!(limits.windows[0].title, "Current session");
    assert_eq!(limits.windows[1].title, "Current week");
    assert_eq!(limits.plan.as_deref(), Some("plus"));
    assert_eq!(limits.credit_balance.as_deref(), Some("42.50"));
}

#[test]
fn parses_account_daily_token_buckets() {
    let value = serde_json::json!({
        "summary": {"lifetimeTokens": 123456, "currentStreakDays": 4},
        "dailyUsageBuckets": [
            {"startDate": "2026-07-23", "tokens": 1200},
            {"startDate": "2026-07-24", "tokens": 3400}
        ]
    });
    let usage = parse_account_usage(&value).unwrap();
    assert_eq!(usage.daily[1].value, 3400);
    assert_eq!(usage.lifetime_tokens, Some(123456));
}

#[test]
fn null_windows_are_omitted() {
    let value = serde_json::json!({
        "rateLimits": {"primary": null, "secondary": null}
    });
    assert!(parse_limits(&value).unwrap().windows.is_empty());
}
```

- [ ] **Step 2: Add failing metadata-only rollout test**

Write a temporary JSONL fixture using `std::env::temp_dir()` and assert:

```rust
let summary = parse_rollout(&path).unwrap();
assert_eq!(summary.messages, 1);
assert_eq!(summary.tools, 1);
assert_eq!(summary.output_by_model["gpt-5.6"], 75);
```

The fixture contains only:

```json
{"timestamp":"2026-07-24T01:00:00Z","type":"session_meta","payload":{"id":"session-1","model_provider":"openai"}}
{"timestamp":"2026-07-24T01:01:00Z","type":"turn_context","payload":{"model":"gpt-5.6"}}
{"timestamp":"2026-07-24T01:01:10Z","type":"response_item","payload":{"type":"message","role":"user","content":[]}}
{"timestamp":"2026-07-24T01:01:11Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{}","call_id":"call-1"}}
{"timestamp":"2026-07-24T01:01:12Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1000,"cached_input_tokens":800,"output_tokens":75,"reasoning_output_tokens":20,"total_tokens":1075}},"rate_limits":null}}
```

- [ ] **Step 3: Confirm all Codex tests fail**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml providers::codex::tests
```

Expected: failure because the Codex parsers do not exist.

- [ ] **Step 4: Implement live limit and account-usage parsing**

Parse `usedPercent`, `windowDurationMins`, and Unix-seconds `resetsAt`.
Convert reset seconds with:

```rust
fn unix_seconds_to_iso(seconds: i64) -> String {
    // Use the existing civil-from-days conversion for the date and derive
    // HH:MM:SS from seconds.rem_euclid(86400).
}
```

Select `rateLimitsByLimitId["codex"]` when present; otherwise use
`rateLimits`. Label primary `Current session` and secondary `Current week`.

Parse `summary.lifetimeTokens`, `summary.currentStreakDays`,
`summary.peakDailyTokens`, and `dailyUsageBuckets[]`. Sort buckets by date and
retain the last 14.

- [ ] **Step 5: Implement safe local rollout summaries and cache**

For each `~/.codex/sessions/**/*.jsonl`:

- Count one session from `session_meta`.
- Count user messages from `response_item` message records with role `user`.
- Count tools from `function_call` and `custom_tool_call`.
- Track the current model from `turn_context.payload.model`.
- Sum only `event_msg.payload.info.last_token_usage.output_tokens` into the
  current model; never sum cumulative `total_token_usage`.
- Keep counts and tokens only, never message/content/arguments text.

Cache each file summary by absolute path, byte length, and modified Unix
seconds. Reuse unchanged summaries and write the merged cache atomically through
a temporary sibling followed by `fs::rename`.

- [ ] **Step 6: Normalize Codex statistics and fallback behavior**

Return:

```rust
ProviderStats {
    provider: ProviderId::Codex,
    metrics: vec![
        SummaryMetric { label: "Tokens".into(), value: today_account_tokens },
        SummaryMetric { label: "Sessions".into(), value: today_local_sessions },
        SummaryMetric { label: "Tools".into(), value: today_local_tools },
    ],
    activity_label: "14-Day Token Activity".into(),
    daily14: account_usage.daily,
    breakdown_label: "Output Tokens by Model".into(),
    breakdown: local.output_by_model,
    footer,
    since: local.first_date,
    data_scope: Some("Account tokens · local session detail".into()),
    last_updated: today_date(),
}
```

If account usage fails, use cached account buckets and mark the scope
`Cached account tokens · local session detail`. If no account cache exists,
return local detail with an empty activity series rather than discarding it.
Limits use last-known-good live windows; if neither live nor cached windows
exist, return the app-server error.

- [ ] **Step 7: Run Codex tests and full Rust checks**

Run:

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml providers::codex::tests
cargo test --manifest-path src-tauri/Cargo.toml
cargo check --manifest-path src-tauri/Cargo.toml
```

Expected: all pass.

- [ ] **Step 8: Commit the Codex provider**

```bash
git add -- src-tauri/src/providers/codex.rs src-tauri/src/providers/mod.rs
git commit -m "feat(ui): add codex usage provider"
```

---

### Task 5: Tauri Shortcuts, Provider Events, and Legacy Migration

**Files:**

- Modify: `src-tauri/src/lib.rs`
- Test: `src-tauri/src/lib.rs`

**Interfaces:**

- Emits `provider-shortcut` and `view-shortcut` with no payload.
- Emits `usage-updated` with payload `"claude"` or `"codex"`.
- Produces `migrate_legacy_data()` called before Tauri builder construction.
- Produces generic `get_provider_limits(provider)` and `get_provider_stats(provider)` commands.

- [ ] **Step 1: Add migration helper tests**

Factor recursive copy selection into a pure helper and test that existing new
files win over legacy files:

```rust
#[test]
fn migration_does_not_overwrite_new_state() {
    let root = unique_temp_dir();
    let old = root.join("old");
    let new = root.join("new");
    fs::create_dir_all(&old).unwrap();
    fs::create_dir_all(&new).unwrap();
    fs::write(old.join(".corner"), "tl").unwrap();
    fs::write(new.join(".corner"), "br").unwrap();
    copy_dir_if_missing(&old, &new).unwrap();
    assert_eq!(fs::read_to_string(new.join(".corner")).unwrap(), "br");
}
```

- [ ] **Step 2: Implement pre-builder migration**

At the beginning of `run()`, before `tauri::Builder::default()`, migrate:

- `~/Library/Application Support/com.chulheong.claudeusage` into the new support
  directory without overwriting new files.
- `~/Library/WebKit/com.chulheong.claudeusage` into
  `~/Library/WebKit/com.chulheong.quotaglass` only when the new directory does
  not exist.

Do not copy `.login-registered`; the new bundle must register its own login
item. Recursive copying uses `fs::read_dir`, `fs::create_dir_all`, and
`fs::copy`; no shell process is launched.

- [ ] **Step 3: Add generic provider command dispatch**

Add `pub mod codex;` and these functions in `providers/mod.rs`:

```rust
pub fn get_limits(
    provider: ProviderId,
    support: &Path,
    codex: &CodexAppServer,
) -> Result<ProviderLimits, String> {
    match provider {
        ProviderId::Claude => claude::get_limits(support),
        ProviderId::Codex => codex::get_limits(codex, support),
    }
}

pub fn get_stats(
    provider: ProviderId,
    support: &Path,
    codex: &CodexAppServer,
) -> Result<ProviderStats, String> {
    match provider {
        ProviderId::Claude => claude::get_stats(),
        ProviderId::Codex => codex::get_stats(codex, support),
    }
}
```

Replace the compatibility commands in `lib.rs` with:

```rust
#[tauri::command]
async fn get_provider_limits(
    provider: ProviderId,
    codex: tauri::State<'_, CodexAppServer>,
) -> Result<ProviderLimits, String> {
    let support = app_support_dir();
    let codex = codex.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        providers::get_limits(provider, &support, &codex)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn get_provider_stats(
    provider: ProviderId,
    codex: tauri::State<'_, CodexAppServer>,
) -> Result<ProviderStats, String> {
    let support = app_support_dir();
    let codex = codex.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        providers::get_stats(provider, &support, &codex)
    })
    .await
    .map_err(|error| error.to_string())?
}
```

Register only these generic names in `tauri::generate_handler!`.

- [ ] **Step 4: Register and handle all global shortcuts**

Define constructors because `Shortcut::new` is not a const function:

```rust
fn toggle_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyU)
}

fn provider_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyP)
}

fn view_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyV)
}
```

In the handler:

```rust
if event.state() != ShortcutState::Pressed {
    return;
}
if shortcut == &toggle_shortcut() {
    toggle_window(app);
} else if shortcut == &provider_shortcut() {
    let _ = app.emit("provider-shortcut", ());
} else if shortcut == &view_shortcut() {
    let _ = app.emit("view-shortcut", ());
}
```

Register all three individually in setup and log failures with `eprintln!`
without returning an error.

- [ ] **Step 5: Watch both provider roots**

Use one `notify` watcher and channel carrying `ProviderId`. Watch existing
provider directories only. Determine the provider from the event path prefix,
debounce separately per provider for 1.5 seconds, and emit:

```rust
let _ = app_handle.emit("usage-updated", provider);
```

- [ ] **Step 6: Verify Rust behavior**

Run:

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
cargo check --manifest-path src-tauri/Cargo.toml
```

Expected: all pass.

- [ ] **Step 7: Commit Tauri integration**

```bash
git add -- src-tauri/src/lib.rs
git commit -m "feat(ui): add provider and view shortcuts"
```

---

### Task 6: Provider-Neutral React UI and Codex Icon

**Files:**

- Create: `src/assets/codex-color-no-bg.svg`
- Modify: `src/App.tsx`
- Modify: `src/styles.css`

**Interfaces:**

- Consumes generic Tauri commands and shortcut/provider events.
- Persists provider in `widget-provider` and view in `widget-view`.
- Preserves existing visible layout and animations.

- [ ] **Step 1: Copy the supplied icon into the repository**

Create `src/assets/codex-color-no-bg.svg` with the exact contents of:

```text
/Users/chulheongkim/Downloads/codex-color-no-bg.svg
```

Do not alter its 12×12 viewBox, gradient, or path.

- [ ] **Step 2: Replace boolean mode state with provider/view state**

In `App.tsx`, initialize:

```ts
const [provider, setProvider] = useState<ProviderId>(readProvider);
const [view, setView] = useState<ViewMode>(readViewMode);
const ultra = view === "superCompact";
const expanded = view === "detailed";
```

Use helpers:

```ts
const switchProvider = useCallback(() => {
  setProvider((current) => {
    const next = nextProvider(current);
    saveProvider(next);
    return next;
  });
}, []);

const cycleView = useCallback(() => {
  setView((current) => {
    const next = nextViewMode(current);
    saveViewMode(next);
    return next;
  });
}, []);
```

Existing buttons set valid enum values:

```ts
const toggleUltra = () =>
  setAndSaveView(view === "superCompact" ? "compact" : "superCompact");
const toggleDetailed = () =>
  setAndSaveView(view === "detailed" ? "compact" : "detailed");
```

- [ ] **Step 3: Call generic commands and provider-specific refreshes**

Invoke:

```ts
invoke<ProviderLimits>("get_provider_limits", { provider });
invoke<ProviderStats>("get_provider_stats", { provider });
```

Callbacks depend on `provider`. Clear stale errors when provider changes, but
retain previously rendered data only when `data.provider === provider`.

Listen to:

```ts
listen("provider-shortcut", switchProvider);
listen("view-shortcut", cycleView);
listen<ProviderId>("usage-updated", ({ payload }) => {
  if (payload === provider) void loadStats();
});
```

- [ ] **Step 4: Render provider identity and dynamic windows**

Use the current Claude inline SVG and:

```tsx
function ProviderIcon({ provider }: { provider: ProviderId }) {
  return provider === "claude" ? (
    <ClaudeIcon />
  ) : (
    <img className="provider-icon" src={codexIcon} alt="" aria-hidden="true" />
  );
}
```

Make the existing title area a button:

```tsx
<button
  className="title provider-switch"
  onClick={switchProvider}
  title="Switch provider (⌃⌥P)"
>
  <ProviderIcon provider={provider} />
  <span className={`title-text${ultra ? " hidden" : ""}`}>
    {provider === "claude" ? "Claude Code" : "Codex"}
  </span>
</button>
```

Render both compact and super-compact bars from `limits?.windows ?? []`. Render
compact titles from `window.title`; never reference `fiveHour`, `sevenDay`, or
`sevenDaySonnet`.

- [ ] **Step 5: Render generic detailed statistics**

Replace fixed stats with `stats.metrics.slice(0, 3)`. Plot `day.value`, use
`stats.activityLabel`, and render `stats.breakdown` with
`stats.breakdownLabel`. The footer joins `stats.footer`, optional `since`, and
optional `dataScope` using the existing separator styling.

Keep the existing model label/color helpers; extend `modelLabel` to remove both
`claude-` and common `gpt-` separators without hard-coding current Codex model
versions.

- [ ] **Step 6: Add only minimal CSS**

Add:

```css
.provider-switch {
  appearance: none;
  background: none;
  border: 0;
  padding: 0;
  cursor: pointer;
  text-align: left;
}

.provider-icon {
  width: 16px;
  height: 16px;
  flex: 0 0 auto;
}
```

Do not change existing color, typography, spacing, card, bar, or animation
rules.

- [ ] **Step 7: Verify the frontend**

Run:

```bash
pnpm build
```

Expected: TypeScript and Vite complete with exit code 0.

- [ ] **Step 8: Commit the frontend integration**

```bash
git add -- src/assets/codex-color-no-bg.svg src/App.tsx src/styles.css src/provider.ts src/types.ts
git commit -m "feat(ui): add codex and global view switching"
```

---

### Task 7: QuotaGlass Identity, Install Migration, and Documentation

**Files:**

- Modify: `package.json`
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/Cargo.lock`
- Modify: `src-tauri/src/main.rs`
- Modify: `src-tauri/tauri.conf.json`
- Modify: `index.html`
- Modify: `scripts/deploy.sh`
- Modify: `README.md`
- Modify: `CLAUDE.md`

**Interfaces:**

- Produces `QuotaGlass.app`.
- Uses bundle/support identifier `com.chulheong.quotaglass`.
- Replaces installed `Claude Usage.app` only after a successful build.

- [ ] **Step 1: Rename manifest and runtime identities**

Apply these exact names:

```json
{
  "name": "quotaglass"
}
```

```toml
[package]
name = "quotaglass"
description = "AI agent usage at a glance"

[lib]
name = "quotaglass_lib"
```

Update `main.rs`:

```rust
fn main() {
    quotaglass_lib::run()
}
```

Update `tauri.conf.json`:

```json
{
  "productName": "QuotaGlass",
  "identifier": "com.chulheong.quotaglass",
  "app": {
    "windows": [
      {
        "title": "QuotaGlass"
      }
    ]
  }
}
```

Set the HTML title to `QuotaGlass`.

- [ ] **Step 2: Update deployment behavior**

Build first, then stop/replace applications so a failed build never removes the
working app. Use:

```bash
APP_NAME="QuotaGlass"
LEGACY_APP_NAME="Claude Usage"
INSTALL_DIR="$HOME/Applications"
INSTALLED_APP="$INSTALL_DIR/$APP_NAME.app"
LEGACY_INSTALLED_APP="$INSTALL_DIR/$LEGACY_APP_NAME.app"
BUILD_APP="$(dirname "$0")/../src-tauri/target/release/bundle/macos/$APP_NAME.app"
```

After a successful build:

- Stop both process names.
- Remove only the explicit legacy paths in `$HOME/Applications` and
  `/Applications`.
- Replace the explicit QuotaGlass install path.
- Copy with `ditto`, ad-hoc sign with `codesign`, and launch QuotaGlass.

- [ ] **Step 3: Refresh lockfiles without dependency changes**

Run:

```bash
cargo check --manifest-path src-tauri/Cargo.toml
pnpm install --lockfile-only --ignore-scripts
```

Expected: `Cargo.lock` root package identity changes to `quotaglass`; pnpm
reports an unchanged dependency graph and does not run lifecycle scripts.

- [ ] **Step 4: Update project documentation**

Rewrite README and CLAUDE architecture sections to describe:

- QuotaGlass identity and tagline.
- Claude and Codex adapters.
- Codex app-server requirement and local fallback.
- Provider/view shortcuts.
- Generic commands and both watched roots.
- New bundle identifier/support directory.

Keep command examples current and remove statements claiming no production
external process, because Codex intentionally launches its installed
app-server.

- [ ] **Step 5: Verify naming consistency**

Run:

```bash
rg -n "claude-usage-widget|Claude Usage|com\\.chulheong\\.claudeusage|claude_usage_widget_lib" \
  package.json src-tauri src index.html scripts README.md CLAUDE.md
```

Expected: matches only in explicit legacy migration/removal constants and
historical explanation.

- [ ] **Step 6: Run complete static verification**

Run:

```bash
pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml
cargo check --manifest-path src-tauri/Cargo.toml
git diff --check
```

Expected: all pass.

- [ ] **Step 7: Commit the rename and documentation**

```bash
git add -- package.json pnpm-lock.yaml src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/main.rs src-tauri/tauri.conf.json index.html scripts/deploy.sh README.md CLAUDE.md
git commit -m "feat(ui): rename app to quotaglass"
```

---

### Task 8: Runtime Verification and External Renames

**Files:**

- No source files unless verification discovers a defect.
- Rename local directory after all source verification.

**Interfaces:**

- Renames the GitHub repository to `quotaglass`.
- Renames `/Users/chulheongkim/claude-usage-widget` to `/Users/chulheongkim/quotaglass`.

- [ ] **Step 1: Build the release bundle**

Run:

```bash
pnpm tauri build --bundles app
```

Expected:

```text
src-tauri/target/release/bundle/macos/QuotaGlass.app
```

- [ ] **Step 2: Install and launch through the deployment script**

Run:

```bash
pnpm ship
```

Expected: QuotaGlass is installed under `~/Applications`, ad-hoc signed,
launched, and the explicit legacy application copy is removed.

- [ ] **Step 3: Perform runtime interaction checks**

Verify:

- Claude displays three compact/super-compact bars.
- Codex displays two live bars and account token activity.
- Clicking the title switches providers.
- `Control+Option+P` switches providers while another app is focused.
- `Control+Option+V` cycles super compact, compact, and detailed.
- `Cmd+Shift+U` still hides/shows the widget.
- Relaunch preserves provider and view.
- Every provider/view combination resizes and remains corner-anchored.
- Cached data remains visible after temporarily making Codex unavailable.

- [ ] **Step 4: Inspect final repository state**

Run:

```bash
git status --short --branch
git log --oneline --decorate -8
git remote -v
```

Expected: only the user's untracked `AGENTS.md` remains outside committed task
changes.

- [ ] **Step 5: Rename the GitHub repository**

Resolve the current repository owner/name from `git remote get-url origin`.
Use the authenticated GitHub connector when available; otherwise use:

```bash
gh repo rename quotaglass
```

Then update the local remote to the canonical renamed URL returned by GitHub:

```bash
git remote set-url origin git@github.com:chulheungkim/quotaglass.git
```

Expected: GitHub reports repository name `quotaglass` and `git remote -v`
contains the renamed URL. Do not push the feature branch unless the user
separately authorizes publishing it.

- [ ] **Step 6: Rename the local project directory**

Stop any development process using the old path, then rename exactly:

```bash
mv /Users/chulheongkim/claude-usage-widget /Users/chulheongkim/quotaglass
```

Expected: the repository is present at `/Users/chulheongkim/quotaglass`, its
branch is `feat/quotaglass`, and `git status --short --branch` is unchanged.

- [ ] **Step 7: Report the completed handoff**

Report:

- Installed QuotaGlass app location.
- New local repository path and remote URL.
- Verification commands and outcomes.
- Any runtime fields omitted because the account did not provide them.
- The existing untracked `AGENTS.md` was preserved.
