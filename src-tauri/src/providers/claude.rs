use super::{
    ActivityPoint, BreakdownRow, ProviderId, ProviderLimit, ProviderLimits, ProviderStats,
    SummaryMetric,
};
use crate::{date_from_secs, days_from_civil, now_ms, today_date};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::UNIX_EPOCH,
};

// The usage endpoint budgets requests per account. When it pushes back we stop
// calling it for a while instead of retrying on every poll, focus event and
// manual refresh — retrying through a 429 is what keeps a card pinned to cache.
const RATE_LIMIT_BACKOFF_MS: u64 = 10 * 60 * 1000;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LimitWindow {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

// A window whose reset time has already passed describes a window that no
// longer exists, so it must never be rendered as current usage.
fn window_expired(window: &ProviderLimit, now: u64) -> bool {
    window
        .resets_at
        .as_deref()
        .and_then(iso_to_epoch_ms)
        .is_some_and(|resets_at| resets_at <= now)
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CachedLimits {
    // The set of windows is server-defined and changes as models come and go,
    // so the cache stores whatever was last returned rather than a fixed shape.
    #[serde(default)]
    windows: Vec<ProviderLimit>,
    saved_at: Option<u64>,
    retry_after: Option<u64>,
    // Shape written before the endpoint exposed model-scoped windows. Read so
    // an upgrade with no network still has something to show; never written.
    #[serde(default, skip_serializing)]
    five_hour: Option<LimitWindow>,
    #[serde(default, skip_serializing)]
    seven_day: Option<LimitWindow>,
}

impl CachedLimits {
    fn windows(&self) -> Vec<ProviderLimit> {
        if !self.windows.is_empty() {
            return self.windows.clone();
        }
        [
            ("five-hour", "Current session", 300, &self.five_hour),
            (
                "seven-day",
                "Current week (all models)",
                10_080,
                &self.seven_day,
            ),
        ]
        .into_iter()
        .filter_map(|(id, title, minutes, window)| {
            let window = window.as_ref()?;
            Some(ProviderLimit {
                id: id.to_string(),
                title: title.to_string(),
                utilization: window.utilization,
                resets_at: window.resets_at.clone(),
                window_minutes: Some(minutes),
            })
        })
        .collect()
    }
}

struct UsageResponse {
    status: u32,
    body: String,
}

pub fn get_limits(support_dir: &Path) -> Result<ProviderLimits, String> {
    let cache_path = support_dir.join("claude-rate-limits-cache.json");
    let legacy_cache_path = support_dir.join("rate-limits-cache.json");
    let cache: CachedLimits = load_json(&cache_path)
        .or_else(|| load_json(&legacy_cache_path))
        .unwrap_or_default();

    if cache.retry_after.is_some_and(|until| now_ms() < until) {
        return limits_from_cache(&cache, "Usage endpoint rate limited");
    }

    let token = match read_oauth_token() {
        Some(token) => token,
        None => {
            return limits_from_cache(&cache, "No Claude Code OAuth credentials found");
        }
    };
    let response = match fetch_usage(&token) {
        Ok(response) => response,
        Err(error) => return limits_from_cache(&cache, &error),
    };
    if response.status == 429 {
        save_json_atomic(
            &cache_path,
            &CachedLimits {
                retry_after: Some(now_ms() + RATE_LIMIT_BACKOFF_MS),
                ..cache.clone()
            },
        );
        return limits_from_cache(&cache, "Usage endpoint rate limited");
    }
    let data: Value = match serde_json::from_str(&response.body) {
        Ok(data) => data,
        Err(error) => return limits_from_cache(&cache, &format!("JSON error: {error}")),
    };
    // Errors arrive both as `{"type":"error","error":{…}}` and as a bare
    // `{"error":{…}}` (rate limits and gateway errors use the latter), so key
    // off the error object itself rather than the optional discriminator.
    if let Some(message) = api_error_message(&data) {
        return limits_from_cache(&cache, &message);
    }
    if response.status != 200 {
        return limits_from_cache(&cache, &format!("Usage endpoint HTTP {}", response.status));
    }

    // The fetch succeeded, so the response is authoritative: a window the API
    // omits is a window with no usage, not one we failed to read. Merging a
    // cached window back in here is what used to mark the whole card stale
    // forever once a single window (Sonnet-only) went quiet.
    let live = parse_live_windows(&data);
    save_json_atomic(
        &cache_path,
        &CachedLimits {
            windows: live.clone(),
            saved_at: Some(now_ms()),
            retry_after: None,
            five_hour: None,
            seven_day: None,
        },
    );
    Ok(ProviderLimits {
        provider: ProviderId::Claude,
        windows: live,
        stale: false,
        cached_at: None,
        stale_reason: None,
        plan: None,
        credit_balance: None,
    })
}

fn api_error_message(data: &Value) -> Option<String> {
    let error = data.get("error")?;
    if error.is_null() {
        return None;
    }
    Some(
        error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Usage unavailable")
            .to_string(),
    )
}

fn fetch_usage(token: &str) -> Result<UsageResponse, String> {
    let output = Command::new("/usr/bin/curl")
        .args([
            "-s",
            "--max-time",
            "8",
            "-w",
            "\n%{http_code}",
            "-H",
            &format!("Authorization: Bearer {token}"),
            "-H",
            "anthropic-beta: oauth-2025-04-20",
            "https://api.anthropic.com/api/oauth/usage",
        ])
        .output()
        .map_err(|error| format!("curl error: {error}"))?;
    let raw = String::from_utf8(output.stdout).map_err(|error| format!("UTF-8 error: {error}"))?;
    let (body, status) = raw
        .rsplit_once('\n')
        .ok_or_else(|| "Usage endpoint returned no response".to_string())?;
    Ok(UsageResponse {
        status: status.trim().parse().unwrap_or(0),
        body: body.to_string(),
    })
}

pub fn get_stats() -> Result<ProviderStats, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
    let cache_path = format!("{home}/.claude/stats-cache.json");
    let projects_dir = format!("{home}/.claude/projects");
    let cache: Value = fs::read_to_string(cache_path)
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or(Value::Null);
    let last_computed = cache
        .get("lastComputedDate")
        .and_then(Value::as_str)
        .unwrap_or("2000-01-01")
        .to_string();
    let mut messages: HashMap<String, i64> = HashMap::new();
    let mut sessions: HashMap<String, HashSet<String>> = HashMap::new();
    let mut tools: HashMap<String, i64> = HashMap::new();
    let mut model_tokens: HashMap<String, i64> = HashMap::new();

    if let Ok(entries) = fs::read_dir(projects_dir) {
        for entry in entries.flatten() {
            let directory = entry.path();
            if !directory.is_dir() {
                continue;
            }
            if let Ok(files) = fs::read_dir(directory) {
                for file in files.flatten() {
                    let path = file.path();
                    if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
                        continue;
                    }
                    if let Some(date) = file_mtime_date(&path) {
                        if date.as_str() <= last_computed.as_str() {
                            continue;
                        }
                    }
                    process_file(
                        &path,
                        &last_computed,
                        &mut messages,
                        &mut sessions,
                        &mut tools,
                        &mut model_tokens,
                    );
                }
            }
        }
    }

    let mut daily: HashMap<String, (i64, i64, i64)> = HashMap::new();
    if let Some(activity) = cache.get("dailyActivity").and_then(Value::as_array) {
        for day in activity {
            let date = day.get("date").and_then(Value::as_str).unwrap_or("");
            if date.is_empty() {
                continue;
            }
            daily.insert(
                date.to_string(),
                (
                    day.get("messageCount").and_then(Value::as_i64).unwrap_or(0),
                    day.get("sessionCount").and_then(Value::as_i64).unwrap_or(0),
                    day.get("toolCallCount")
                        .and_then(Value::as_i64)
                        .unwrap_or(0),
                ),
            );
        }
    }
    let mut delta_dates: HashSet<String> = HashSet::new();
    delta_dates.extend(messages.keys().cloned());
    delta_dates.extend(sessions.keys().cloned());
    delta_dates.extend(tools.keys().cloned());
    for date in delta_dates {
        let target = daily.entry(date.clone()).or_default();
        target.0 += messages.get(&date).copied().unwrap_or(0);
        target.1 += sessions
            .get(&date)
            .map(|sessions| sessions.len() as i64)
            .unwrap_or(0);
        target.2 += tools.get(&date).copied().unwrap_or(0);
    }

    let mut dates: Vec<_> = daily.keys().cloned().collect();
    dates.sort();
    let daily14 = dates
        .iter()
        .rev()
        .take(14)
        .rev()
        .map(|date| ActivityPoint {
            date: date.clone(),
            value: daily[date].0,
        })
        .collect();
    let today = today_date();
    let today_values = daily.get(&today).copied().unwrap_or_default();

    let mut all_model_tokens = HashMap::new();
    if let Some(usage) = cache.get("modelUsage").and_then(Value::as_object) {
        for (model, values) in usage {
            all_model_tokens.insert(
                model.clone(),
                values
                    .get("outputTokens")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
            );
        }
    }
    for (model, output) in model_tokens {
        *all_model_tokens.entry(model).or_default() += output;
    }
    let mut breakdown: Vec<_> = all_model_tokens
        .into_iter()
        .map(|(key, value)| BreakdownRow { key, value })
        .collect();
    breakdown.sort_by(|a, b| b.value.cmp(&a.value));

    let delta_sessions: i64 = sessions.values().map(|set| set.len() as i64).sum();
    let total_sessions = cache
        .get("totalSessions")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        + delta_sessions;
    let total_messages = cache
        .get("totalMessages")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        + messages.values().sum::<i64>();
    let since = cache
        .get("firstSessionDate")
        .and_then(Value::as_str)
        .map(|value| value.chars().take(10).collect());

    Ok(ProviderStats {
        provider: ProviderId::Claude,
        metrics: vec![
            SummaryMetric {
                label: "Messages".to_string(),
                value: today_values.0,
            },
            SummaryMetric {
                label: "Sessions".to_string(),
                value: today_values.1,
            },
            SummaryMetric {
                label: "Tools".to_string(),
                value: today_values.2,
            },
        ],
        activity_label: "14-Day Activity".to_string(),
        daily14,
        breakdown_label: "Tokens by Model".to_string(),
        breakdown,
        footer: vec![
            format!("{total_sessions} sessions"),
            format!("{total_messages} messages"),
        ],
        since,
        data_scope: None,
        last_updated: today,
    })
}

