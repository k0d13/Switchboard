//! Owns the features and everything they share: settings, runtime status, secrets, log, events.
//! Features talk to it only through `Ctx`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use tauri::{AppHandle, Emitter};
use tauri_plugin_notification::NotificationExt;

use super::feature::{ActionDef, Event, Feature, Field, Kind, Prompt, Status, Tone};
use super::log::{Level, Log, LogLine};
use super::settings::{self, Store};
use crate::platform::secrets::Secrets;

#[derive(Default)]
struct Runtime {
    status: Option<Status>,
    prompt: Option<Prompt>,
    busy: bool,
    started: bool,
}

pub struct Host {
    pub app: AppHandle,
    pub features: Vec<Arc<dyn Feature>>,
    pub log: Log,
    pub data_dir: PathBuf,
    store: Mutex<Store>,
    runtime: Mutex<HashMap<&'static str, Runtime>>,
    secrets: Mutex<Secrets>,
}

impl Host {
    pub fn new(app: AppHandle, features: Vec<Arc<dyn Feature>>) -> Arc<Self> {
        let config_dir = env_dir("APPDATA");
        let data_dir = env_dir("LOCALAPPDATA");
        let log = Log::new(data_dir.join("switchboard.log"));
        let (store, problem) = Store::load(config_dir.join("settings.json"));
        if let Some(p) = problem {
            log.push(Level::Warn, "app", p);
        }
        let secrets = Secrets::load(data_dir.join("secrets.bin"));
        Arc::new(Self {
            app,
            features,
            log,
            data_dir,
            store: Mutex::new(store),
            runtime: Mutex::default(),
            secrets: Mutex::new(secrets),
        })
    }

    pub fn ctx(self: &Arc<Self>, id: &'static str) -> Ctx {
        Ctx { id, host: self.clone() }
    }

    pub fn feature(&self, id: &str) -> Option<(&'static str, Arc<dyn Feature>)> {
        self.features.iter().find(|f| f.info().id == id).map(|f| (f.info().id, f.clone()))
    }

    pub fn enabled(&self, id: &str) -> bool {
        let Some((_, f)) = self.feature(id) else { return false };
        let info = f.info();
        if info.kind == Kind::Account {
            return true;
        }
        let default = info.enabled_by_default;
        self.store.lock().data.features.get(id).and_then(|e| e.enabled).unwrap_or(default)
    }

    pub fn app_log(&self, level: Level, message: impl Into<String>) {
        self.push_log(level, "app", message.into());
    }

    fn push_log(&self, level: Level, source: &str, message: String) {
        let line = self.log.push(level, source, message);
        let _ = self.app.emit("log", line);
    }

    pub fn logs(&self) -> Vec<LogLine> {
        self.log.all()
    }

    // ---- lifecycle ---------------------------------------------------------------------------

    pub fn start_enabled(self: &Arc<Self>) {
        for f in self.features.clone() {
            let id = f.info().id;
            if self.enabled(id) {
                self.start_feature(id, &f);
            }
        }
        self.changed();
    }

    fn start_feature(self: &Arc<Self>, id: &'static str, f: &Arc<dyn Feature>) {
        {
            let mut rt = self.runtime.lock();
            let r = rt.entry(id).or_default();
            if r.started {
                return;
            }
            r.started = true;
        }
        f.start(&self.ctx(id));
    }

    fn stop_feature(self: &Arc<Self>, id: &'static str, f: &Arc<dyn Feature>) {
        {
            let mut rt = self.runtime.lock();
            let r = rt.entry(id).or_default();
            if !r.started {
                return;
            }
            *r = Runtime::default();
        }
        f.stop(&self.ctx(id));
    }

    pub fn shutdown(self: &Arc<Self>) {
        for f in self.features.clone() {
            self.stop_feature(f.info().id, &f);
        }
    }

    pub fn set_enabled(self: &Arc<Self>, id: &str, enabled: bool) -> Result<(), String> {
        let (id, f) = self.feature(id).ok_or("No such feature")?;
        if f.info().kind == Kind::Account {
            return Err("Accounts can't be turned off".into());
        }
        {
            let mut store = self.store.lock();
            store.entry(id).enabled = Some(enabled);
            store.save().map_err(|e| e.to_string())?;
        }
        if enabled {
            self.start_feature(id, &f);
        } else {
            self.stop_feature(id, &f);
        }
        self.app_log(Level::Info, format!("{} {}", f.info().name, if enabled { "enabled" } else { "disabled" }));
        crate::hotkeys::sync(self);
        self.changed();
        Ok(())
    }

    // ---- settings ----------------------------------------------------------------------------

    pub fn settings_value(&self, id: &str) -> Value {
        let Some((_, f)) = self.feature(id) else { return Value::Null };
        let store = self.store.lock();
        let empty = Map::new();
        let saved = store.data.features.get(id).map(|e| &e.settings).unwrap_or(&empty);
        settings::merged(&f.defaults(), saved)
    }

