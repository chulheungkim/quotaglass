use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;
use tauri::menu::{CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::Manager;

// ── Rate-limit usage structs (for /api/oauth/usage) ──────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitWindow {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimits {
    five_hour: Option<LimitWindow>,
    seven_day: Option<LimitWindow>,
    seven_day_sonnet: Option<LimitWindow>,
}

// ── Historical usage structs ──────────────────────────────────────────────────

#[derive(Serialize)]
pub struct DayStat {
    date: String,
    messages: i64,
    sessions: i64,
    tools: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Today {
    date: String,
    messages: i64,
    sessions: i64,
    tool_calls: i64,
}

#[derive(Serialize)]
pub struct AllTime {
    sessions: i64,
    messages: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageStats {
    today: Today,
    daily14: Vec<DayStat>,
    model_tokens: HashMap<String, i64>,
    all_time: AllTime,
    since: Option<String>,
    last_updated: String,
}

// Convert a count of days since the Unix epoch into (year, month, day).
// Howard Hinnant's civil-from-days algorithm; pure std, no chrono dependency.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn date_from_secs(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

fn today_date() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    date_from_secs(secs)
}

fn file_mtime_date(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let secs = modified.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    Some(date_from_secs(secs))
}

fn process_file(
    path: &Path,
    last_computed: &str,
    dm: &mut HashMap<String, i64>,
    ds: &mut HashMap<String, HashSet<String>>,
    dt: &mut HashMap<String, i64>,
    dmt: &mut HashMap<String, i64>,
) {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let obj: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let ts = match obj.get("timestamp").and_then(|v| v.as_str()) {
            Some(t) if t.len() >= 10 => t,
            _ => continue,
        };
        let date = &ts[0..10];
        if date <= last_computed {
            continue;
        }
        let outer_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if outer_type == "user" {
            *dm.entry(date.to_string()).or_insert(0) += 1;
            let sid = obj
                .get("sessionId")
                .and_then(|v| v.as_str())
                .or_else(|| obj.get("parentUuid").and_then(|v| v.as_str()));
            if let Some(sid) = sid {
                ds.entry(date.to_string())
                    .or_default()
                    .insert(sid.to_string());
            }
        } else if outer_type == "assistant" {
            if let Some(msg) = obj.get("message") {
                if let Some(content) = msg.get("content").and_then(|v| v.as_array()) {
                    let tools = content
                        .iter()
                        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
                        .count() as i64;
                    if tools > 0 {
                        *dt.entry(date.to_string()).or_insert(0) += tools;
                    }
                }
                let model = msg.get("model").and_then(|v| v.as_str());
                let out = msg
                    .get("usage")
                    .and_then(|u| u.get("output_tokens"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                if let (Some(model), true) = (model, out != 0) {
                    *dmt.entry(model.to_string()).or_insert(0) += out;
                }
            }
        }
    }
}

#[tauri::command]
fn get_usage_stats() -> Result<UsageStats, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
    let cache_path = format!("{home}/.claude/stats-cache.json");
    let projects_dir = format!("{home}/.claude/projects");

    let cache: Value = fs::read_to_string(&cache_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);

    let last_computed = cache
        .get("lastComputedDate")
        .and_then(|v| v.as_str())
        .unwrap_or("2000-01-01")
        .to_string();

    let mut dm: HashMap<String, i64> = HashMap::new();
    let mut ds: HashMap<String, HashSet<String>> = HashMap::new();
    let mut dt: HashMap<String, i64> = HashMap::new();
    let mut dmt: HashMap<String, i64> = HashMap::new();

    if let Ok(entries) = fs::read_dir(&projects_dir) {
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            if let Ok(files) = fs::read_dir(&dir) {
                for f in files.flatten() {
                    let p = f.path();
                    if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                        continue;
                    }
                    if let Some(fdate) = file_mtime_date(&p) {
                        if fdate.as_str() <= last_computed.as_str() {
                            continue;
                        }
                    }
                    process_file(&p, &last_computed, &mut dm, &mut ds, &mut dt, &mut dmt);
                }
            }
        }
    }

    // Merge cached daily activity with the freshly scanned delta.
    let mut daily: HashMap<String, (i64, i64, i64)> = HashMap::new();
    if let Some(arr) = cache.get("dailyActivity").and_then(|v| v.as_array()) {
        for d in arr {
            let date = d.get("date").and_then(|v| v.as_str()).unwrap_or("");
            if date.is_empty() {
                continue;
            }
            let m = d.get("messageCount").and_then(|v| v.as_i64()).unwrap_or(0);
            let s = d.get("sessionCount").and_then(|v| v.as_i64()).unwrap_or(0);
            let t = d.get("toolCallCount").and_then(|v| v.as_i64()).unwrap_or(0);
            daily.insert(date.to_string(), (m, s, t));
        }
    }
    let mut delta_dates: HashSet<String> = HashSet::new();
    delta_dates.extend(dm.keys().cloned());
    delta_dates.extend(ds.keys().cloned());
    delta_dates.extend(dt.keys().cloned());
    for date in &delta_dates {
        let e = daily.entry(date.clone()).or_insert((0, 0, 0));
        e.0 += dm.get(date).copied().unwrap_or(0);
        e.1 += ds.get(date).map(|s| s.len() as i64).unwrap_or(0);
        e.2 += dt.get(date).copied().unwrap_or(0);
    }

    let mut dates: Vec<String> = daily.keys().cloned().collect();
    dates.sort();
    let daily14: Vec<DayStat> = dates
        .iter()
        .rev()
        .take(14)
        .rev()
        .map(|d| {
            let v = daily[d];
            DayStat {
                date: d.clone(),
                messages: v.0,
                sessions: v.1,
                tools: v.2,
            }
        })
        .collect();

    let today_str = today_date();
    let tv = daily.get(&today_str).copied().unwrap_or((0, 0, 0));
    let today = Today {
        date: today_str.clone(),
        messages: tv.0,
        sessions: tv.1,
        tool_calls: tv.2,
    };

    let mut model_tokens: HashMap<String, i64> = HashMap::new();
    if let Some(mu) = cache.get("modelUsage").and_then(|v| v.as_object()) {
        for (model, usage) in mu {
            let out = usage.get("outputTokens").and_then(|v| v.as_i64()).unwrap_or(0);
            model_tokens.insert(model.clone(), out);
        }
    }
    for (model, out) in &dmt {
        *model_tokens.entry(model.clone()).or_insert(0) += *out;
    }

    let delta_sessions: i64 = ds.values().map(|s| s.len() as i64).sum();
    let delta_messages: i64 = dm.values().sum();
    let all_time = AllTime {
        sessions: cache.get("totalSessions").and_then(|v| v.as_i64()).unwrap_or(0) + delta_sessions,
        messages: cache.get("totalMessages").and_then(|v| v.as_i64()).unwrap_or(0) + delta_messages,
    };

    let since = cache
        .get("firstSessionDate")
        .and_then(|v| v.as_str())
        .map(|s| s.chars().take(10).collect::<String>());

    Ok(UsageStats {
        today,
        daily14,
        model_tokens,
        all_time,
        since,
        last_updated: today_str,
    })
}