// The endpoint describes windows two ways. The legacy top-level fields
// (five_hour, seven_day_sonnet, seven_day_opus, …) are a fixed per-model set
// that is now null for every model. The `limits` array is self-describing and
// carries model-scoped windows with their display name, which is how a scoped
// model such as Fable arrives. Prefer the array so a model added or retired
// server-side needs no code change here, and keep the legacy fields as a
// fallback for older responses.
fn parse_live_windows(data: &Value) -> Vec<ProviderLimit> {
    let entries = data.get("limits").and_then(Value::as_array);
    let mut windows: Vec<_> = entries
        .into_iter()
        .flatten()
        .filter_map(parse_limit_entry)
        .collect();
    if windows.is_empty() {
        return parse_legacy_windows(data);
    }
    windows.sort_by_key(|(rank, _)| *rank);
    windows.into_iter().map(|(_, window)| window).collect()
}

fn parse_limit_entry(entry: &Value) -> Option<(u8, ProviderLimit)> {
    let utilization = entry.get("percent").and_then(Value::as_f64);
    let resets_at = entry
        .get("resets_at")
        .and_then(Value::as_str)
        .map(str::to_string);
    let model = entry
        .get("scope")
        .and_then(|scope| scope.get("model"))
        .and_then(|model| model.get("display_name"))
        .and_then(Value::as_str);
    let (rank, id, title, minutes) = match entry.get("kind").and_then(Value::as_str)? {
        "session" => (
            0,
            "five-hour".to_string(),
            "Current session".to_string(),
            300,
        ),
        "weekly_all" => (
            1,
            "seven-day".to_string(),
            "Current week (all models)".to_string(),
            10_080,
        ),
        // A scoped window is only meaningful with a name to put on it.
        "weekly_scoped" => {
            let model = model?;
            (
                2,
                format!("seven-day-{}", window_slug(model)),
                format!("Current week ({model} only)"),
                10_080,
            )
        }
        _ => return None,
    };
    Some((
        rank,
        ProviderLimit {
            id,
            title,
            utilization,
            resets_at,
            window_minutes: Some(minutes),
        },
    ))
}

