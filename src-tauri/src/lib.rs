//! Tauri backend. Owns all secrets and network access; the renderer only sees
//! the sanitized `UsageSummary`.

mod cache;
mod config;
mod fetchers;
mod fs_util;
mod mock;
mod models;
mod secrets;
mod util;

use cache::CacheState;
use config::Config;
use models::{EnabledProviders, UsageSummary};
use std::time::Duration as StdDuration;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Mutex;

const SCREENSAVER_REFRESH_INTERVAL_MINUTES: u64 = 15;
const MAX_DEEPSEEK_API_KEY_BYTES: usize = 2_048;

pub struct AppState {
    config: Mutex<Config>,
    /// "normal" | "fullscreen" | "screensaver" — drives how the UI exits.
    mode: Mutex<String>,
    judge_demo: bool,
    summary: Mutex<UsageSummary>,
    cache: Mutex<CacheState>,
    refreshing: Mutex<bool>,
    refresh_pending: Mutex<bool>,
}

struct RefreshFlagGuard {
    app: AppHandle,
    armed: bool,
}

impl RefreshFlagGuard {
    fn new(app: &AppHandle) -> Self {
        Self {
            app: app.clone(),
            armed: true,
        }
    }

    async fn release(mut self) {
        *self.app.state::<AppState>().refreshing.lock().await = false;
        self.armed = false;
    }
}

impl Drop for RefreshFlagGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let app = self.app.clone();
        tauri::async_runtime::spawn(async move {
            *app.state::<AppState>().refreshing.lock().await = false;
        });
    }
}

/// Launch flags parsed from argv, mirroring the WinForms `AppOptions`.
#[derive(Default)]
struct AppOptions {
    fullscreen: bool,
    screensaver: bool,
    open_config: bool,
    preview: bool,
    judge_demo: bool,
}

impl AppOptions {
    fn parse<I: Iterator<Item = String>>(args: I) -> Self {
        let mut o = AppOptions::default();
        let mut it = args.peekable();
        while let Some(arg) = it.next() {
            let lower = arg.to_lowercase();
            match lower.as_str() {
                "/s" | "-s" => {
                    o.screensaver = true;
                    o.fullscreen = true;
                }
                "--fullscreen" => o.fullscreen = true,
                "--config" | "/c" => o.open_config = true,
                "--judge-demo" => o.judge_demo = true,
                "/p" => {
                    o.preview = true;
                    it.next(); // consume the following HWND argument
                }
                s if s.starts_with("/c:") => o.open_config = true,
                s if s.starts_with("/p") => o.preview = true,
                _ => {}
            }
        }
        o
    }
}

/// Return the latest summary (cached or live). Never errors — keeps the UI fed.
#[tauri::command]
async fn get_summary(state: State<'_, AppState>) -> Result<UsageSummary, String> {
    Ok(state.summary.lock().await.clone())
}

/// Kick a refresh in the background; the result arrives via the `summary` event.
#[tauri::command]
async fn refresh_now(app: AppHandle) -> Result<bool, String> {
    if !try_begin_refresh(&app).await {
        return Ok(false);
    }
    tauri::async_runtime::spawn(async move {
        run_refresh(&app).await;
    });
    Ok(true)
}

async fn publish_enabled_providers(app: &AppHandle, enabled_providers: EnabledProviders) {
    let state = app.state::<AppState>();
    let summary = if state.judge_demo {
        mock::summary("normal", enabled_providers)
            .expect("judge demo mock mode is always available")
    } else {
        let cache = state.cache.lock().await;
        fetchers::summary_from_cache(&cache, enabled_providers)
    };
    *state.summary.lock().await = summary.clone();
    let _ = app.emit("summary", &summary);

    if queue_refresh(app).await {
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            run_refresh(&handle).await;
        });
    }
}

/// Quit (Esc from fullscreen / screensaver, or input in screensaver mode).
#[tauri::command]
fn exit_app(app: AppHandle) {
    app.exit(0);
}

