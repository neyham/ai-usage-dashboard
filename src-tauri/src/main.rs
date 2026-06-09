// Hide the console window on Windows in release builds (screensaver-friendly).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    ai_usage_dashboard_lib::run()
}
