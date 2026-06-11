use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::menu::{CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::Manager;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

// ── drag state (shared between window events and commands) ───────────────────

struct DragState {
    // True while the user is holding & dragging the window. Set in begin_drag,
    // cleared the instant the real left-mouse-button release is detected.
    is_dragging: AtomicBool,
    // True while the corner-snap animation is running, so background refreshes
    // don't reposition the window mid-animation.
    is_animating: AtomicBool,
    // Guards against spawning more than one mouse-up watcher at a time.
    drag_watching: AtomicBool,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── Rate-limit usage structs (for /api/oauth/usage) ──────────────────────────

#[derive(Serialize, Deserialize, Clone)]
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
    // True when one or more windows came from the on-disk cache because the live
    // API returned null/error — so the widget can always show last-known usage.
    stale: bool,
    // Epoch millis of the cached data when stale; None when fully live.
    cached_at: Option<u64>,
}

// Last-known-good windows, persisted so usage is shown even when the live API
// intermittently returns null windows (observed even with a valid token) or the
// token has expired between Claude Code sessions.
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct CachedLimits {
    five_hour: Option<LimitWindow>,
    seven_day: Option<LimitWindow>,
    seven_day_sonnet: Option<LimitWindow>,
    saved_at: Option<u64>,
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
            let out = usage
                .get("outputTokens")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            model_tokens.insert(model.clone(), out);
        }
    }
    for (model, out) in &dmt {
        *model_tokens.entry(model.clone()).or_insert(0) += *out;
    }

    let delta_sessions: i64 = ds.values().map(|s| s.len() as i64).sum();
    let delta_messages: i64 = dm.values().sum();
    let all_time = AllTime {
        sessions: cache
            .get("totalSessions")
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            + delta_sessions,
        messages: cache
            .get("totalMessages")
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            + delta_messages,
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

fn rate_cache_path() -> std::path::PathBuf {
    app_support_dir().join("rate-limits-cache.json")
}

fn load_rate_cache() -> CachedLimits {
    fs::read_to_string(rate_cache_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_rate_cache(c: &CachedLimits) {
    let _ = fs::create_dir_all(app_support_dir());
    if let Ok(s) = serde_json::to_string(c) {
        let _ = fs::write(rate_cache_path(), s);
    }
}

// Fall back to the last-known-good cache when the live fetch is unusable. Only
// errors if we've never cached anything (so the UI shows a message, not blank).
fn from_cache_or_err(cache: &CachedLimits, err: &str) -> Result<RateLimits, String> {
    if cache.five_hour.is_some() || cache.seven_day.is_some() || cache.seven_day_sonnet.is_some() {
        Ok(RateLimits {
            five_hour: cache.five_hour.clone(),
            seven_day: cache.seven_day.clone(),
            seven_day_sonnet: cache.seven_day_sonnet.clone(),
            stale: true,
            cached_at: cache.saved_at,
        })
    } else {
        Err(err.to_string())
    }
}

#[tauri::command]
fn get_rate_limits() -> Result<RateLimits, String> {
    let cache = load_rate_cache();

    let token = match read_oauth_token() {
        Some(t) => t,
        None => return from_cache_or_err(&cache, "No OAuth token found in keychain"),
    };

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
        .output();
    let out = match out {
        Ok(o) => o,
        Err(e) => return from_cache_or_err(&cache, &format!("curl error: {e}")),
    };

    let body = match String::from_utf8(out.stdout) {
        Ok(b) => b,
        Err(e) => return from_cache_or_err(&cache, &format!("UTF-8 error: {e}")),
    };
    let data: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return from_cache_or_err(&cache, &format!("JSON error: {e}")),
    };

    // An expired/invalid token returns {"type":"error","error":{...}} with no
    // window keys — fall back to cached usage rather than blanking the panel.
    if data.get("type").and_then(|v| v.as_str()) == Some("error") {
        let msg = data
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("Usage unavailable");
        return from_cache_or_err(&cache, msg);
    }

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

    let live_5 = parse_window("five_hour");
    let live_7 = parse_window("seven_day");
    let live_7s = parse_window("seven_day_sonnet");

    // The API intermittently returns null windows even with a valid token. Keep
    // the freshest known value per window so the widget always shows usage.
    let five_hour = live_5.clone().or_else(|| cache.five_hour.clone());
    let seven_day = live_7.clone().or_else(|| cache.seven_day.clone());
    let seven_day_sonnet = live_7s.clone().or_else(|| cache.seven_day_sonnet.clone());

    if live_5.is_some() || live_7.is_some() || live_7s.is_some() {
        save_rate_cache(&CachedLimits {
            five_hour: five_hour.clone(),
            seven_day: seven_day.clone(),
            seven_day_sonnet: seven_day_sonnet.clone(),
            saved_at: Some(now_ms()),
        });
    }

    // Stale if any returned window had to come from the cache this round.
    let stale = (live_5.is_none() && five_hour.is_some())
        || (live_7.is_none() && seven_day.is_some())
        || (live_7s.is_none() && seven_day_sonnet.is_some());

    Ok(RateLimits {
        five_hour,
        seven_day,
        seven_day_sonnet,
        stale,
        cached_at: if stale { cache.saved_at } else { None },
    })
}

// ── corner-placement system ───────────────────────────────────────────────────

fn app_support_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join("Library/Application Support/com.chulheong.claudeusage")
}

fn load_corner() -> String {
    fs::read_to_string(app_support_dir().join(".corner"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| ["tl", "tr", "bl", "br"].contains(&s.as_str()))
        .unwrap_or_else(|| "tr".to_string())
}

fn save_corner(corner: &str) {
    let dir = app_support_dir();
    let _ = fs::create_dir_all(&dir);
    let _ = fs::write(dir.join(".corner"), corner.as_bytes());
}

const CARD_WIDTH: f64 = 300.0;
const MARGIN_SIDE: f64 = 10.0;
const MARGIN_TOP: f64 = 44.0;
const MARGIN_BOTTOM: f64 = 10.0;

// Top-left target position for a given corner and *physical* window size.
// Taking the size explicitly (rather than reading outer_size()) lets callers
// position the window for a height it hasn't visually applied yet — essential
// for anchoring a bottom corner while the height animates, so the bottom edge
// stays put and the window grows upward.
fn corner_xy(monitor: &tauri::Monitor, corner: &str, phys_w: i32, phys_h: i32) -> (i32, i32) {
    let mp = monitor.position();
    let ms = monitor.size();
    let scale = monitor.scale_factor();
    let mh = (MARGIN_SIDE * scale) as i32;
    let mt = (MARGIN_TOP * scale) as i32;
    let mb = (MARGIN_BOTTOM * scale) as i32;
    let x = if corner.ends_with('l') {
        mp.x + mh
    } else {
        (mp.x + ms.width as i32 - phys_w - mh).max(mp.x)
    };
    let y = if corner.starts_with('t') {
        mp.y + mt
    } else {
        (mp.y + ms.height as i32 - phys_h - mb).max(mp.y)
    };
    (x, y)
}

fn position_at_corner(win: &tauri::WebviewWindow, corner: &str) {
    if let (Ok(Some(monitor)), Ok(wsize)) = (win.current_monitor(), win.outer_size()) {
        let (x, y) = corner_xy(&monitor, corner, wsize.width as i32, wsize.height as i32);
        let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
    }
}

// Returns (corner, target_x, target_y) based on where the window center is.
fn snap_target(win: &tauri::WebviewWindow) -> Option<(String, i32, i32)> {
    let (pos, wsize, monitor) = match (
        win.outer_position(),
        win.outer_size(),
        win.current_monitor(),
    ) {
        (Ok(p), Ok(ws), Ok(Some(m))) => (p, ws, m),
        _ => return None,
    };
    let mp = monitor.position();
    let ms = monitor.size();
    let cx = mp.x + ms.width as i32 / 2;
    let cy = mp.y + ms.height as i32 / 2;
    let wcx = pos.x + wsize.width as i32 / 2;
    let wcy = pos.y + wsize.height as i32 / 2;
    let corner = format!(
        "{}{}",
        if wcy < cy { 't' } else { 'b' },
        if wcx < cx { 'l' } else { 'r' }
    );
    let (tx, ty) = corner_xy(&monitor, &corner, wsize.width as i32, wsize.height as i32);
    Some((corner, tx, ty))
}

// Glide to the nearest corner with a time-based ease-out-quint animation.
// Driving the position off elapsed time (not a fixed step count) keeps the
// motion frame-accurate and buttery even if a frame is delayed; the quint curve
// gives a gentle, soft landing. Aborts immediately if the user grabs the window
// again, so a new drag never fights a still-running snap.
fn snap_corner_impl(app: &tauri::AppHandle, win: &tauri::WebviewWindow) {
    let (corner, tx, ty) = match snap_target(win) {
        Some(v) => v,
        None => return,
    };
    let start = match win.outer_position() {
        Ok(p) => p,
        Err(_) => return,
    };
    save_corner(&corner);
    if start.x == tx && start.y == ty {
        return;
    }
    let win = win.clone();
    let app = app.clone();
    let sx = start.x as f32;
    let sy = start.y as f32;
    let dx = (tx - start.x) as f32;
    let dy = (ty - start.y) as f32;
    std::thread::spawn(move || {
        let st = app.state::<DragState>();
        st.is_animating.store(true, Ordering::Relaxed);
        const DURATION: f32 = 0.42; // seconds
        let begin = Instant::now();
        loop {
            if st.is_dragging.load(Ordering::Relaxed) {
                break; // user grabbed it again — hand control back to the drag
            }
            let t = (begin.elapsed().as_secs_f32() / DURATION).min(1.0);
            let e = 1.0 - (1.0 - t).powi(5); // ease-out quint — soft landing
            let x = (sx + dx * e).round() as i32;
            let y = (sy + dy * e).round() as i32;
            let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
            if t >= 1.0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(8)); // ~120 fps
        }
        if !st.is_dragging.load(Ordering::Relaxed) {
            let _ = win.set_position(tauri::PhysicalPosition::new(tx, ty));
        }
        st.is_animating.store(false, Ordering::Relaxed);
    });
}

// True while the left mouse button is physically held down.
#[cfg(target_os = "macos")]
fn left_mouse_down() -> bool {
    (objc2_app_kit::NSEvent::pressedMouseButtons() & 1) != 0
}
#[cfg(not(target_os = "macos"))]
fn left_mouse_down() -> bool {
    false
}

// Begin a window drag: marks the drag active and starts a watcher that fires the
// corner snap the *instant* the real left-mouse-button release is detected. This
// replaces Moved-gap debouncing, so slow and fast drags behave identically and
// there is zero delay between releasing the mouse and the snap starting.
#[tauri::command]
fn begin_drag(window: tauri::WebviewWindow, state: tauri::State<DragState>) {
    state.is_dragging.store(true, Ordering::Relaxed);
    if state.drag_watching.swap(true, Ordering::Relaxed) {
        return; // a watcher is already running
    }
    let app = window.app_handle().clone();
    let win = window.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(8));
            if !left_mouse_down() {
                break;
            }
        }
        let st = app.state::<DragState>();
        st.is_dragging.store(false, Ordering::Relaxed);
        st.drag_watching.store(false, Ordering::Relaxed);
        snap_corner_impl(&app, &win);
    });
}

