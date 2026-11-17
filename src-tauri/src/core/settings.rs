//! %APPDATA%\Switchboard\settings.json: one entry per feature (enabled, settings, hotkey overrides).

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Default, Serialize, Deserialize)]
pub struct SettingsFile {
    #[serde(default)]
    pub features: BTreeMap<String, FeatureEntry>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct FeatureEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Only what the user changed; merged over the feature's defaults when read.
    #[serde(default)]
    pub settings: Map<String, Value>,
    /// Absent: use the default. `null`: unbound. A string: the user's own binding.
    #[serde(default)]
    pub hotkeys: BTreeMap<String, Option<String>>,
}

pub struct Store {
    path: PathBuf,
    pub data: SettingsFile,
}

impl Store {
    pub fn load(path: PathBuf) -> (Self, Option<String>) {
        let (data, problem) = match fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(d) => (d, None),
                Err(e) => {
                    // Keep the broken file rather than overwriting it on the next save
                    let _ = fs::copy(&path, path.with_extension("broken.json"));
                    (SettingsFile::default(), Some(format!("settings.json couldn't be read ({e}), started with defaults")))
                }
            },
            Err(_) => (SettingsFile::default(), None),
        };
        (Self { path, data }, problem)
    }

    pub fn entry(&mut self, id: &str) -> &mut FeatureEntry {
        self.data.features.entry(id.to_string()).or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&self.data).expect("settings serialise"))?;
        fs::rename(tmp, &self.path)
    }
}

/// Defaults with the saved values laid over them, key by key.
pub fn merged(defaults: &Value, saved: &Map<String, Value>) -> Value {
    let mut out = defaults.as_object().cloned().unwrap_or_default();
    for (k, v) in saved {
        out.insert(k.clone(), v.clone());
    }
    Value::Object(out)
}

/// Only the keys that differ from the defaults, so changing a default later reaches existing users.
pub fn diff(defaults: &Value, current: &Map<String, Value>) -> Map<String, Value> {
    let defaults = defaults.as_object();
    current
        .iter()
        .filter(|(k, v)| defaults.and_then(|d| d.get(*k)) != Some(v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}
