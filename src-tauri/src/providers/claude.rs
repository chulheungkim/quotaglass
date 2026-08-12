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

impl LimitWindow {
    // A cached window whose reset time has already passed describes a window
    // that no longer exists, so it must never be rendered as current usage.
    fn is_expired(&self, now: u64) -> bool {
        self.resets_at
            .as_deref()
            .and_then(iso_to_epoch_ms)
            .is_some_and(|resets_at| resets_at <= now)
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CachedLimits {
    five_hour: Option<LimitWindow>,
    seven_day: Option<LimitWindow>,
    seven_day_sonnet: Option<LimitWindow>,
    saved_at: Option<u64>,
    retry_after: Option<u64>,
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
            five_hour: live.get("five-hour").cloned(),
            seven_day: live.get("seven-day").cloned(),
            seven_day_sonnet: live.get("seven-day-sonnet").cloned(),
            saved_at: Some(now_ms()),
            retry_after: None,
        },
    );
    Ok(normalize_limits(
        live.get("five-hour").cloned(),
        live.get("seven-day").cloned(),
        live.get("seven-day-sonnet").cloned(),
        false,
        None,
        None,
    ))
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

fn parse_live_windows(data: &Value) -> HashMap<String, LimitWindow> {
    [
        ("five-hour", "five_hour"),
        ("seven-day", "seven_day"),
        ("seven-day-sonnet", "seven_day_sonnet"),
    ]
    .into_iter()
    .filter_map(|(id, key)| {
        let value = data.get(key)?;
        if value.is_null() {
            return None;
        }
        Some((
            id.to_string(),
            LimitWindow {
                utilization: value.get("utilization").and_then(Value::as_f64),
                resets_at: value
                    .get("resets_at")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            },
        ))
    })
    .collect()
}

fn normalize_limits(
    five_hour: Option<LimitWindow>,
    seven_day: Option<LimitWindow>,
    seven_day_sonnet: Option<LimitWindow>,
    stale: bool,
    cached_at: Option<u64>,
    stale_reason: Option<String>,
) -> ProviderLimits {
    let mut windows = Vec::new();
    let mut push = |id: &str, title: &str, minutes: i64, window: Option<LimitWindow>| {
        if let Some(window) = window {
            windows.push(ProviderLimit {
                id: id.to_string(),
                title: title.to_string(),
                utilization: window.utilization,
                resets_at: window.resets_at,
                window_minutes: Some(minutes),
            });
        }
    };
    push("five-hour", "Current session", 300, five_hour);
    push("seven-day", "Current week (all models)", 10_080, seven_day);
    push(
        "seven-day-sonnet",
        "Current week (Sonnet only)",
        10_080,
        seven_day_sonnet,
    );
    ProviderLimits {
        provider: ProviderId::Claude,
        windows,
        stale,
        cached_at,
        stale_reason,
        plan: None,
        credit_balance: None,
    }
}

fn limits_from_cache(cache: &CachedLimits, error: &str) -> Result<ProviderLimits, String> {
    // Expired windows are dropped rather than rendered: a five-hour window whose
    // reset time has passed would otherwise show a long-dead utilization under a
    // reset time in the past, which reads as live data.
    let now = now_ms();
    let keep = |window: &Option<LimitWindow>| {
        window
            .clone()
            .filter(|candidate| !candidate.is_expired(now))
    };
    let five_hour = keep(&cache.five_hour);
    let seven_day = keep(&cache.seven_day);
    let seven_day_sonnet = keep(&cache.seven_day_sonnet);
    if five_hour.is_none() && seven_day.is_none() && seven_day_sonnet.is_none() {
        Err(error.to_string())
    } else {
        Ok(normalize_limits(
            five_hour,
            seven_day,
            seven_day_sonnet,
            true,
            cache.saved_at,
            Some(error.to_string()),
        ))
    }
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

    #[test]
    fn normalizes_all_three_claude_windows() {
        let data = serde_json::json!({
            "five_hour": {"utilization": 10.0, "resets_at": "2026-07-24T10:00:00Z"},
            "seven_day": {"utilization": 20.0, "resets_at": "2026-07-28T10:00:00Z"},
            "seven_day_sonnet": {"utilization": 30.0, "resets_at": "2026-07-28T10:00:00Z"}
        });
        let live = parse_live_windows(&data);
        let normalized = normalize_limits(
            live.get("five-hour").cloned(),
            live.get("seven-day").cloned(),
            live.get("seven-day-sonnet").cloned(),
            false,
            None,
            None,
        );
        assert_eq!(normalized.windows.len(), 3);
        assert_eq!(normalized.windows[0].id, "five-hour");
        assert_eq!(normalized.windows[2].title, "Current week (Sonnet only)");
    }

    #[test]
    fn a_missing_live_window_does_not_mark_the_card_stale() {
        // The API omits a window that has no usage. That is live data, not a
        // failed read, so the remaining windows must still render as fresh.
        let data = serde_json::json!({
            "five_hour": {"utilization": 4.0, "resets_at": "2026-08-12T05:00:00+00:00"},
            "seven_day": {"utilization": 18.0, "resets_at": "2026-08-17T14:59:59+00:00"},
            "seven_day_sonnet": Value::Null
        });
        let live = parse_live_windows(&data);
        assert_eq!(live.len(), 2);
        let normalized = normalize_limits(
            live.get("five-hour").cloned(),
            live.get("seven-day").cloned(),
            live.get("seven-day-sonnet").cloned(),
            false,
            None,
            None,
        );
        assert!(!normalized.stale);
        assert_eq!(normalized.windows.len(), 2);
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

    #[test]
    fn expired_cached_windows_are_dropped_instead_of_rendered() {
        let cache = CachedLimits {
            five_hour: Some(LimitWindow {
                utilization: Some(0.0),
                resets_at: Some("2026-08-11T16:19:59.718872+00:00".to_string()),
            }),
            seven_day: Some(LimitWindow {
                utilization: Some(18.0),
                resets_at: Some("2099-01-01T00:00:00+00:00".to_string()),
            }),
            seven_day_sonnet: None,
            saved_at: Some(1_786_450_354_868),
            retry_after: None,
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
            five_hour: Some(LimitWindow {
                utilization: Some(0.0),
                resets_at: Some("2026-08-11T16:19:59+00:00".to_string()),
            }),
            ..CachedLimits::default()
        };
        assert_eq!(
            limits_from_cache(&cache, "boom").unwrap_err(),
            "boom".to_string()
        );
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
