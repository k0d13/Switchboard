//! What a feature is. Every utility in Switchboard implements `Feature` and lives in its own folder
//! under `features/`, registered with one line in `features/mod.rs`. The host turns its actions into
//! tray items, global hotkeys and `switchboard run <feature>.<action>` commands, and generates its
//! settings page from `schema`.

use serde::Serialize;
use serde_json::Value;

use super::host::Ctx;

pub struct FeatureInfo {
    /// Stable id: keys the settings file, hotkeys and CLI commands. kebab-case.
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// Icon name from the UI's icon set (src/lib/Icon.svelte).
    pub icon: &'static str,
    pub enabled_by_default: bool,
}

#[derive(Clone, Debug)]
pub struct ActionDef {
    pub id: &'static str,
    pub label: String,
    pub icon: Option<&'static str>,
    /// Accelerator such as "Ctrl+Alt+X"; the user can rebind or clear it.
    pub default_hotkey: Option<&'static str>,
    pub hotkeyable: bool,
    /// Shown as the current state (e.g. "Xbox mode" while in Xbox mode).
    pub active: bool,
}

impl ActionDef {
    pub fn new(id: &'static str, label: impl Into<String>) -> Self {
        Self { id, label: label.into(), icon: None, default_hotkey: None, hotkeyable: true, active: false }
    }
    pub fn icon(mut self, icon: &'static str) -> Self {
        self.icon = Some(icon);
        self
    }
    pub fn hotkey(mut self, accelerator: &'static str) -> Self {
        self.default_hotkey = Some(accelerator);
        self
    }
    pub fn no_hotkey(mut self) -> Self {
        self.hotkeyable = false;
        self
    }
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Tone {
    Idle,
    Ok,
    Active,
    Warn,
    Error,
}

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub text: String,
    pub tone: Tone,
}

/// Something the user has to do, shown as a card on the feature (e.g. a sign-in code).
#[derive(Clone, Debug, Serialize)]
pub struct Prompt {
    pub title: String,
    pub message: String,
    pub code: Option<String>,
    pub url: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    Text,
    Number,
    Toggle,
    Select,
    /// A number shown and edited as 0x..
    Hex,
    /// An array of strings, edited one per line.
    List,
}

#[derive(Clone, Debug, Serialize)]
pub struct FieldOption {
    pub value: Value,
    pub label: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub help: Option<String>,
    pub kind: FieldKind,
    pub options: Option<Vec<FieldOption>>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub placeholder: Option<String>,
    pub section: Option<&'static str>,
}

impl Field {
    pub fn new(key: &'static str, label: &'static str, kind: FieldKind) -> Self {
        Self { key, label, help: None, kind, options: None, min: None, max: None, placeholder: None, section: None }
    }
    pub fn text(key: &'static str, label: &'static str) -> Self {
        Self::new(key, label, FieldKind::Text)
    }
    pub fn number(key: &'static str, label: &'static str, min: f64, max: f64) -> Self {
        Self { min: Some(min), max: Some(max), ..Self::new(key, label, FieldKind::Number) }
    }
    pub fn toggle(key: &'static str, label: &'static str) -> Self {
        Self::new(key, label, FieldKind::Toggle)
    }
    pub fn hex(key: &'static str, label: &'static str) -> Self {
        Self { min: Some(0.0), max: Some(255.0), ..Self::new(key, label, FieldKind::Hex) }
    }
    pub fn list(key: &'static str, label: &'static str) -> Self {
        Self::new(key, label, FieldKind::List)
    }
    pub fn select(key: &'static str, label: &'static str, options: Vec<FieldOption>) -> Self {
        Self { options: Some(options), ..Self::new(key, label, FieldKind::Select) }
    }
    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }
    pub fn section(mut self, section: &'static str) -> Self {
        self.section = Some(section);
        self
    }
}

pub fn option(value: impl Into<Value>, label: impl Into<String>) -> FieldOption {
    FieldOption { value: value.into(), label: label.into() }
}

/// A message one feature broadcasts to the others, so they can react without depending on each other.
#[derive(Clone, Debug)]
pub struct Event {
    /// The feature that sent it.
    pub source: &'static str,
    /// e.g. "display-mode-changed"
    pub name: &'static str,
    pub data: Value,
}

pub trait Feature: Send + Sync + 'static {
    fn info(&self) -> FeatureInfo;

    /// Default settings as a JSON object. Saved settings are merged over these key by key, so a
    /// feature's settings struct can use `#[serde(default)]` and new keys just appear.
    fn defaults(&self) -> Value;

    /// Fields for the settings page. Called when the page opens, so it may query hardware.
    fn schema(&self, ctx: &Ctx) -> Vec<Field>;

    /// Called whenever the host rebuilds the tray or UI state, so labels and `active` can change.
    fn actions(&self, ctx: &Ctx) -> Vec<ActionDef>;

    /// Runs on its own thread; the feature shows as busy until it returns. An `Err` is logged and
    /// shown to the user as a notification.
    fn run(&self, ctx: &Ctx, action: &str) -> anyhow::Result<()>;

    /// The feature was enabled (or the app started with it enabled). Must return quickly: start
    /// background work with `core::worker::Worker`.
    fn start(&self, _ctx: &Ctx) {}

    /// The feature was disabled or the app is quitting. Stop background work.
    fn stop(&self, _ctx: &Ctx) {}

    /// The user saved new settings.
    fn settings_changed(&self, _ctx: &Ctx) {}

    /// Another feature broadcast an event with `ctx.broadcast`.
    fn on_event(&self, _ctx: &Ctx, _event: &Event) {}
}
