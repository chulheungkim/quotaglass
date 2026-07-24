use super::{
    ActivityPoint, BreakdownRow, ProviderId, ProviderLimit, ProviderLimits, ProviderStats,
    SummaryMetric,
};
use crate::{app_support_dir, codex_rpc::CodexAppServer, date_from_secs, now_ms, today_date};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CachedRpcValue {
    saved_at: u64,
    value: Value,
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct CodexDaily {
    messages: i64,
    sessions: i64,
    tools: i64,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CachedSessionFile {
    modified_secs: u64,
    length: u64,
    daily: HashMap<String, CodexDaily>,
    model_tokens: HashMap<String, i64>,
}

#[derive(Default, Deserialize, Serialize)]
struct CodexSessionCache {
    files: HashMap<String, CachedSessionFile>,
}

struct LocalStats {
    daily: HashMap<String, CodexDaily>,
    model_tokens: HashMap<String, i64>,
    session_count: i64,
    since: Option<String>,
}

pub fn get_limits(server: &CodexAppServer) -> Result<ProviderLimits, String> {
    let cache_path = app_support_dir().join("codex-rate-limits-cache.json");
    let (value, stale, cached_at) =
        request_with_cache(server, "account/rateLimits/read", &cache_path)?;
    parse_limits(&value, stale, cached_at)
}

pub fn get_stats(server: &CodexAppServer) -> Result<ProviderStats, String> {
    let cache_path = app_support_dir().join("codex-account-usage-cache.json");
    let account = request_with_cache(server, "account/usage/read", &cache_path);
    let local = scan_local_sessions();
    let today = today_date();

    let (account_value, account_stale) = match account {
        Ok((value, stale, _)) => (Some(value), stale),
        Err(_) => (None, false),
    };

    if account_value.is_none() && local.is_err() {
        return Err("Codex account and local session usage are unavailable".to_string());
    }

    let local = local.unwrap_or_else(|_| LocalStats {
        daily: HashMap::new(),
        model_tokens: HashMap::new(),
        session_count: 0,
        since: None,
    });
    let today_local = local.daily.get(&today).cloned().unwrap_or_default();
    let account_today = account_value
        .as_ref()
        .and_then(|value| {
            daily_buckets(value)
                .into_iter()
                .find(|point| point.date == today)
        })
        .map(|point| point.value)
        .unwrap_or(0);

    let daily14 = account_value
        .as_ref()
        .map(daily_buckets)
        .filter(|points| !points.is_empty())
        .unwrap_or_else(|| {
            let mut points: Vec<_> = local
                .daily
                .iter()
                .map(|(date, counts)| ActivityPoint {
                    date: date.clone(),
                    value: counts.messages,
                })
                .collect();
            points.sort_by(|a, b| a.date.cmp(&b.date));
            points
                .into_iter()
                .rev()
                .take(14)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect()
        });
    let activity_label = if account_value.is_some() {
        "14-Day Token Activity"
    } else {
        "14-Day Prompt Activity"
    };

    let mut breakdown: Vec<_> = local
        .model_tokens
        .into_iter()
        .map(|(key, value)| BreakdownRow { key, value })
        .collect();
    breakdown.sort_by(|a, b| b.value.cmp(&a.value));

    let mut footer = Vec::new();
    if let Some(value) = account_value.as_ref() {
        if let Some(lifetime) = value
            .get("summary")
            .and_then(|summary| summary.get("lifetimeTokens"))
            .and_then(Value::as_i64)
        {
            footer.push(format!("{} lifetime tokens", compact_number(lifetime)));
        }
        if let Some(streak) = value
            .get("summary")
            .and_then(|summary| summary.get("currentStreakDays"))
            .and_then(Value::as_i64)
        {
            footer.push(format!("{streak}-day streak"));
        }
    }
    footer.push(format!("{} local sessions", local.session_count));

    let data_scope = match (account_value.is_some(), account_stale) {
        (true, true) => Some("Cached account usage + local sessions".to_string()),
        (true, false) => Some("Account usage + local sessions".to_string()),
        (false, _) => Some("Local sessions only".to_string()),
    };

    Ok(ProviderStats {
        provider: ProviderId::Codex,
        metrics: vec![
            SummaryMetric {
                label: if account_value.is_some() {
                    "Tokens".to_string()
                } else {
                    "Prompts".to_string()
                },
                value: if account_value.is_some() {
                    account_today
                } else {
                    today_local.messages
                },
            },
            SummaryMetric {
                label: "Sessions".to_string(),
                value: today_local.sessions,
            },
            SummaryMetric {
                label: "Tools".to_string(),
                value: today_local.tools,
            },
        ],
        activity_label: activity_label.to_string(),
        daily14,
        breakdown_label: "Output Tokens by Model".to_string(),
        breakdown,
        footer,
        since: local.since,
        data_scope,
        last_updated: today,
    })
}

fn parse_limits(
    value: &Value,
    stale: bool,
    cached_at: Option<u64>,
) -> Result<ProviderLimits, String> {
    let fallback = value
        .get("rateLimits")
        .ok_or_else(|| "Codex returned no rate-limit data".to_string())?;
    let snapshot = value
        .get("rateLimitsByLimitId")
        .and_then(|limits| limits.get("codex"))
        .unwrap_or(fallback);
    let mut windows = Vec::new();

    if let Some(window) = parse_window("primary", snapshot.get("primary")) {
        windows.push(window);
    }
    if let Some(window) = parse_window("secondary", snapshot.get("secondary")) {
        windows.push(window);
    }
    if windows.is_empty() {
        return Err("Codex returned no active rate-limit windows".to_string());
    }

    let credit_balance = snapshot
        .get("credits")
        .and_then(|credits| credits.get("balance"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let plan = snapshot
        .get("planType")
        .and_then(Value::as_str)
        .map(str::to_string);

    Ok(ProviderLimits {
        provider: ProviderId::Codex,
        windows,
        stale,
        cached_at,
        plan,
        credit_balance,
    })
}

fn parse_window(id: &str, window: Option<&Value>) -> Option<ProviderLimit> {
    let window = window?.as_object()?;
    let duration = window.get("windowDurationMins").and_then(Value::as_i64);
    Some(make_window(
        id,
        window_title(id, duration),
        duration,
        window,
    ))
}

fn window_title(id: &str, duration: Option<i64>) -> String {
    match duration {
        Some(300) => "5-hour usage".to_string(),
        Some(10_080) => "Weekly usage".to_string(),
        Some(minutes) if minutes > 0 && minutes % 1_440 == 0 => {
            format!("{}-day usage", minutes / 1_440)
        }
        Some(minutes) if minutes > 0 && minutes % 60 == 0 => {
            format!("{}-hour usage", minutes / 60)
        }
        _ if id == "primary" => "Primary usage".to_string(),
        _ if id == "secondary" => "Secondary usage".to_string(),
        _ => "Usage".to_string(),
    }
}

fn make_window(
    id: &str,
    title: String,
    duration: Option<i64>,
    window: &serde_json::Map<String, Value>,
) -> ProviderLimit {
    ProviderLimit {
        id: id.to_string(),
        title,
        utilization: window.get("usedPercent").and_then(Value::as_f64),
        resets_at: window
            .get("resetsAt")
            .and_then(Value::as_i64)
            .map(timestamp_to_iso),
        window_minutes: duration,
    }
}

fn request_with_cache(
    server: &CodexAppServer,
    method: &str,
    cache_path: &Path,
) -> Result<(Value, bool, Option<u64>), String> {
    match server.request(method) {
        Ok(value) => {
            let cache = CachedRpcValue {
                saved_at: now_ms(),
                value: value.clone(),
            };
            save_json_atomic(cache_path, &cache);
            Ok((value, false, None))
        }
        Err(live_error) => {
            let cache: CachedRpcValue = load_json(cache_path).ok_or(live_error)?;
            Ok((cache.value, true, Some(cache.saved_at)))
        }
    }
}

fn daily_buckets(value: &Value) -> Vec<ActivityPoint> {
    let mut points: Vec<_> = value
        .get("dailyUsageBuckets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|bucket| {
            Some(ActivityPoint {
                date: bucket
                    .get("startDate")?
                    .as_str()?
                    .chars()
                    .take(10)
                    .collect(),
                value: bucket.get("tokens")?.as_i64()?,
            })
        })
        .collect();
    points.sort_by(|a, b| a.date.cmp(&b.date));
    points
        .into_iter()
        .rev()
        .take(14)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn scan_local_sessions() -> Result<LocalStats, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME is unavailable".to_string())?;
    let root = PathBuf::from(home).join(".codex/sessions");
    let cache_path = app_support_dir().join("codex-session-index.json");
    let old_cache: CodexSessionCache = load_json(&cache_path).unwrap_or_default();
    let mut next_cache = CodexSessionCache::default();
    let mut paths = Vec::new();
    collect_jsonl_files(&root, &mut paths);

    for path in paths {
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        let modified_secs = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let key = path.to_string_lossy().to_string();
        let cached = old_cache
            .files
            .get(&key)
            .filter(|entry| entry.modified_secs == modified_secs && entry.length == metadata.len());
        let summary = cached.cloned().unwrap_or_else(|| scan_session_file(&path));
        next_cache.files.insert(key, summary);
    }
    save_json_atomic(&cache_path, &next_cache);

    let mut daily: HashMap<String, CodexDaily> = HashMap::new();
    let mut model_tokens: HashMap<String, i64> = HashMap::new();
    for file in next_cache.files.values() {
        for (date, value) in &file.daily {
            let target = daily.entry(date.clone()).or_default();
            target.messages += value.messages;
            target.sessions += value.sessions;
            target.tools += value.tools;
        }
        for (model, tokens) in &file.model_tokens {
            *model_tokens.entry(model.clone()).or_default() += tokens;
        }
    }
    let session_count = daily.values().map(|value| value.sessions).sum();
    let since = daily.keys().min().cloned();
    Ok(LocalStats {
        daily,
        model_tokens,
        session_count,
        since,
    })
}

fn collect_jsonl_files(path: &Path, output: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, output);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("jsonl") {
            output.push(path);
        }
    }
}

fn scan_session_file(path: &Path) -> CachedSessionFile {
    let metadata = fs::metadata(path).ok();
    let mut summary = CachedSessionFile {
        modified_secs: metadata
            .as_ref()
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or(0),
        length: metadata.as_ref().map(|meta| meta.len()).unwrap_or(0),
        ..Default::default()
    };
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return summary,
    };
    let mut current_model = "unknown".to_string();
    let mut session_counted = false;
    let mut first_date = None;

    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let value: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let date = value
            .get("timestamp")
            .and_then(Value::as_str)
            .filter(|timestamp| timestamp.len() >= 10)
            .map(|timestamp| timestamp[..10].to_string());
        if first_date.is_none() {
            first_date = date.clone();
        }
        let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
        let payload = value.get("payload").unwrap_or(&Value::Null);

        if kind == "session_meta" && !session_counted {
            if let Some(date) = date.as_ref() {
                summary.daily.entry(date.clone()).or_default().sessions += 1;
                session_counted = true;
            }
        } else if kind == "turn_context" {
            if let Some(model) = payload.get("model").and_then(Value::as_str) {
                current_model = model.to_string();
            }
        } else if kind == "response_item" {
            let item_type = payload.get("type").and_then(Value::as_str).unwrap_or("");
            if let Some(date) = date.as_ref() {
                if item_type == "message"
                    && payload.get("role").and_then(Value::as_str) == Some("user")
                {
                    summary.daily.entry(date.clone()).or_default().messages += 1;
                } else if matches!(
                    item_type,
                    "function_call" | "custom_tool_call" | "web_search_call"
                ) {
                    summary.daily.entry(date.clone()).or_default().tools += 1;
                }
            }
        } else if kind == "event_msg"
            && payload.get("type").and_then(Value::as_str) == Some("token_count")
        {
            let output = payload
                .get("info")
                .and_then(|info| info.get("last_token_usage"))
                .and_then(|usage| usage.get("output_tokens"))
                .and_then(Value::as_i64)
                .unwrap_or(0);
            if output > 0 {
                *summary
                    .model_tokens
                    .entry(current_model.clone())
                    .or_default() += output;
            }
        }
    }

    if !session_counted {
        if let Some(date) = first_date {
            summary.daily.entry(date).or_default().sessions += 1;
        }
    }
    summary
}

