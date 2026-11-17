mod commands;
mod core;
mod features;
mod hotkeys;
mod platform;
mod tray;
mod ui;

use std::sync::Arc;

use tauri::{AppHandle, Manager, RunEvent};
use tauri_plugin_autostart::MacosLauncher;

use crate::core::host::Host;
use crate::core::log::Level;

pub fn run() {
    let app = tauri::Builder::default()
        // Must be first: a second launch hands its arguments to the running copy and exits
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| handle_args(app, &argv, true)))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--background"])))
        .plugin(tauri_plugin_global_shortcut::Builder::new().with_handler(hotkeys::handler).build())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::get_schema,
            commands::set_enabled,
            commands::set_settings,
            commands::run_action,
            commands::set_hotkey,
            commands::set_autostart,
            commands::get_logs,
            commands::open_settings,
            commands::flyout_resize,
            commands::open_url,
            commands::quit,
        ])
        .setup(|app| {
            let host = Host::new(app.handle().clone(), features::all());
            app.manage(host.clone());
            host.app_log(Level::Info, format!("Switchboard {} started", env!("CARGO_PKG_VERSION")));
            tray::create(app.handle())?;
            host.start_enabled();
            hotkeys::sync(&host);
            let argv: Vec<String> = std::env::args().collect();
            handle_args(app.handle(), &argv, false);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("couldn't start Switchboard");

    app.run(|app, event| match event {
        // Closing the last window shouldn't quit a tray app; only `app.exit` (Quit) passes a code
        RunEvent::ExitRequested { code: None, api, .. } => api.prevent_exit(),
        RunEvent::Exit => {
            if let Some(host) = app.try_state::<Arc<Host>>() {
                host.shutdown();
            }
        }
        _ => {}
    });
}

/// `switchboard` opens settings, `switchboard --background` just sits in the tray (login),
/// `switchboard run xbox.xbox-mode` runs an action (for shortcuts, Stream Deck and so on).
fn handle_args(app: &AppHandle, argv: &[String], second_launch: bool) {
    let args: Vec<&str> = argv.iter().skip(1).map(String::as_str).collect();
    match args.as_slice() {
        ["run", target, ..] => {
            let host = app.state::<Arc<Host>>();
            match target.split_once('.') {
                Some((feature, action)) => host.spawn_action(feature.to_string(), action.to_string()),
                None => host.app_log(Level::Warn, format!("run needs feature.action, got {target}")),
            }
        }
        // Same as clicking the tray icon; opens on the monitor the mouse is on
        ["flyout", ..] => {
            let at = app.cursor_position().unwrap_or_default();
            ui::toggle_flyout(app, at);
        }
        ["--background", ..] if !second_launch => {}
        _ => ui::open_settings(app, None),
    }
}
