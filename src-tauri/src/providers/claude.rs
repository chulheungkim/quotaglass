use super::{
    ActivityPoint, BreakdownRow, ProviderId, ProviderLimit, ProviderLimits, ProviderStats,
    SummaryMetric,
};
use crate::{date_from_secs, now_ms, today_date};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
    process::Command,
    time::UNIX_EPOCH,
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LimitWindow {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CachedLimits {
    five_hour: Option<LimitWindow>,
    seven_day: Option<LimitWindow>,
    seven_day_sonnet: Option<LimitWindow>,
    saved_at: Option<u64>,
}

pub fn get_limits(support_dir: &Path) -> Result<ProviderLimits, String> {
    let cache_path = support_dir.join("claude-rate-limits-cache.json");
    let legacy_cache_path = support_dir.join("rate-limits-cache.json");
    let cache: CachedLimits = load_json(&cache_path)
        .or_else(|| load_json(&legacy_cache_path))
        .unwrap_or_default();
    let token = match read_oauth_token() {
        Some(token) => token,
        None => return limits_from_cache(&cache, "No OAuth token found in keychain"),
    };
    let output = Command::new("/usr/bin/curl")
        .args([
            "-s",
            "--max-time",
            "8",
            "-H",
            &format!("Authorization: Bearer {token}"),
            "-H",
            "anthropic-beta: oauth-2025-04-20",
            "https://api.anthropic.com/api/oauth/usage",
        ])
        .output()
        .map_err(|error| format!("curl error: {error}"));
    let output = match output {
        Ok(output) => output,
        Err(error) => return limits_from_cache(&cache, &error),
    };
    let body = match String::from_utf8(output.stdout) {
        Ok(body) => body,
        Err(error) => return limits_from_cache(&cache, &format!("UTF-8 error: {error}")),
    };
    let data: Value = match serde_json::from_str(&body) {
        Ok(data) => data,
        Err(error) => return limits_from_cache(&cache, &format!("JSON error: {error}")),
    };
    if data.get("type").and_then(Value::as_str) == Some("error") {
        let message = data
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("Usage unavailable");
        return limits_from_cache(&cache, message);
    }

    let live = parse_live_windows(&data);
    let live_five_hour = live.get("five-hour").cloned();
    let live_seven_day = live.get("seven-day").cloned();
    let live_sonnet = live.get("seven-day-sonnet").cloned();
    let five_hour = live_five_hour.clone().or_else(|| cache.five_hour.clone());
    let seven_day = live_seven_day.clone().or_else(|| cache.seven_day.clone());
    let seven_day_sonnet = live_sonnet
        .clone()
        .or_else(|| cache.seven_day_sonnet.clone());

    if !live.is_empty() {
        save_json_atomic(
            &cache_path,
            &CachedLimits {
                five_hour: five_hour.clone(),
                seven_day: seven_day.clone(),
                seven_day_sonnet: seven_day_sonnet.clone(),
                saved_at: Some(now_ms()),
            },
        );
    }
    let stale = (live_five_hour.is_none() && five_hour.is_some())
        || (live_seven_day.is_none() && seven_day.is_some())
        || (live_sonnet.is_none() && seven_day_sonnet.is_some());
    Ok(normalize_limits(
        five_hour,
        seven_day,
        seven_day_sonnet,
        stale,
        if stale { cache.saved_at } else { None },
    ))
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
        plan: None,
        credit_balance: None,
    }
}

fn limits_from_cache(cache: &CachedLimits, error: &str) -> Result<ProviderLimits, String> {
    if cache.five_hour.is_none() && cache.seven_day.is_none() && cache.seven_day_sonnet.is_none() {
        Err(error.to_string())
    } else {
        Ok(normalize_limits(
            cache.five_hour.clone(),
            cache.seven_day.clone(),
            cache.seven_day_sonnet.clone(),
            true,
            cache.saved_at,
        ))
    }
}

fn read_oauth_token() -> Option<String> {
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
    let credentials: Value =
        serde_json::from_str(String::from_utf8(output.stdout).ok()?.trim()).ok()?;
    credentials
        .get("claudeAiOauth")?
        .get("accessToken")?
        .as_str()
        .map(str::to_string)
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
        );
        assert_eq!(normalized.windows.len(), 3);
        assert_eq!(normalized.windows[0].id, "five-hour");
        assert_eq!(normalized.windows[2].title, "Current week (Sonnet only)");
    }
}