fn compact_number(value: i64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn timestamp_to_iso(timestamp: i64) -> String {
    let date = date_from_secs(timestamp);
    let seconds = timestamp.rem_euclid(86_400);
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;
    format!("{date}T{hour:02}:{minute:02}:{second:02}Z")
}

fn load_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
}

fn save_json_atomic<T: Serialize>(path: &Path, value: &T) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
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
    use std::io::Write;

    #[test]
    fn parses_dynamic_rate_limit_windows() {
        let value = serde_json::json!({
            "rateLimits": {
                "primary": {"usedPercent": 24, "windowDurationMins": 300, "resetsAt": 1_753_356_000_i64},
                "secondary": {"usedPercent": 51, "windowDurationMins": 10080, "resetsAt": 1_753_356_000_i64},
                "planType": "plus",
                "credits": {"balance": "3.50", "hasCredits": true, "unlimited": false}
            }
        });
        let parsed = parse_limits(&value, false, None).unwrap();
        assert_eq!(parsed.windows[0].title, "5-hour usage");
        assert_eq!(parsed.windows[1].title, "Weekly usage");
        assert_eq!(parsed.plan.as_deref(), Some("plus"));
        assert_eq!(parsed.credit_balance.as_deref(), Some("3.50"));
    }

    #[test]
    fn labels_single_primary_window_by_duration() {
        let value = serde_json::json!({
            "rateLimits": {
                "primary": {
                    "usedPercent": 22,
                    "windowDurationMins": 10080,
                    "resetsAt": 1_785_385_956_i64
                },
                "secondary": null,
                "planType": "prolite"
            }
        });
        let parsed = parse_limits(&value, false, None).unwrap();
        assert_eq!(parsed.windows.len(), 1);
        assert_eq!(parsed.windows[0].title, "Weekly usage");
    }

    #[test]
    fn scans_session_metadata_without_retaining_content() {
        let path = std::env::temp_dir().join(format!(
            "quotaglass-session-{}-{}.jsonl",
            std::process::id(),
            now_ms()
        ));
        let mut file = fs::File::create(&path).unwrap();
        writeln!(
            file,
            "{}",
            serde_json::json!({"timestamp":"2026-07-24T01:00:00Z","type":"session_meta","payload":{}})
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            serde_json::json!({"timestamp":"2026-07-24T01:01:00Z","type":"turn_context","payload":{"model":"gpt-5"}})
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            serde_json::json!({"timestamp":"2026-07-24T01:02:00Z","type":"response_item","payload":{"type":"message","role":"user","content":"private"}})
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            serde_json::json!({"timestamp":"2026-07-24T01:03:00Z","type":"response_item","payload":{"type":"function_call"}})
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            serde_json::json!({"timestamp":"2026-07-24T01:04:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"output_tokens":42}}}})
        )
        .unwrap();
        drop(file);

        let summary = scan_session_file(&path);
        let day = summary.daily.get("2026-07-24").unwrap();
        assert_eq!((day.messages, day.sessions, day.tools), (1, 1, 1));
        assert_eq!(summary.model_tokens.get("gpt-5"), Some(&42));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn timestamp_is_serialized_for_frontend_reset_display() {
        assert_eq!(timestamp_to_iso(0), "1970-01-01T00:00:00Z");
    }
}