fn read_oauth_token() -> Option<String> {
    let username = std::env::var("USER").unwrap_or_else(|_| "claude-code-user".to_string());
    let out = Command::new("/usr/bin/security")
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
    if !out.status.success() {
        return None;
    }
    let creds_json = String::from_utf8(out.stdout).ok()?;
    let v: Value = serde_json::from_str(creds_json.trim()).ok()?;
    v.get("claudeAiOauth")?
        .get("accessToken")?
        .as_str()
        .map(|s| s.to_string())
}

#[tauri::command]
fn get_rate_limits() -> Result<RateLimits, String> {
    let token =
        read_oauth_token().ok_or_else(|| "No OAuth token found in keychain".to_string())?;

    let out = Command::new("/usr/bin/curl")
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
        .map_err(|e| format!("curl error: {e}"))?;

    let body = String::from_utf8(out.stdout).map_err(|e| format!("UTF-8 error: {e}"))?;
    let data: Value = serde_json::from_str(&body).map_err(|e| {
        let preview = &body[..body.len().min(120)];
        format!("JSON error: {e}: {preview}")
    })?;

    let parse_window = |key: &str| -> Option<LimitWindow> {
        let w = data.get(key)?;
        if w.is_null() {
            return None;
        }
        Some(LimitWindow {
            utilization: w.get("utilization").and_then(|v| v.as_f64()),
            resets_at: w
                .get("resets_at")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        })
    };

    Ok(RateLimits {
        five_hour: parse_window("five_hour"),
        seven_day: parse_window("seven_day"),
        seven_day_sonnet: parse_window("seven_day_sonnet"),
    })
}