// Window ids reach the frontend as React keys, so they must be stable and
// unique per model regardless of how the display name is punctuated.
fn window_slug(model: &str) -> String {
    let slug: String = model
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    slug.trim_matches('-').to_string()
}

fn parse_legacy_windows(data: &Value) -> Vec<ProviderLimit> {
    [
        ("five-hour", "five_hour", "Current session", 300),
        (
            "seven-day",
            "seven_day",
            "Current week (all models)",
            10_080,
        ),
    ]
    .into_iter()
    .filter_map(|(id, key, title, minutes)| {
        let value = data.get(key)?;
        if value.is_null() {
            return None;
        }
        Some(ProviderLimit {
            id: id.to_string(),
            title: title.to_string(),
            utilization: value.get("utilization").and_then(Value::as_f64),
            resets_at: value
                .get("resets_at")
                .and_then(Value::as_str)
                .map(str::to_string),
            window_minutes: Some(minutes),
        })
    })
    .collect()
}

fn limits_from_cache(cache: &CachedLimits, error: &str) -> Result<ProviderLimits, String> {
    // Expired windows are dropped rather than rendered: a five-hour window whose
    // reset time has passed would otherwise show a long-dead utilization under a
    // reset time in the past, which reads as live data.
    let now = now_ms();
    let windows: Vec<_> = cache
        .windows()
        .into_iter()
        .filter(|window| !window_expired(window, now))
        .collect();
    if windows.is_empty() {
        return Err(error.to_string());
    }
    Ok(ProviderLimits {
        provider: ProviderId::Claude,
        windows,
        stale: true,
        cached_at: cache.saved_at,
        stale_reason: Some(error.to_string()),
        plan: None,
        credit_balance: None,
    })
}

