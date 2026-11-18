//! Commands the UI calls. Mirrors the contract in src/lib/types.ts.

use std::sync::Arc;

use serde_json::Value;
use tauri::{AppHandle, State, WebviewWindow};

use crate::core::feature::Field;
use crate::core::host::{AppState, Host};
use crate::core::log::LogLine;

type H<'a> = State<'a, Arc<Host>>;

/// Runs host work on a blocking thread. Commands that aren't async run on the main thread, where
/// anything slow (a feature stopping, a lock held by a worker) would freeze every window.
async fn blocking<T: Send + 'static>(
    host: &H<'_>,
    f: impl FnOnce(Arc<Host>) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let host = host.inner().clone();
    tauri::async_runtime::spawn_blocking(move || f(host)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn get_state(host: H<'_>) -> Result<AppState, String> {
    blocking(&host, |h| Ok(h.state())).await
}

/// Can query hardware (e.g. DDC/CI capabilities take a second per monitor), so off the main thread.
#[tauri::command]
pub async fn get_schema(host: H<'_>, id: String) -> Result<Vec<Field>, String> {
    blocking(&host, move |h| h.schema(&id)).await
}

#[tauri::command]
pub async fn set_enabled(host: H<'_>, id: String, enabled: bool) -> Result<(), String> {
    blocking(&host, move |h| h.set_enabled(&id, enabled)).await
}

#[tauri::command]
pub async fn set_settings(host: H<'_>, id: String, settings: Value) -> Result<(), String> {
    blocking(&host, move |h| h.set_settings(&id, settings)).await
}

#[tauri::command]
pub async fn run_action(host: H<'_>, id: String, action: String) -> Result<(), String> {
    blocking(&host, move |h| h.run_action(&id, &action, false)).await
}

#[tauri::command]
pub fn set_hotkey(host: H, id: String, action: String, hotkey: Option<String>) -> Result<(), String> {
    crate::hotkeys::set(&host, &id, &action, hotkey)
}

#[tauri::command]
pub async fn set_autostart(host: H<'_>, enabled: bool) -> Result<(), String> {
    blocking(&host, move |h| {
        crate::platform::autostart::set(enabled).map_err(|e| e.to_string())?;
        h.changed();
        Ok(())
    })
    .await
}

#[tauri::command]
pub fn get_logs(host: H) -> Vec<LogLine> {
    host.logs()
}

#[tauri::command]
pub fn open_settings(app: AppHandle, feature: Option<String>) {
    crate::ui::flyout_to_settings(&app, feature);
}

#[tauri::command]
pub fn flyout_resize(window: WebviewWindow, height: f64) {
    if window.label() == "flyout" {
        crate::ui::flyout_resize(&window, height);
    }
}

#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("Only https links can be opened".into());
    }
    std::process::Command::new("explorer.exe").arg(&url).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn quit(app: AppHandle) {
    app.exit(0);
}