// Resize and reanchor. Position is computed from the *target* height so a
// bottom-anchored window grows upward correctly on every frame. Skipped while
// dragging or snapping so background refreshes never fight those motions.
#[tauri::command]
fn set_height(window: tauri::WebviewWindow, state: tauri::State<DragState>, h: u32) {
    if h == 0 {
        return;
    }
    let _ = window.set_size(tauri::LogicalSize::new(CARD_WIDTH, h as f64));
    if state.is_dragging.load(Ordering::Relaxed) || state.is_animating.load(Ordering::Relaxed) {
        return;
    }
    if let Ok(Some(monitor)) = window.current_monitor() {
        let scale = monitor.scale_factor();
        let phys_w = (CARD_WIDTH * scale).round() as i32;
        let phys_h = (h as f64 * scale).round() as i32;
        let (x, y) = corner_xy(&monitor, &load_corner(), phys_w, phys_h);
        let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    }
}

#[tauri::command]
fn reanchor(window: tauri::WebviewWindow, state: tauri::State<DragState>) {
    if !state.is_dragging.load(Ordering::Relaxed) && !state.is_animating.load(Ordering::Relaxed) {
        position_at_corner(&window, &load_corner());
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

    #[cfg(not(debug_assertions))]
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
    #[cfg(not(debug_assertions))]
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
        .manage(DragState {
            is_dragging: AtomicBool::new(false),
            is_animating: AtomicBool::new(false),
            drag_watching: AtomicBool::new(false),
        })
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() == ShortcutState::Pressed
                        && shortcut
                            == &Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyU)
                    {
                        toggle_window(app);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            get_usage_stats,
            get_rate_limits,
            begin_drag,
            set_height,
            reanchor,
        ])
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

            // Global show/hide hotkey: Cmd+Shift+U (U = Usage). Chosen to avoid
            // common app/system bindings. Ignore errors so a conflicting binding
            // never blocks startup.
            let _ = app.global_shortcut().register(Shortcut::new(
                Some(Modifiers::SUPER | Modifiers::SHIFT),
                Code::KeyU,
            ));

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
                // Drag start is signalled from the frontend (begin_drag), and the
                // snap fires on the real mouse-up — so no Moved-event listener or
                // debounce is needed here. Just place the window initially.
                position_at_corner(&win, &load_corner());
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