#[derive(Clone)]
struct OauthToken {
    access_token: String,
    expires_at: Option<u64>,
}

// Claude Code writes the same `claudeAiOauth` blob either into the macOS
// Keychain or into `<config dir>/.credentials.json`, depending on how the CLI
// was installed and which storage it last migrated to. Reading only the
// Keychain silently yields no token the moment the CLI switches to the file,
// which pins the card to its last cached numbers indefinitely. Read both and
// keep whichever token is valid for longest.
fn read_oauth_token() -> Option<String> {
    let now = now_ms();
    let mut candidates: Vec<OauthToken> = [token_from_file(), token_from_keychain()]
        .into_iter()
        .flatten()
        .collect();
    candidates.sort_by_key(|token| std::cmp::Reverse(token.expires_at.unwrap_or(u64::MAX)));
    candidates
        .iter()
        .find(|token| token.expires_at.is_none_or(|expires_at| expires_at > now))
        // Every token is expired: send the newest anyway so the API reports the
        // authentication failure instead of the widget guessing at one.
        .or_else(|| candidates.first())
        .map(|token| token.access_token.clone())
}

fn parse_oauth_blob(raw: &str) -> Option<OauthToken> {
    let credentials: Value = serde_json::from_str(raw.trim()).ok()?;
    let oauth = credentials.get("claudeAiOauth")?;
    Some(OauthToken {
        access_token: oauth.get("accessToken")?.as_str()?.to_string(),
        expires_at: oauth.get("expiresAt").and_then(Value::as_u64),
    })
}

fn credentials_file_path() -> Option<PathBuf> {
    let directory = std::env::var("CLAUDE_CONFIG_DIR")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("{}/.claude", std::env::var("HOME").unwrap_or_default()));
    Some(PathBuf::from(directory).join(".credentials.json"))
}

fn token_from_file() -> Option<OauthToken> {
    parse_oauth_blob(&fs::read_to_string(credentials_file_path()?).ok()?)
}