/// Let the renderer know how it was launched so it can wire exit-on-input.
#[tauri::command]
async fn launch_mode(state: State<'_, AppState>) -> Result<String, String> {
    Ok(state.mode.lock().await.clone())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DisplaySettings {
    enabled_providers: EnabledProviders,
    window_mode: String,
}

/// Atomically save changed display settings. Omitted fields are left untouched,
/// so a provider-only save cannot persist a temporary CLI fullscreen override.
#[tauri::command]
async fn save_display_settings(
    app: AppHandle,
    enabled_providers: Option<EnabledProviders>,
    window_mode: Option<String>,
    deepseek_api_key: Option<String>,
) -> Result<DisplaySettings, String> {
    let requested_window = match window_mode {
        Some(mode) => {
            Some(normalize_window_mode(&mode).ok_or_else(|| "INVALID WINDOW MODE".to_string())?)
        }
        None => None,
    };
    let requested_deepseek_key = normalize_deepseek_api_key(deepseek_api_key)?;
    let state = app.state::<AppState>();
    if state.judge_demo && requested_deepseek_key.is_some() {
        return Err("CREDENTIAL STORAGE DISABLED IN DEMO".into());
    }
    let mut current_mode = state.mode.lock().await;
    if requested_window.is_some() && current_mode.as_str() == "screensaver" {
        return Err("SCREENSAVER MODE IS FIXED AT LAUNCH".into());
    }

    let previous_mode = current_mode.clone();
    let mut current_config = state.config.lock().await;
    if current_config.load_error {
        return Err("CONFIG FILE IS INVALID".into());
    }

    let previous_enabled = current_config.enabled_providers;
    let next_enabled = enabled_providers.unwrap_or(previous_enabled);
    let providers_changed = next_enabled != previous_enabled;
    let deepseek_key_changed = requested_deepseek_key
        .as_ref()
        .is_some_and(|key| key != &current_config.deep_seek_api_key);
    let mut next_config = current_config.clone();
    next_config.enabled_providers = next_enabled;
    if let Some(key) = requested_deepseek_key {
        next_config.deep_seek_api_key = key;
    }
    if let Some(requested) = requested_window {
        next_config.window_mode = requested.to_string();
    }

    let chrome_changed = requested_window.is_some_and(|requested| requested != previous_mode);
    if let Some(requested) = requested_window.filter(|_| chrome_changed) {
        apply_window_chrome(&app, requested == "fullscreen", false)?;
        *current_mode = requested.to_string();
    }

    let save_result = if state.judge_demo {
        if providers_changed {
            config::save_judge_demo_selection(next_enabled)
        } else {
            Ok(())
        }
    } else if providers_changed || requested_window.is_some() || deepseek_key_changed {
        config::save(&next_config)
    } else {
        Ok(())
    };

    if save_result.is_err() {
        let rollback_failed = chrome_changed
            && apply_window_chrome(&app, previous_mode == "fullscreen", false).is_err();
        *current_mode = mode_after_failed_save(&previous_mode, requested_window, rollback_failed);
        let message = if state.judge_demo {
            "DEMO SETTINGS SAVE FAILED"
        } else {
            "CONFIG SAVE FAILED"
        };
        return Err(if rollback_failed {
            format!("{message}; WINDOW ROLLBACK FAILED")
        } else {
            message.to_string()
        });
    }

    *current_config = next_config;
    if let Some(requested) = requested_window {
        *current_mode = requested.to_string();
    }
    let saved = DisplaySettings {
        enabled_providers: current_config.enabled_providers,
        window_mode: current_mode.clone(),
    };
    drop(current_config);
    drop(current_mode);

    if providers_changed {
        publish_enabled_providers(&app, saved.enabled_providers).await;
    } else if deepseek_key_changed && queue_refresh(&app).await {
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            run_refresh(&handle).await;
        });
    }

    Ok(saved)
}

