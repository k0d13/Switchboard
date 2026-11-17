//! The tray icon: left click opens the flyout, right click shows the menu built from features.

use std::sync::Arc;

use parking_lot::Mutex;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;

use crate::core::feature::Tone;
use crate::core::host::{AppState, Host};

const ICON: &[u8] = include_bytes!("../icons/tray.png");
const ICON_ACTIVE: &[u8] = include_bytes!("../icons/tray-active.png");

/// What the menu was last built from, so status updates that change nothing don't rebuild it.
static LAST: Mutex<String> = Mutex::new(String::new());

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    TrayIconBuilder::with_id("main")
        .icon(Image::from_bytes(ICON)?)
        .tooltip("Switchboard")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, position, .. } =
                event
            {
                crate::ui::toggle_flyout(tray.app_handle(), position);
            }
        })
        .build(app)?;
    Ok(())
}

fn on_menu(app: &AppHandle, id: &str) {
    match id {
        "settings" => crate::ui::open_settings(app, None),
        "quit" => app.exit(0),
        "autostart" => {
            let al = app.autolaunch();
            let _ = if al.is_enabled().unwrap_or(false) { al.disable() } else { al.enable() };
            app.state::<Arc<Host>>().changed();
        }
        _ => {
            if let Some((feature, action)) = id.strip_prefix("act:").and_then(|r| r.split_once(':')) {
                app.state::<Arc<Host>>().spawn_action(feature.to_string(), action.to_string());
            }
        }
    }
}

pub fn refresh(app: &AppHandle, state: &AppState) {
    let Some(tray) = app.tray_by_id("main") else { return };

    let signature = serde_json::to_string(&(
        state.autostart,
        state
            .features
            .iter()
            .map(|f| {
                let actions: Vec<_> = f.actions.iter().map(|a| (&a.label, a.active, &a.hotkey)).collect();
                (f.enabled, f.status.as_ref().map(|s| &s.text), f.busy, actions)
            })
            .collect::<Vec<_>>(),
    ))
    .unwrap_or_default();
    {
        let mut last = LAST.lock();
        if *last == signature {
            return;
        }
        *last = signature;
    }

    if let Ok(menu) = build_menu(app, state) {
        let _ = tray.set_menu(Some(menu));
    }

    let mut tip = String::from("Switchboard");
    for f in state.features.iter().filter(|f| f.enabled) {
        if let Some(s) = &f.status {
            tip.push_str(&format!("\n{}: {}", f.name, s.text));
        }
    }
    // Windows cuts tray tooltips at 127 characters
    let tip: String = tip.chars().take(127).collect();
    let _ = tray.set_tooltip(Some(&tip));

    let active = state.features.iter().any(|f| f.enabled && f.status.as_ref().is_some_and(|s| s.tone == Tone::Active));
    if let Ok(img) = Image::from_bytes(if active { ICON_ACTIVE } else { ICON }) {
        let _ = tray.set_icon(Some(img));
    }
}

fn build_menu(app: &AppHandle, state: &AppState) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    for f in state.features.iter().filter(|f| f.enabled) {
        let title = match &f.status {
            Some(s) => format!("{}: {}", f.name, s.text),
            None => f.name.to_string(),
        };
        menu.append(&MenuItem::with_id(app, format!("title:{}", f.id), title, false, None::<&str>)?)?;
        for a in &f.actions {
            let id = format!("act:{}:{}", f.id, a.id);
            // The hotkey goes after a tab, which Windows right-aligns like a menu accelerator
            let label = match &a.hotkey {
                Some(k) => format!("{}\t{}", a.label, k),
                None => a.label.clone(),
            };
            if a.active {
                menu.append(&CheckMenuItem::with_id(app, id, label, !f.busy, true, None::<&str>)?)?;
            } else {
                menu.append(&MenuItem::with_id(app, id, label, !f.busy, None::<&str>)?)?;
            }
        }
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }
    menu.append(&MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?)?;
    menu.append(&CheckMenuItem::with_id(app, "autostart", "Start at login", true, state.autostart, None::<&str>)?)?;
    menu.append(&MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?)?;
    Ok(menu)
}