fn token_from_keychain() -> Option<OauthToken> {
    let username = std::env::var("USER").unwrap_or_else(|_| "claude-code-user".to_string());
    let output = Command::new("/usr/bin/security")
        .args([
            "find-generic-password",
            "-a",
            &username,
            "-w",
            "-s",
            "Claude Code-credentials",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_oauth_blob(&String::from_utf8(output.stdout).ok()?)
}

// Parse the RFC 3339 timestamps the usage endpoint emits — for example
// "2026-08-11T16:19:59.718872+00:00" or "2026-08-11T16:19:59Z" — into epoch
// milliseconds, so cached windows can be checked for expiry without pulling in
// a date library.
fn iso_to_epoch_ms(value: &str) -> Option<u64> {
    let bytes = value.as_bytes();
    if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| value.get(range)?.parse::<i64>().ok();
    let year = number(0..4)?;
    let month = number(5..7)?;
    let day = number(8..10)?;
    let hour = number(11..13)?;
    let minute = number(14..16)?;
    let second = number(17..19)?;
    let offset_minutes = match value[19..].find(['+', '-']) {
        Some(index) => {
            let offset = &value[19 + index..];
            if offset.len() < 6 {
                return None;
            }
            let sign = if offset.starts_with('-') { -1 } else { 1 };
            sign * (offset[1..3].parse::<i64>().ok()? * 60 + offset[4..6].parse::<i64>().ok()?)
        }
        None => 0,
    };
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second
        - offset_minutes * 60;
    u64::try_from(seconds.checked_mul(1_000)?).ok()
}

fn file_mtime_date(path: &Path) -> Option<String> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let seconds = modified.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    Some(date_from_secs(seconds))
}

fn process_file(
    path: &Path,
    last_computed: &str,
    messages: &mut HashMap<String, i64>,
    sessions: &mut HashMap<String, HashSet<String>>,
    tools: &mut HashMap<String, i64>,
    model_tokens: &mut HashMap<String, i64>,
) {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(_) => return,
    };
    for line in contents.lines() {
        let value: Value = match serde_json::from_str(line.trim()) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let timestamp = match value.get("timestamp").and_then(Value::as_str) {
            Some(timestamp) if timestamp.len() >= 10 => timestamp,
            _ => continue,
        };
        let date = &timestamp[..10];
        if date <= last_computed {
            continue;
        }
        match value.get("type").and_then(Value::as_str).unwrap_or("") {
            "user" => {
                *messages.entry(date.to_string()).or_default() += 1;
                let session_id = value
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .or_else(|| value.get("parentUuid").and_then(Value::as_str));
                if let Some(session_id) = session_id {
                    sessions
                        .entry(date.to_string())
                        .or_default()
                        .insert(session_id.to_string());
                }
            }
            "assistant" => {
                let Some(message) = value.get("message") else {
                    continue;
                };
                let tool_count = message
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|content| {
                        content
                            .iter()
                            .filter(|item| {
                                item.get("type").and_then(Value::as_str) == Some("tool_use")
                            })
                            .count() as i64
                    })
                    .unwrap_or(0);
                *tools.entry(date.to_string()).or_default() += tool_count;
                let output = message
                    .get("usage")
                    .and_then(|usage| usage.get("output_tokens"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                if output > 0 {
                    if let Some(model) = message.get("model").and_then(Value::as_str) {
                        *model_tokens.entry(model.to_string()).or_default() += output;
                    }
                }
            }
            _ => {}
        }
    }
}

fn load_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
}