/// Blank input means "leave the existing key unchanged". A non-blank value is
/// trimmed and bounded before it can enter Config; no error includes the key.
fn normalize_deepseek_api_key(value: Option<String>) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > MAX_DEEPSEEK_API_KEY_BYTES {
        return Err("DEEPSEEK KEY IS TOO LONG".into());
    }
    if value
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err("DEEPSEEK KEY CONTAINS INVALID CHARACTERS".into());
    }
    Ok(Some(value.to_string()))
}

fn mode_after_failed_save(
    previous_mode: &str,
    requested_window: Option<&str>,
    rollback_failed: bool,
) -> String {
    if rollback_failed {
        requested_window.unwrap_or(previous_mode).to_string()
    } else {
        previous_mode.to_string()
    }
}

fn launch_chrome_failure_is_fatal(screensaver: bool) -> bool {
    screensaver
}

fn normalize_window_mode(mode: &str) -> Option<&'static str> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "normal" | "windowed" | "window" => Some("normal"),
        "fullscreen" | "full-screen" | "full" => Some("fullscreen"),
        _ => None,
    }
}

/// Apply OS-level window chrome. Shared by startup and settings.
fn apply_window_chrome(app: &AppHandle, fullscreen: bool, screensaver: bool) -> Result<(), String> {
    let win = app
        .get_webview_window("main")
        .ok_or_else(|| "MAIN WINDOW MISSING".to_string())?;
    win.set_fullscreen(fullscreen)
        .map_err(|_| "FULLSCREEN TOGGLE FAILED".to_string())?;
    // Keep the cursor usable in normal/fullscreen so Settings remains clickable;
    // only screensaver hides it for kiosk-style idle display.
    let _ = win.set_cursor_visible(!screensaver);
    if screensaver {
        let _ = win.set_always_on_top(true);
    } else {
        let _ = win.set_always_on_top(false);
    }
    let _ = win.set_focus();
    Ok(())
}

/// Resolve effective launch mode: CLI screensaver/fullscreen overrides config.
fn resolve_launch_mode(options: &AppOptions, config_window_mode: &str) -> String {
    if options.screensaver {
        return "screensaver".into();
    }
    if options.fullscreen {
        return "fullscreen".into();
    }
    if normalize_window_mode(config_window_mode) == Some("fullscreen") {
        "fullscreen".into()
    } else {
        "normal".into()
    }
}

/// True only for the isolated offline synthetic demo launch mode.
#[tauri::command]
fn judge_demo(state: State<'_, AppState>) -> bool {
    state.judge_demo
}