    pub fn set_settings(self: &Arc<Self>, id: &str, value: Value) -> Result<(), String> {
        let (id, f) = self.feature(id).ok_or("No such feature")?;
        let Value::Object(current) = value else { return Err("Settings must be an object".into()) };
        {
            let mut store = self.store.lock();
            store.entry(id).settings = settings::diff(&f.defaults(), &current);
            store.save().map_err(|e| e.to_string())?;
        }
        if self.enabled(id) {
            f.settings_changed(&self.ctx(id));
        }
        self.changed();
        Ok(())
    }

    pub fn schema(self: &Arc<Self>, id: &str) -> Result<Vec<Field>, String> {
        let (id, f) = self.feature(id).ok_or("No such feature")?;
        Ok(f.schema(&self.ctx(id)))
    }

    // ---- hotkeys -----------------------------------------------------------------------------

    /// The binding in effect for an action: the user's override, else the default.
    pub fn hotkey_for(&self, id: &str, action: &ActionDef) -> Option<String> {
        if !action.hotkeyable {
            return None;
        }
        let store = self.store.lock();
        match store.data.features.get(id).and_then(|e| e.hotkeys.get(action.id)) {
            Some(over) => over.clone(),
            None => action.default_hotkey.map(str::to_string),
        }
    }

    /// `None` = unbound, `Some(None)` = back to default.
    pub fn save_hotkey(&self, id: &str, action: &str, binding: Option<Option<String>>) -> Result<(), String> {
        let mut store = self.store.lock();
        let entry = store.entry(id);
        match binding {
            Some(None) => entry.hotkeys.remove(action),
            Some(Some(b)) => entry.hotkeys.insert(action.to_string(), Some(b)),
            None => entry.hotkeys.insert(action.to_string(), None),
        };
        store.save().map_err(|e| e.to_string())
    }

    // ---- actions -----------------------------------------------------------------------------

    /// Runs an action and waits for it. `notify`: show a toast on failure (for hotkeys and the
    /// tray; the UI shows its own errors).
    pub fn run_action(self: &Arc<Self>, id: &str, action: &str, notify: bool) -> Result<(), String> {
        let (id, f) = self.feature(id).ok_or_else(|| format!("No feature called {id}"))?;
        let name = f.info().name;
        if !self.enabled(id) {
            return Err(format!("{name} is turned off"));
        }
        {
            let mut rt = self.runtime.lock();
            let r = rt.entry(id).or_default();
            if r.busy {
                return Err(format!("{name} is still busy"));
            }
            r.busy = true;
        }
        self.changed();

        let ctx = self.ctx(id);
        // A panicking feature shouldn't leave it stuck as busy (release builds abort anyway)
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f.run(&ctx, action)))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("crashed")));

        self.runtime.lock().entry(id).or_default().busy = false;
        self.changed();

        result.map_err(|e| {
            let msg = format!("{e:#}");
            ctx.error(format!("{action} failed: {msg}"));
            if notify {
                ctx.notify(name, &msg);
            }
            msg
        })
    }

    /// Fire and forget, for hotkeys, the tray and the command line.
    pub fn spawn_action(self: &Arc<Self>, id: String, action: String) {
        let host = self.clone();
        std::thread::spawn(move || {
            let _ = host.run_action(&id, &action, true);
        });
    }

    // ---- state for the UI and tray -----------------------------------------------------------

    pub fn state(self: &Arc<Self>) -> AppState {
        let features = self
            .features
            .iter()
            .map(|f| {
                let info = f.info();
                let enabled = self.enabled(info.id);
                let ctx = self.ctx(info.id);
                let actions = f
                    .actions(&ctx)
                    .into_iter()
                    .map(|a| ActionState {
                        hotkey: self.hotkey_for(info.id, &a),
                        id: a.id,
                        label: a.label,
                        icon: a.icon,
                        default_hotkey: a.default_hotkey,
                        hotkeyable: a.hotkeyable,
                        flyout: a.flyout,
                        active: a.active,
                    })
                    .collect();
                let (status, prompt, busy) = {
                    let rt = self.runtime.lock();
                    let r = rt.get(info.id);
                    (
                        r.and_then(|r| r.status.clone()).filter(|_| enabled),
                        r.and_then(|r| r.prompt.clone()).filter(|_| enabled),
                        r.map(|r| r.busy).unwrap_or(false),
                    )
                };
                FeatureState {
                    id: info.id,
                    name: info.name,
                    description: info.description,
                    icon: info.icon,
                    kind: info.kind,
                    uses: info.uses,
                    enabled,
                    status,
                    busy,
                    prompt,
                    actions,
                    settings: self.settings_value(info.id),
                }
            })
            .collect();
        AppState {
            version: env!("CARGO_PKG_VERSION"),
            autostart: crate::platform::autostart::is_enabled(),
            features,
        }
    }

    /// Something changed: tell the UI and rebuild the tray.
    pub fn changed(self: &Arc<Self>) {
        let state = self.state();
        crate::tray::refresh(&self.app, &state);
        let _ = self.app.emit("state", state);
    }
}

