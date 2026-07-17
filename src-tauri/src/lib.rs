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

pub struct AppState {
    config: Mutex<Config>,
    /// "normal" | "fullscreen" | "screensaver" — drives how the UI exits.
    mode: String,
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

    fn mode(&self) -> String {
        if self.screensaver {
            "screensaver".into()
        } else if self.fullscreen {
            "fullscreen".into()
        } else {
            "normal".into()
        }
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

/// Persist the home-screen selection without exposing the rest of config.json.
#[tauri::command]
async fn save_enabled_providers(
    app: AppHandle,
    enabled_providers: EnabledProviders,
) -> Result<EnabledProviders, String> {
    let state = app.state::<AppState>();
    {
        let mut current = state.config.lock().await;
        if current.load_error {
            return Err("CONFIG FILE IS INVALID".into());
        }
        let mut next = current.clone();
        next.enabled_providers = enabled_providers;
        if state.judge_demo {
            config::save_judge_demo_selection(enabled_providers)
                .map_err(|_| "DEMO SETTINGS SAVE FAILED".to_string())?;
        } else {
            config::save(&next).map_err(|_| "CONFIG SAVE FAILED".to_string())?;
        }
        *current = next;
    }

    let summary = if state.judge_demo {
        mock::summary("normal", enabled_providers)
            .expect("judge demo mock mode is always available")
    } else {
        let cache = state.cache.lock().await;
        fetchers::summary_from_cache(&cache, enabled_providers)
    };
    *state.summary.lock().await = summary.clone();
    let _ = app.emit("summary", &summary);

    if queue_refresh(&app).await {
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            run_refresh(&handle).await;
        });
    }

    Ok(enabled_providers)
}

/// Quit (Esc from fullscreen / screensaver, or input in screensaver mode).
#[tauri::command]
fn exit_app(app: AppHandle) {
    app.exit(0);
}

/// Let the renderer know how it was launched so it can wire exit-on-input.
#[tauri::command]
fn launch_mode(state: State<'_, AppState>) -> String {
    state.mode.clone()
}

/// True only for the isolated, synthetic Build Week judge experience.
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
            let mut cache = state.cache.lock().await;
            let s = fetchers::collect_summary(&config, &mut cache).await;
            if config.enabled_providers.count() > 0 {
                if let Err(err) = cache.save() {
                    eprintln!("failed to persist dashboard cache: {err}");
                }
            }
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
    summary.services.codex.data_may_be_stale = true;
    summary.services.claude.data_may_be_stale = true;
    summary.services.deepseek.data_may_be_stale = true;
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
    let mode = options.mode();

    tauri::Builder::default()
        .manage(AppState {
            config: Mutex::new(config),
            mode,
            judge_demo: options.judge_demo,
            summary: Mutex::new(initial),
            cache: Mutex::new(cache),
            refreshing: Mutex::new(false),
            refresh_pending: Mutex::new(false),
        })
        .invoke_handler(tauri::generate_handler![
            get_summary,
            refresh_now,
            save_enabled_providers,
            exit_app,
            launch_mode,
            judge_demo
        ])
        .setup(move |app| {
            // Apply fullscreen / screensaver window state.
            if options.fullscreen {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.set_fullscreen(true);
                    let _ = win.set_cursor_visible(false);
                    let _ = win.set_focus();
                    if options.screensaver {
                        let _ = win.set_always_on_top(true);
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
    use super::{effective_refresh_interval_minutes, AppOptions};

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
        assert_eq!(options.mode(), "normal");
    }
}
