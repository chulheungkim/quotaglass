use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use notify::{RecursiveMode, Watcher};

use tauri::menu::{CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use codex_rpc::CodexAppServer;
use providers::{ProviderId, ProviderLimits, ProviderStats};

// ── tray icon ────────────────────────────────────────────────────────────────

mod codex_rpc;
mod providers;
mod tray_icon;

fn make_tray_icon() -> tauri::image::Image<'static> {
    tauri::image::Image::new(
        tray_icon::TRAY_ICON_RGBA,
        tray_icon::TRAY_ICON_SIZE,
        tray_icon::TRAY_ICON_SIZE,
    )
}

// ── drag state (shared between window events and commands) ───────────────────

// Keeps the FSEvents watcher alive for the app's lifetime.
#[allow(dead_code)]
struct FsWatcher(std::sync::Mutex<notify::RecommendedWatcher>);

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

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// Convert a count of days since the Unix epoch into (year, month, day).
// Howard Hinnant's civil-from-days algorithm; pure std, no chrono dependency.
pub(crate) fn civil_from_days(z: i64) -> (i64, i64, i64) {
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

// Inverse of civil_from_days: (year, month, day) back to days since the epoch.
pub(crate) fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub(crate) fn date_from_secs(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

pub(crate) fn today_date() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    date_from_secs(secs)
}

#[tauri::command]
async fn get_provider_limits(
    provider: ProviderId,
    codex: tauri::State<'_, CodexAppServer>,
) -> Result<ProviderLimits, String> {
    let support = app_support_dir();
    let codex = codex.inner().clone();
    tauri::async_runtime::spawn_blocking(move || match provider {
        ProviderId::Claude => providers::claude::get_limits(&support),
        ProviderId::Codex => providers::codex::get_limits(&codex),
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn get_provider_stats(
    provider: ProviderId,
    codex: tauri::State<'_, CodexAppServer>,
) -> Result<ProviderStats, String> {
    let codex = codex.inner().clone();
    tauri::async_runtime::spawn_blocking(move || match provider {
        ProviderId::Claude => providers::claude::get_stats(),
        ProviderId::Codex => providers::codex::get_stats(&codex),
    })
    .await
    .map_err(|error| error.to_string())?
}

// ── corner-placement system ───────────────────────────────────────────────────

pub(crate) fn app_support_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join("Library/Application Support/com.chulheong.quotaglass")
}

fn copy_missing_tree(source: &Path, destination: &Path, skip_login_marker: bool) {
    let entries = match fs::read_dir(source) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let _ = fs::create_dir_all(destination);
    for entry in entries.flatten() {
        let source_path = entry.path();
        if skip_login_marker
            && source_path.file_name().and_then(|name| name.to_str()) == Some(".login-registered")
        {
            continue;
        }
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_missing_tree(&source_path, &destination_path, skip_login_marker);
        } else if !destination_path.exists() {
            let _ = fs::copy(source_path, destination_path);
        }
    }
}

fn migrate_legacy_state() {
    let Ok(home) = std::env::var("HOME") else {
        return;
    };
    let library = std::path::PathBuf::from(home).join("Library");
    let old_support = library.join("Application Support/com.chulheong.claudeusage");
    copy_missing_tree(&old_support, &app_support_dir(), true);

    // Tauri's localStorage lives under the bundle-specific WebKit directory.
    // Copy it only before the new bundle creates its own store.
    let old_webkit = library.join("WebKit/com.chulheong.claudeusage");
    let new_webkit = library.join("WebKit/com.chulheong.quotaglass");
    if !new_webkit.exists() {
        copy_missing_tree(&old_webkit, &new_webkit, false);
    }
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
            .join("Library/Application Support/com.chulheong.quotaglass/.login-registered");
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

fn show_hide_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyU)
}

fn provider_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyP)
}