fn position_top_right(win: &tauri::WebviewWindow) {
    if let (Ok(Some(monitor)), Ok(wsize)) = (win.current_monitor(), win.outer_size()) {
        let m = monitor.size();
        let scale = monitor.scale_factor();
        let margin = (20.0 * scale) as i32;
        let top = (44.0 * scale) as i32;
        let x = (m.width as i32 - wsize.width as i32 - margin).max(0);
        let _ = win.set_position(tauri::PhysicalPosition::new(x, top));
    }
}

// Native launch-at-login via SMAppService — the app registers itself as a
// login item, which is the only mechanism that works for an LSUIElement agent
// app and keeps the System Settings toggle authoritative.
#[cfg(target_os = "macos")]
mod login_item {
    use objc2_service_management::{SMAppService, SMAppServiceStatus};

    pub fn is_enabled() -> bool {
        unsafe { SMAppService::mainAppService().status() == SMAppServiceStatus::Enabled }
    }

    pub fn set(enabled: bool) -> bool {
        let service = unsafe { SMAppService::mainAppService() };
        let result = if enabled {
            unsafe { service.registerAndReturnError() }
        } else {
            unsafe { service.unregisterAndReturnError() }
        };
        result.is_ok()
    }

    pub fn ensure_registered() {
        // Auto-enable login-at-launch only on the very first run. A marker file
        // records that we've done it, so if the user later turns it off (tray or
        // System Settings) we never silently re-enable it.
        let Ok(home) = std::env::var("HOME") else {
            return;
        };
        let marker = std::path::PathBuf::from(home)
            .join("Library/Application Support/com.chulheong.claudeusage/.login-registered");
        if marker.exists() {
            return;
        }
        let service = unsafe { SMAppService::mainAppService() };
        if unsafe { service.status() } == SMAppServiceStatus::NotRegistered {
            let _ = unsafe { service.registerAndReturnError() };
        }
        if let Some(parent) = marker.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&marker, b"1");
    }
}

#[cfg(not(target_os = "macos"))]
mod login_item {
    pub fn is_enabled() -> bool {
        false
    }
    pub fn set(_enabled: bool) -> bool {
        false
    }
    pub fn ensure_registered() {}
}

fn toggle_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        } else {
            let _ = win.show();
            let _ = win.set_focus();
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![get_usage_stats, get_rate_limits])
        .on_window_event(|window, event| {
            // Closing hides the widget instead of quitting; it lives in the menu bar.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(|app| {
            // Run as a menu-bar agent: hidden from Dock and Cmd+Tab.
            #[cfg(target_os = "macos")]
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Register as a login item on first run of the installed (release) build.
            #[cfg(not(debug_assertions))]
            login_item::ensure_registered();

            // Menu-bar tray icon + menu.
            let toggle = MenuItemBuilder::with_id("toggle", "Show / Hide").build(app)?;
            let login = CheckMenuItemBuilder::with_id("login", "Start at Login")
                .checked(login_item::is_enabled())
                .build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
            let menu = MenuBuilder::new(app)
                .item(&toggle)
                .item(&login)
                .separator()
                .item(&quit)
                .build()?;

            let login_item_handle = login.clone();
            let mut tray = TrayIconBuilder::with_id("main-tray")
                .menu(&menu)
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "quit" => app.exit(0),
                    "toggle" => toggle_window(app),
                    "login" => {
                        let target = !login_item::is_enabled();
                        login_item::set(target);
                        let _ = login_item_handle.set_checked(login_item::is_enabled());
                    }
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon().cloned() {
                tray = tray.icon(icon);
            }
            let _tray = tray.build(app)?;

            if let Some(win) = app.get_webview_window("main") {
                position_top_right(&win);
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