fn env_dir(var: &str) -> PathBuf {
    let base = std::env::var_os(var).map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let dir = base.join("Switchboard");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub version: &'static str,
    pub autostart: bool,
    pub features: Vec<FeatureState>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureState {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub icon: &'static str,
    pub kind: Kind,
    pub uses: &'static [&'static str],
    pub enabled: bool,
    pub status: Option<Status>,
    pub busy: bool,
    pub prompt: Option<Prompt>,
    pub actions: Vec<ActionState>,
    pub settings: Value,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionState {
    pub id: &'static str,
    pub label: String,
    pub icon: Option<&'static str>,
    pub hotkey: Option<String>,
    pub default_hotkey: Option<&'static str>,
    pub hotkeyable: bool,
    pub flyout: bool,
    pub active: bool,
}

/// A feature's handle on the host. Cheap to clone; safe to move into background threads.
#[derive(Clone)]
pub struct Ctx {
    id: &'static str,
    host: Arc<Host>,
}

impl Ctx {
    pub fn id(&self) -> &'static str {
        self.id
    }

    pub fn enabled(&self) -> bool {
        self.host.enabled(self.id)
    }

    /// Current settings (defaults merged with saved values). Falls back to `T::default()` and
    /// logs a warning if they don't fit `T`.
    pub fn settings<T: DeserializeOwned + Default>(&self) -> T {
        serde_json::from_value(self.host.settings_value(self.id)).unwrap_or_else(|e| {
            self.warn(format!("settings didn't load ({e}), using defaults"));
            T::default()
        })
    }

    /// Change one setting from inside the feature (e.g. remembering a value). Persists it.
    pub fn set_setting(&self, key: &str, value: Value) {
        let Value::Object(mut current) = self.host.settings_value(self.id) else { return };
        current.insert(key.to_string(), value);
        let Some((_, f)) = self.host.feature(self.id) else { return };
        let mut store = self.host.store.lock();
        store.entry(self.id).settings = settings::diff(&f.defaults(), &current);
        if let Err(e) = store.save() {
            drop(store);
            self.warn(format!("couldn't save settings: {e}"));
        }
    }

    pub fn status(&self) -> Option<Status> {
        self.host.runtime.lock().get(self.id).and_then(|r| r.status.clone())
    }

    pub fn set_status(&self, text: impl Into<String>, tone: Tone) {
        let status = Status { text: text.into(), tone };
        {
            let mut rt = self.host.runtime.lock();
            let r = rt.entry(self.id).or_default();
            if r.status.as_ref().is_some_and(|s| s.text == status.text && s.tone == status.tone) {
                return;
            }
            r.status = Some(status);
        }
        self.host.changed();
    }

    pub fn clear_status(&self) {
        self.host.runtime.lock().entry(self.id).or_default().status = None;
        self.host.changed();
    }

    pub fn set_prompt(&self, prompt: Option<Prompt>) {
        self.host.runtime.lock().entry(self.id).or_default().prompt = prompt;
        self.host.changed();
    }

    /// Labels or `active` flags of actions changed: rebuild the tray and UI.
    pub fn refresh(&self) {
        self.host.changed();
    }

    pub fn info(&self, message: impl Into<String>) {
        self.host.push_log(Level::Info, self.id, message.into());
    }

    pub fn warn(&self, message: impl Into<String>) {
        self.host.push_log(Level::Warn, self.id, message.into());
    }

    pub fn error(&self, message: impl Into<String>) {
        self.host.push_log(Level::Error, self.id, message.into());
    }

    /// A Windows toast.
    pub fn notify(&self, title: &str, body: &str) {
        if let Err(e) = self.host.app.notification().builder().title(title).body(body).show() {
            self.warn(format!("couldn't show notification: {e}"));
        }
    }

    /// DPAPI-encrypted, namespaced by feature id.
    pub fn secret(&self, key: &str) -> Option<String> {
        self.shared_secret(self.id, key)
    }

    pub fn set_secret(&self, key: &str, value: Option<&str>) {
        self.set_shared_secret(self.id, key, value);
    }

    /// A secret several features use, such as one sign-in. Namespaced by `group` instead of the
    /// feature id.
    pub fn shared_secret(&self, group: &str, key: &str) -> Option<String> {
        self.host.secrets.lock().get(&format!("{group}.{key}"))
    }

    pub fn set_shared_secret(&self, group: &str, key: &str, value: Option<&str>) {
        if let Err(e) = self.host.secrets.lock().set(&format!("{group}.{key}"), value) {
            self.error(format!("couldn't save secret: {e}"));
        }
    }

    /// %LOCALAPPDATA%\Switchboard
    pub fn data_dir(&self) -> PathBuf {
        self.host.data_dir.clone()
    }

    /// Runs one of this feature's own actions in the background, exactly as if its hotkey was
    /// pressed: busy guard, logging and a toast if it fails. Safe to call from `on_event`.
    pub fn run_action(&self, action: &str) {
        self.host.spawn_action(self.id.to_string(), action.to_string());
    }

    /// Tell the other enabled features something happened. Runs their handlers on this thread.
    pub fn broadcast(&self, name: &'static str, data: Value) {
        let event = Event { source: self.id, name, data };
        for f in &self.host.features {
            let id = f.info().id;
            if id != self.id && self.host.enabled(id) {
                f.on_event(&self.host.ctx(id), &event);
            }
        }
    }
}