/// Open a path in the platform's default editor (used for `--config`).
fn open_path(path: &std::path::Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("notepad.exe").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

async fn try_begin_refresh(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    let mut flag = state.refreshing.lock().await;
    if *flag {
        return false;
    }
    *flag = true;

    let refreshing_summary = {
        let mut summary = state.summary.lock().await;
        summary.status = "refreshing".into();
        summary.clone()
    };
    let _ = app.emit("summary", &refreshing_summary);
    true
}

async fn run_refresh(app: &AppHandle) {
    loop {
        let refresh_flag = RefreshFlagGuard::new(app);
        let state = app.state::<AppState>();
        let config = state.config.lock().await.clone();

        let summary = if state.judge_demo {
            mock::summary("normal", config.enabled_providers)
                .expect("judge demo mock mode is always available")
        } else if config.load_error {
            config_error_summary(config.enabled_providers)
        } else if let Some(summary) = mock::summary(&config.mock_mode, config.enabled_providers) {
            summary
        } else {
            // Do not hold the shared cache lock across provider network calls.
            // Settings can render a last-known snapshot while this refresh runs.
            let mut working_cache = state.cache.lock().await.clone();
            let s = fetchers::collect_summary(&config, &mut working_cache).await;
            if config.enabled_providers.count() > 0 {
                if let Err(err) = working_cache.save() {
                    eprintln!("failed to persist dashboard cache: {err}");
                }
            }
            *state.cache.lock().await = working_cache;
            s
        };

        // A settings change can land while a network cycle is in flight.
        // Publish only the latest selection; the pending cycle below fetches it.
        let latest_enabled = state.config.lock().await.enabled_providers;
        let selection_changed = latest_enabled != config.enabled_providers;
        let summary = if selection_changed && state.judge_demo {
            mock::summary("normal", latest_enabled)
                .expect("judge demo mock mode is always available")
        } else if selection_changed {
            let cache = state.cache.lock().await;
            fetchers::summary_from_cache(&cache, latest_enabled)
        } else {
            summary
        };

        *state.summary.lock().await = summary.clone();
        let _ = app.emit("summary", &summary);

        refresh_flag.release().await;

        if !begin_pending_refresh(app).await {
            break;
        }
    }
}

async fn queue_refresh(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    let mut pending = state.refresh_pending.lock().await;
    *pending = true;
    if try_begin_refresh(app).await {
        *pending = false;
        true
    } else {
        false
    }
}

async fn begin_pending_refresh(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    let mut pending = state.refresh_pending.lock().await;
    if !*pending || !try_begin_refresh(app).await {
        return false;
    }
    *pending = false;
    true
}

fn config_error_summary(enabled: EnabledProviders) -> UsageSummary {
    let mut summary = UsageSummary::empty();
    summary.status = if enabled.count() == 0 {
        "idle".into()
    } else {
        "error".into()
    };
    summary.enabled_providers = enabled;
    summary.services.codex.status = "CONFIG ERROR".into();
    summary.services.claude.status = "CONFIG ERROR".into();
    summary.services.deepseek.status = "CONFIG ERROR".into();
    summary.services.grok.status = "CONFIG ERROR".into();
    summary.services.codex.data_may_be_stale = true;
    summary.services.claude.data_may_be_stale = true;
    summary.services.deepseek.data_may_be_stale = true;
    summary.services.grok.data_may_be_stale = true;
    summary
}

async fn do_refresh(app: &AppHandle) {
    if try_begin_refresh(app).await {
        run_refresh(app).await;
    }
}

fn effective_refresh_interval_minutes(configured: u64, screensaver: bool) -> u64 {
    if screensaver {
        configured.max(SCREENSAVER_REFRESH_INTERVAL_MINUTES)
    } else {
        configured
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let options = AppOptions::parse(std::env::args().skip(1));

    // `/c` / `--config`: open the config file and exit without launching the UI.
    if options.open_config {
        let _ = config::load_or_create(); // make sure it exists first
        open_path(&config::config_path());
        return;
    }

    // `/p <HWND>`: screensaver preview. We don't render into the preview pane;
    // per the brief, the requirement is simply to not crash.
    if options.preview {
        return;
    }

    let (mut config, cache) = if options.judge_demo {
        (Config::default(), CacheState::default())
    } else {
        (config::load_or_create(), CacheState::load())
    };
    let initial = if options.judge_demo {
        config.enabled_providers = config::load_judge_demo_selection();
        config.mock_mode = "normal".into();
        mock::summary(&config.mock_mode, config.enabled_providers)
            .expect("judge demo mock mode is always available")
    } else {
        // Seed from cache so the dashboard is never blank on startup.
        fetchers::summary_from_cache(&cache, config.enabled_providers)
    };
    let interval_minutes =
        effective_refresh_interval_minutes(config.refresh_interval_minutes, options.screensaver);
    let mode = resolve_launch_mode(&options, &config.window_mode);
    let start_fullscreen = mode == "fullscreen" || mode == "screensaver";
    let start_screensaver = mode == "screensaver";

    tauri::Builder::default()
        .manage(AppState {
            config: Mutex::new(config),
            mode: Mutex::new(mode),
            judge_demo: options.judge_demo,
            summary: Mutex::new(initial),
            cache: Mutex::new(cache),
            refreshing: Mutex::new(false),
            refresh_pending: Mutex::new(false),
        })
        .invoke_handler(tauri::generate_handler![
            get_summary,
            refresh_now,
            save_display_settings,
            exit_app,
            launch_mode,
            judge_demo
        ])
        .setup(move |app| {
            // Apply fullscreen / screensaver window state (CLI or persisted windowMode).
            if start_fullscreen {
                let handle = app.handle().clone();
                if let Err(err) = apply_window_chrome(&handle, true, start_screensaver) {
                    eprintln!("failed to apply requested launch window mode: {err}");
                    if launch_chrome_failure_is_fatal(start_screensaver) {
                        app.handle().exit(1);
                        return Ok(());
                    }
                    if let Ok(mut mode) = app.state::<AppState>().mode.try_lock() {
                        *mode = "normal".into();
                    }
                }
            }

            let handle = app.handle().clone();
            // Background refresh loop: refresh immediately, then every interval.
            tauri::async_runtime::spawn(async move {
                loop {
                    do_refresh(&handle).await;
                    tokio::time::sleep(StdDuration::from_secs(interval_minutes.saturating_mul(60)))
                        .await;
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::{
        effective_refresh_interval_minutes, launch_chrome_failure_is_fatal, mode_after_failed_save,
        normalize_deepseek_api_key, normalize_window_mode, resolve_launch_mode, AppOptions,
    };

    #[test]
    fn deepseek_key_input_is_optional_trimmed_and_bounded() {
        assert_eq!(normalize_deepseek_api_key(None).unwrap(), None);
        assert_eq!(
            normalize_deepseek_api_key(Some("   ".into())).unwrap(),
            None
        );
        assert_eq!(
            normalize_deepseek_api_key(Some("  sk-test  ".into())).unwrap(),
            Some("sk-test".into())
        );
        assert!(normalize_deepseek_api_key(Some("sk bad".into())).is_err());
        assert!(normalize_deepseek_api_key(Some("sk\0bad".into())).is_err());
        assert!(normalize_deepseek_api_key(Some("x".repeat(2_049))).is_err());
    }

    #[test]
    fn failed_settings_save_tracks_the_actual_window_after_rollback() {
        assert_eq!(
            mode_after_failed_save("normal", Some("fullscreen"), false),
            "normal"
        );
        assert_eq!(
            mode_after_failed_save("normal", Some("fullscreen"), true),
            "fullscreen"
        );
    }

    #[test]
    fn screensaver_chrome_failure_is_fatal() {
        assert!(launch_chrome_failure_is_fatal(true));
        assert!(!launch_chrome_failure_is_fatal(false));
    }

    #[test]
    fn screensaver_uses_a_quieter_refresh_floor() {
        assert_eq!(effective_refresh_interval_minutes(5, false), 5);
        assert_eq!(effective_refresh_interval_minutes(5, true), 15);
        assert_eq!(effective_refresh_interval_minutes(30, true), 30);
    }

    #[test]
    fn judge_demo_argument_is_recognized_without_changing_window_mode() {
        let options = AppOptions::parse(["--judge-demo".to_string()].into_iter());

        assert!(options.judge_demo);
        assert_eq!(resolve_launch_mode(&options, "normal"), "normal");
    }

    #[test]
    fn config_window_mode_applies_when_no_cli_override() {
        let options = AppOptions::default();
        assert_eq!(resolve_launch_mode(&options, "fullscreen"), "fullscreen");
        assert_eq!(resolve_launch_mode(&options, "normal"), "normal");
        assert_eq!(resolve_launch_mode(&options, "junk"), "normal");
    }

    #[test]
    fn cli_fullscreen_and_screensaver_override_config() {
        let fullscreen = AppOptions::parse(["--fullscreen".to_string()].into_iter());
        assert_eq!(resolve_launch_mode(&fullscreen, "normal"), "fullscreen");

        let saver = AppOptions::parse(["/s".to_string()].into_iter());
        assert_eq!(resolve_launch_mode(&saver, "normal"), "screensaver");
    }

    #[test]
    fn window_mode_aliases_normalize() {
        assert_eq!(normalize_window_mode("windowed"), Some("normal"));
        assert_eq!(normalize_window_mode("FULLSCREEN"), Some("fullscreen"));
        assert_eq!(normalize_window_mode("kiosk"), None);
    }
}
