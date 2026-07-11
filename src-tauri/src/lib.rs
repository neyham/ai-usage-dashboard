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
use models::UsageSummary;
use std::time::Duration as StdDuration;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Mutex;

const SCREENSAVER_REFRESH_INTERVAL_MINUTES: u64 = 15;

pub struct AppState {
    config: Config,
    /// "normal" | "fullscreen" | "screensaver" — drives how the UI exits.
    mode: String,
    summary: Mutex<UsageSummary>,
    cache: Mutex<CacheState>,
    refreshing: Mutex<bool>,
}

/// Launch flags parsed from argv, mirroring the WinForms `AppOptions`.
#[derive(Default)]
struct AppOptions {
    fullscreen: bool,
    screensaver: bool,
    open_config: bool,
    preview: bool,
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
    let state = app.state::<AppState>();

    let summary = if state.config.load_error {
        config_error_summary()
    } else if let Some(summary) = mock::summary(&state.config.mock_mode) {
        summary
    } else {
        let mut cache = state.cache.lock().await;
        let s = fetchers::collect_summary(&state.config, &mut cache).await;
        if let Err(err) = cache.save() {
            eprintln!("failed to persist dashboard cache: {err}");
        }
        s
    };

    *state.summary.lock().await = summary.clone();
    let _ = app.emit("summary", &summary);

    *state.refreshing.lock().await = false;
}

fn config_error_summary() -> UsageSummary {
    let mut summary = UsageSummary::empty();
    summary.status = "error".into();
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

    let config = config::load_or_create();
    let cache = CacheState::load();

    // Seed from cache so the dashboard is never blank on startup.
    let initial = fetchers::summary_from_cache(&cache);
    let interval_minutes =
        effective_refresh_interval_minutes(config.refresh_interval_minutes, options.screensaver);
    let mode = options.mode();

    tauri::Builder::default()
        .manage(AppState {
            config,
            mode,
            summary: Mutex::new(initial),
            cache: Mutex::new(cache),
            refreshing: Mutex::new(false),
        })
        .invoke_handler(tauri::generate_handler![
            get_summary,
            refresh_now,
            exit_app,
            launch_mode
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
    use super::effective_refresh_interval_minutes;

    #[test]
    fn screensaver_uses_a_quieter_refresh_floor() {
        assert_eq!(effective_refresh_interval_minutes(5, false), 5);
        assert_eq!(effective_refresh_interval_minutes(5, true), 15);
        assert_eq!(effective_refresh_interval_minutes(30, true), 30);
    }
}