fn view_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyV)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    migrate_legacy_state();
    tauri::Builder::default()
        .manage(CodexAppServer::default())
        .manage(DragState {
            is_dragging: AtomicBool::new(false),
            is_animating: AtomicBool::new(false),
            drag_watching: AtomicBool::new(false),
        })
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        if shortcut == &show_hide_shortcut() {
                            toggle_window(app);
                        } else if shortcut == &provider_shortcut() {
                            let _ = app.emit("provider-shortcut", ());
                        } else if shortcut == &view_shortcut() {
                            let _ = app.emit("view-shortcut", ());
                        }
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            get_provider_stats,
            get_provider_limits,
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
            for (label, shortcut) in [
                ("show/hide", show_hide_shortcut()),
                ("provider", provider_shortcut()),
                ("view", view_shortcut()),
            ] {
                if let Err(error) = app.global_shortcut().register(shortcut) {
                    eprintln!("Could not register {label} shortcut: {error}");
                }
            }

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
            // Dedicated 32×32 template icon keeps the menu-bar item at the
            // standard 16pt height. icon_as_template lets macOS invert it for
            // light/dark mode automatically.
            tray = tray.icon(make_tray_icon()).icon_as_template(true);
            let _tray = tray.build(app)?;

            if let Some(win) = app.get_webview_window("main") {
                position_at_corner(&win, &load_corner());
            }

            // Watch both providers' local session stores. The provider id in the
            // event lets the frontend ignore background writes for the inactive
            // provider while retaining one debounced watcher.
            if let Ok(home) = std::env::var("HOME") {
                let claude_path = std::path::PathBuf::from(&home)
                    .join(".claude")
                    .join("projects");
                let codex_path = std::path::PathBuf::from(&home)
                    .join(".codex")
                    .join("sessions");
                let callback_claude_path = claude_path.clone();
                let callback_codex_path = codex_path.clone();
                let (tx, rx) = std::sync::mpsc::channel::<ProviderId>();
                let app_h = app.handle().clone();

                let watcher_result =
                    notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                        if let Ok(event) = res {
                            let has_jsonl = event
                                .paths
                                .iter()
                                .any(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"));
                            if has_jsonl {
                                let provider = if event
                                    .paths
                                    .iter()
                                    .any(|path| path.starts_with(&callback_codex_path))
                                {
                                    ProviderId::Codex
                                } else if event
                                    .paths
                                    .iter()
                                    .any(|path| path.starts_with(&callback_claude_path))
                                {
                                    ProviderId::Claude
                                } else {
                                    return;
                                };
                                let _ = tx.send(provider);
                            }
                        }
                    });

                if let Ok(mut watcher) = watcher_result {
                    let mut watching = false;
                    if claude_path.exists()
                        && watcher
                            .watch(&claude_path, RecursiveMode::Recursive)
                            .is_ok()
                    {
                        watching = true;
                    }
                    if codex_path.exists()
                        && watcher.watch(&codex_path, RecursiveMode::Recursive).is_ok()
                    {
                        watching = true;
                    }
                    if watching {
                        // Debounce: emit only after 1.5 s of silence.
                        std::thread::spawn(move || {
                            let mut pending: HashMap<ProviderId, Instant> = HashMap::new();
                            loop {
                                match rx.recv_timeout(Duration::from_millis(100)) {
                                    Ok(provider) => {
                                        pending.insert(provider, Instant::now());
                                    }
                                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                                        let ready: Vec<_> = pending
                                            .iter()
                                            .filter(|(_, updated)| {
                                                updated.elapsed() >= Duration::from_millis(1500)
                                            })
                                            .map(|(provider, _)| *provider)
                                            .collect();
                                        for provider in ready {
                                            let _ = app_h.emit("usage-updated", provider);
                                            pending.remove(&provider);
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                        });
                        app.manage(FsWatcher(std::sync::Mutex::new(watcher)));
                    }
                }
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_does_not_overwrite_new_state_or_copy_login_marker() {
        let root = std::env::temp_dir().join(format!(
            "quotaglass-migration-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let old = root.join("old");
        let new = root.join("new");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::write(old.join(".corner"), "tl").unwrap();
        fs::write(old.join(".login-registered"), "1").unwrap();
        fs::write(old.join("cache.json"), "{}").unwrap();
        fs::write(new.join(".corner"), "br").unwrap();

        copy_missing_tree(&old, &new, true);

        assert_eq!(fs::read_to_string(new.join(".corner")).unwrap(), "br");
        assert!(new.join("cache.json").exists());
        assert!(!new.join(".login-registered").exists());
        let _ = fs::remove_dir_all(root);
    }
}
