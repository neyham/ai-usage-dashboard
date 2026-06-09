//! Tauri backend. Owns all secrets and network access; the renderer only sees
//! the sanitized `UsageSummary`.

mod cache;
mod config;
mod fetchers;
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
async fn refresh_now(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn(async move {
        do_refresh(&app).await;
    });
    Ok(())
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

async fn do_refresh(app: &AppHandle) {
    let state = app.state::<AppState>();

    // Guard against overlapping refresh cycles.
    {
        let mut flag = state.refreshing.lock().await;
        if *flag {
            return;
        }
        *flag = true;
    }

    let summary = if !state.config.mock_mode.trim().is_empty() {
        mock::summary(&state.config.mock_mode)
    } else {
        let mut cache = state.cache.lock().await;
        let s = fetchers::collect_summary(&state.config, &mut cache).await;
        cache.save();
        s
    };

    *state.summary.lock().await = summary.clone();
    let _ = app.emit("summary", &summary);

    *state.refreshing.lock().await = false;
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
    let interval_minutes = config.refresh_interval_minutes.max(15);
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
                    tokio::time::sleep(StdDuration::from_secs(interval_minutes * 60)).await;
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