fn save_json_atomic<T: Serialize>(path: &Path, value: &T) {
    let Some(parent) = path.parent() else {
        return;
    };
    let _ = fs::create_dir_all(parent);
    let temporary = path.with_extension(format!("{}.tmp", now_ms()));
    if let Ok(contents) = serde_json::to_vec(value) {
        if fs::write(&temporary, contents).is_ok() {
            let _ = fs::rename(temporary, path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mirrors a real payload: every legacy per-model field is null and the
    // model-scoped window is only described by the `limits` array.
    fn live_payload() -> Value {
        serde_json::json!({
            "five_hour": {"utilization": 9.0, "resets_at": "2026-08-12T05:00:00.219750+00:00"},
            "seven_day": {"utilization": 19.0, "resets_at": "2026-08-17T15:00:00.219772+00:00"},
            "seven_day_opus": Value::Null,
            "seven_day_sonnet": Value::Null,
            "limits": [
                {"kind": "session", "group": "session", "percent": 9,
                 "resets_at": "2026-08-12T05:00:00.219750+00:00", "scope": Value::Null},
                {"kind": "weekly_all", "group": "weekly", "percent": 19,
                 "resets_at": "2026-08-17T15:00:00.219772+00:00", "scope": Value::Null},
                {"kind": "weekly_scoped", "group": "weekly", "percent": 19,
                 "resets_at": "2026-08-17T15:00:00.219979+00:00",
                 "scope": {"model": {"id": Value::Null, "display_name": "Fable"}}}
            ]
        })
    }

    #[test]
    fn model_scoped_window_becomes_the_third_row() {
        let windows = parse_live_windows(&live_payload());
        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0].id, "five-hour");
        assert_eq!(windows[0].title, "Current session");
        assert_eq!(windows[1].id, "seven-day");
        assert_eq!(windows[2].id, "seven-day-fable");
        assert_eq!(windows[2].title, "Current week (Fable only)");
        assert_eq!(windows[2].utilization, Some(19.0));
        assert_eq!(windows[2].window_minutes, Some(10_080));
    }

    #[test]
    fn window_order_does_not_depend_on_the_arrays_order() {
        let mut payload = live_payload();
        let entries = payload["limits"].as_array_mut().unwrap();
        entries.reverse();
        let windows = parse_live_windows(&payload);
        let ids: Vec<_> = windows.iter().map(|window| window.id.as_str()).collect();
        assert_eq!(ids, ["five-hour", "seven-day", "seven-day-fable"]);
    }

    #[test]
    fn a_scoped_window_without_a_model_name_is_skipped() {
        // Nothing legible to title the row with, so it is dropped rather than
        // rendered as an unlabelled bar.
        let data = serde_json::json!({
            "limits": [
                {"kind": "weekly_all", "percent": 19, "resets_at": Value::Null, "scope": Value::Null},
                {"kind": "weekly_scoped", "percent": 19, "resets_at": Value::Null, "scope": Value::Null},
                {"kind": "something_new", "percent": 5, "resets_at": Value::Null, "scope": Value::Null}
            ]
        });
        let windows = parse_live_windows(&data);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id, "seven-day");
    }

    #[test]
    fn falls_back_to_legacy_fields_when_the_array_is_absent() {
        let data = serde_json::json!({
            "five_hour": {"utilization": 4.0, "resets_at": "2026-08-12T05:00:00+00:00"},
            "seven_day": {"utilization": 18.0, "resets_at": "2026-08-17T14:59:59+00:00"},
            "seven_day_sonnet": Value::Null
        });
        let windows = parse_live_windows(&data);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].id, "five-hour");
        assert_eq!(windows[1].utilization, Some(18.0));
    }

    #[test]
    fn scoped_ids_stay_unique_and_url_safe_across_model_names() {
        assert_eq!(window_slug("Fable"), "fable");
        assert_eq!(window_slug("Claude Opus 4.8"), "claude-opus-4-8");
        assert_ne!(window_slug("Opus"), window_slug("Sonnet"));
    }

    #[test]
    fn rate_limit_and_bare_error_bodies_are_detected() {
        let bare = serde_json::json!({
            "error": {"type": "rate_limit_error", "message": "Rate limited. Please try again later."}
        });
        let discriminated = serde_json::json!({
            "type": "error",
            "error": {"type": "authentication_error", "message": "invalid bearer token"}
        });
        assert_eq!(
            api_error_message(&bare).as_deref(),
            Some("Rate limited. Please try again later.")
        );
        assert_eq!(
            api_error_message(&discriminated).as_deref(),
            Some("invalid bearer token")
        );
        assert_eq!(
            api_error_message(&serde_json::json!({"five_hour": null})),
            None
        );
    }

    fn cached_window(id: &str, resets_at: &str) -> ProviderLimit {
        ProviderLimit {
            id: id.to_string(),
            title: id.to_string(),
            utilization: Some(18.0),
            resets_at: Some(resets_at.to_string()),
            window_minutes: Some(10_080),
        }
    }

    #[test]
    fn expired_cached_windows_are_dropped_instead_of_rendered() {
        let cache = CachedLimits {
            windows: vec![
                cached_window("five-hour", "2026-08-11T16:19:59.718872+00:00"),
                cached_window("seven-day", "2099-01-01T00:00:00+00:00"),
            ],
            saved_at: Some(1_786_450_354_868),
            ..CachedLimits::default()
        };
        let limits = limits_from_cache(&cache, "No Claude Code OAuth credentials found").unwrap();
        assert!(limits.stale);
        assert_eq!(limits.cached_at, Some(1_786_450_354_868));
        assert_eq!(
            limits.stale_reason.as_deref(),
            Some("No Claude Code OAuth credentials found")
        );
        assert_eq!(limits.windows.len(), 1);
        assert_eq!(limits.windows[0].id, "seven-day");
    }

    #[test]
    fn an_entirely_expired_cache_surfaces_the_error() {
        let cache = CachedLimits {
            windows: vec![cached_window("five-hour", "2026-08-11T16:19:59+00:00")],
            ..CachedLimits::default()
        };
        assert_eq!(
            limits_from_cache(&cache, "boom").unwrap_err(),
            "boom".to_string()
        );
    }

    #[test]
    fn a_cache_written_before_scoped_windows_still_renders() {
        // Upgrading must not blank the card on a first offline start.
        let legacy = r#"{"fiveHour":{"utilization":0.0,"resetsAt":"2099-01-01T00:00:00+00:00"},
            "sevenDay":{"utilization":18.0,"resetsAt":"2099-01-01T00:00:00+00:00"},
            "sevenDaySonnet":{"utilization":1.0,"resetsAt":"2026-07-06T14:59:59+00:00"},
            "savedAt":1786450354868}"#;
        let cache: CachedLimits = serde_json::from_str(legacy).expect("legacy cache should parse");
        let limits = limits_from_cache(&cache, "offline").unwrap();
        let ids: Vec<_> = limits.windows.iter().map(|w| w.id.as_str()).collect();
        // The retired Sonnet window is not carried forward.
        assert_eq!(ids, ["five-hour", "seven-day"]);
    }

    #[test]
    fn parses_the_timestamp_forms_the_usage_endpoint_emits() {
        assert_eq!(iso_to_epoch_ms("1970-01-01T00:00:00+00:00"), Some(0),);
        assert_eq!(
            iso_to_epoch_ms("2026-08-11T16:19:59.718872+00:00"),
            Some(1_786_465_199_000),
        );
        // Z and an explicit offset must resolve to the same instant.
        assert_eq!(
            iso_to_epoch_ms("2026-08-11T16:19:59Z"),
            iso_to_epoch_ms("2026-08-12T01:19:59+09:00"),
        );
        assert_eq!(iso_to_epoch_ms("not a timestamp"), None);
    }

    // Opt-in check against the machine's real Claude Code install: proves the
    // credential lookup still finds a token wherever the CLI is storing it, and
    // that the endpoint answers with live windows rather than a cache fallback.
    #[test]
    #[ignore = "requires a locally installed and authenticated Claude Code CLI"]
    fn live_claude_usage_endpoint_responds() {
        assert!(
            read_oauth_token().is_some(),
            "no Claude Code OAuth credentials found in the keychain or config dir"
        );
        let support = std::env::temp_dir().join(format!("quotaglass-live-{}", now_ms()));
        let limits = get_limits(&support).expect("live usage fetch should succeed");
        let _ = fs::remove_dir_all(&support);
        println!(
            "stale={} reason={:?} windows={:?}",
            limits.stale,
            limits.stale_reason,
            limits
                .windows
                .iter()
                .map(|window| (&window.id, window.utilization, &window.resets_at))
                .collect::<Vec<_>>()
        );
        assert!(!limits.stale, "expected live data, got cache fallback");
        assert!(!limits.windows.is_empty(), "expected at least one window");
    }

    #[test]
    fn oauth_blobs_from_either_storage_parse_identically() {
        let blob = r#"{"claudeAiOauth":{"accessToken":"sk-test","refreshToken":"rt","expiresAt":1786551599000,"scopes":["user:inference"]}}"#;
        let token = parse_oauth_blob(blob).expect("blob should parse");
        assert_eq!(token.access_token, "sk-test");
        assert_eq!(token.expires_at, Some(1_786_551_599_000));
        assert!(parse_oauth_blob("{}").is_none());
    }
}
