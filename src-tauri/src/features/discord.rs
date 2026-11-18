//! The Discord account: nothing to sign in to, since Switchboard talks to the Discord app on this PC
//! directly. It only shows whether Discord is there. Features that use it (Xbox presence) tell it
//! when they connect or disconnect, through events.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{Value, json};

use crate::core::feature::{ActionDef, Event, Feature, FeatureInfo, Field, Kind, Tone};
use crate::core::host::Ctx;
use crate::core::worker::Worker;

const CHECK_EVERY: Duration = Duration::from_secs(10);

#[derive(Default)]
struct State {
    /// Who Discord says we're connected as, while a feature holds a connection.
    username: Option<String>,
    /// Whether a `discord-ipc-N` pipe exists, as of the last check.
    pipe: bool,
}

/// What the account says, given who's connected and whether Discord's pipe is there.
fn status_text(username: Option<&str>, pipe: bool) -> (String, Tone) {
    match (username, pipe) {
        (Some(name), true) => (format!("Connected as {name}"), Tone::Ok),
        (_, true) => ("Discord is running".to_string(), Tone::Idle),
        _ => ("Discord isn't running".to_string(), Tone::Idle),
    }
}

/// Looks for Discord's pipe by listing them. Opening it would use up one of Discord's connections.
fn pipe_exists() -> bool {
    let Ok(entries) = std::fs::read_dir(r"\\.\pipe\") else { return false };
    entries.flatten().any(|e| e.file_name().to_string_lossy().starts_with("discord-ipc-"))
}

/// Takes the values out of the lock first: `set_status` calls back into `actions()`.
fn show(state: &Mutex<State>, ctx: &Ctx) {
    let (text, tone) = {
        let s = state.lock();
        status_text(s.username.as_deref(), s.pipe)
    };
    ctx.set_status(text, tone);
}

pub struct DiscordAccount {
    state: Arc<Mutex<State>>,
    worker: Mutex<Option<Worker>>,
}

impl DiscordAccount {
    pub fn new() -> Self {
        Self { state: Arc::default(), worker: Mutex::new(None) }
    }
}

impl Feature for DiscordAccount {
    fn info(&self) -> FeatureInfo {
        FeatureInfo {
            id: "discord",
            name: "Discord",
            description: "The Discord app on this PC. There's nothing to sign in to, Switchboard talks to the app directly",
            icon: "message-circle",
            enabled_by_default: true,
            kind: Kind::Account,
            uses: &[],
        }
    }

    fn defaults(&self) -> Value {
        json!({})
    }

    fn schema(&self, _ctx: &Ctx) -> Vec<Field> {
        Vec::new()
    }

    fn actions(&self, _ctx: &Ctx) -> Vec<ActionDef> {
        Vec::new()
    }

    fn run(&self, _ctx: &Ctx, action: &str) -> anyhow::Result<()> {
        anyhow::bail!("Unknown action {action}")
    }

    fn start(&self, ctx: &Ctx) {
        let (state, c) = (self.state.clone(), ctx.clone());
        let worker = Worker::spawn("discord-account", move |token| {
            loop {
                {
                    let mut s = state.lock();
                    s.pipe = pipe_exists();
                    if !s.pipe {
                        s.username = None;
                    }
                }
                show(&state, &c);
                if token.sleep(CHECK_EVERY) {
                    break;
                }
            }
        });
        *self.worker.lock() = Some(worker);
    }

    fn stop(&self, _ctx: &Ctx) {
        let worker = self.worker.lock().take();
        if let Some(w) = worker {
            w.stop();
        }
        *self.state.lock() = State::default();
    }

    fn on_event(&self, ctx: &Ctx, event: &Event) {
        if event.source != "xbox-presence" {
            return;
        }
        match event.name {
            "discord-connected" => {
                let name = event.data.get("username").and_then(Value::as_str).map(str::to_string);
                {
                    let mut s = self.state.lock();
                    s.username = name;
                    // It just connected, so the pipe is there even if the next check hasn't seen it
                    s.pipe = true;
                }
                show(&self.state, ctx);
            }
            "discord-disconnected" => {
                self.state.lock().username = None;
                show(&self.state, ctx);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_follows_connection_and_pipe() {
        assert_eq!(status_text(Some("sam"), true), ("Connected as sam".to_string(), Tone::Ok));
        assert_eq!(status_text(None, true), ("Discord is running".to_string(), Tone::Idle));
        assert_eq!(status_text(None, false), ("Discord isn't running".to_string(), Tone::Idle));
        // A remembered name means nothing once the pipe is gone
        assert_eq!(status_text(Some("sam"), false), ("Discord isn't running".to_string(), Tone::Idle));
    }

    #[test]
    fn defaults_are_empty() {
        assert_eq!(DiscordAccount::new().defaults(), json!({}));
    }
}
