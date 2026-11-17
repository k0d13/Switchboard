//! Global hotkeys for feature actions, through RegisterHotKey (tauri-plugin-global-shortcut).

use std::str::FromStr;
use std::sync::Arc;

use parking_lot::Mutex;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

use crate::core::host::Host;
use crate::core::log::Level;

struct Binding {
    shortcut: u32,
    feature: &'static str,
    action: &'static str,
}

static BINDINGS: Mutex<Vec<Binding>> = Mutex::new(Vec::new());

pub fn handler(app: &AppHandle, shortcut: &Shortcut, event: ShortcutEvent) {
    if event.state != ShortcutState::Pressed {
        return;
    }
    let target = BINDINGS
        .lock()
        .iter()
        .find(|b| b.shortcut == shortcut.id())
        .map(|b| (b.feature.to_string(), b.action.to_string()));
    if let Some((feature, action)) = target {
        app.state::<Arc<Host>>().spawn_action(feature, action);
    }
}

fn parse(accelerator: &str) -> Result<Shortcut, String> {
    Shortcut::from_str(accelerator).map_err(|_| format!("{accelerator} isn't a key combination Switchboard understands"))
}

/// Every (feature, action, accelerator) that should be registered right now.
fn wanted(host: &Arc<Host>) -> Vec<(&'static str, &'static str, String, String)> {
    let mut out = Vec::new();
    for f in &host.features {
        let info = f.info();
        if !host.enabled(info.id) {
            continue;
        }
        for a in f.actions(&host.ctx(info.id)) {
            if let Some(key) = host.hotkey_for(info.id, &a) {
                out.push((info.id, a.id, key, a.label.clone()));
            }
        }
    }
    out
}

/// Re-registers everything. Returns the accelerators Windows refused.
pub fn sync(host: &Arc<Host>) -> Vec<String> {
    let gs = host.app.global_shortcut();
    let _ = gs.unregister_all();
    let mut bindings = BINDINGS.lock();
    bindings.clear();
    let mut refused = Vec::new();
    for (feature, action, key, label) in wanted(host) {
        let shortcut = match parse(&key) {
            Ok(s) => s,
            Err(e) => {
                host.app_log(Level::Warn, e);
                continue;
            }
        };
        if bindings.iter().any(|b| b.shortcut == shortcut.id()) {
            continue; // conflicts are refused in `set`, so this only happens with a hand-edited file
        }
        match gs.register(shortcut) {
            Ok(()) => bindings.push(Binding { shortcut: shortcut.id(), feature, action }),
            Err(e) => {
                host.app_log(Level::Warn, format!("Couldn't use {key} for {label}: {e}"));
                refused.push(key);
            }
        }
    }
    refused
}

/// `hotkey`: an accelerator, `None` to unbind, or "default".
pub fn set(host: &Arc<Host>, id: &str, action: &str, hotkey: Option<String>) -> Result<(), String> {
    let (id, f) = host.feature(id).ok_or("No such feature")?;
    let def = f
        .actions(&host.ctx(id))
        .into_iter()
        .find(|a| a.id == action)
        .ok_or("No such action")?;
    if !def.hotkeyable {
        return Err(format!("{} can't have a hotkey", def.label));
    }

    let binding = match hotkey.as_deref() {
        None => None,
        Some("default") => Some(None),
        Some(key) => Some(Some(key.to_string())),
    };
    let effective = match &binding {
        None => None,
        Some(None) => def.default_hotkey.map(str::to_string),
        Some(Some(k)) => Some(k.clone()),
    };

    if let Some(key) = &effective {
        let shortcut = parse(key)?;
        for (other_f, other_a, other_key, label) in wanted(host) {
            if (other_f, other_a) != (id, action) && parse(&other_key).is_ok_and(|s| s.id() == shortcut.id()) {
                return Err(format!("{key} is already used for {label}"));
            }
        }
    }

    let previous = host.hotkey_for(id, &def);
    host.save_hotkey(id, action, binding)?;
    let refused = sync(host);
    if let Some(key) = effective.filter(|k| refused.contains(k)) {
        // Put the old binding back rather than leaving the action with a dead hotkey
        host.save_hotkey(id, action, Some(previous))?;
        sync(host);
        host.changed();
        return Err(format!("Windows wouldn't give Switchboard {key}; another app is probably using it"));
    }
    host.changed();
    Ok(())
}
