//! Shows what you're playing on Xbox in your Discord status. One long-lived worker owns the Xbox
//! client and the Discord pipe, and is the only thing that talks to either. Everything else changes
//! the shared flags and nudges it, and it notices within half a second.
//!
//! The sign-in comes from the Xbox account (`xbox_live::account`). Xbox mode tells this feature
//! whether the Xbox is on, so it only polls Xbox Live while it is. The Discord account is told
//! about the connection through `discord-connected` and `discord-disconnected` events.

mod discord;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::bail;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};

use discord::{Activity, Discord, DiscordError};
use super::xbox_live::{self, Game, Power, Shared, Xbox, XboxError};
use crate::core::feature::{ActionDef, Feature, FeatureInfo, Field, Kind, Tone};
use crate::core::host::Ctx;
use crate::core::worker::{StopToken, Worker};

const DEFAULT_DISCORD_CLIENT_ID: &str = "622174530214821906";
const STOP_WAIT: Duration = Duration::from_secs(3);

/// While the Xbox is on.
const INTERVAL: Duration = Duration::from_secs(5);
/// Without Xbox mode there's no telling when it's on, so check all the time, less often (unless
/// the user turned that off).
const NO_CONSOLE_INTERVAL: Duration = Duration::from_secs(15);
const RETRY: Duration = Duration::from_secs(15);
const DISCORD_RECONNECT: Duration = Duration::from_secs(5);
/// Nothing to poll, so only wake for a nudge or stop.
const IDLE: Duration = Duration::from_secs(3600);
/// Discord is only written to when the game changes, so re-send now and then to notice it has gone.
const KEEPALIVE: Duration = Duration::from_secs(60);
const SLICE: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Settings {
    discord_client_id: String,
    ignored_titles: Vec<String>,
    check_without_xbox_mode: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            discord_client_id: String::new(),
            // Home, Store, Xbox app
            ignored_titles: ["750323071", "1022622766", "714681658"].map(String::from).to_vec(),
            check_without_xbox_mode: true,
        }
    }
}

impl Settings {
    fn discord_id(&self) -> String {
        let id = self.discord_client_id.trim();
        if id.is_empty() { DEFAULT_DISCORD_CLIENT_ID.into() } else { id.into() }
    }

    fn ignored(&self) -> Vec<String> {
        self.ignored_titles.iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect()
    }
}

struct MainLoop {
    worker: Worker,
    done: Arc<AtomicBool>,
}

pub struct XboxPresence {
    shared: Arc<Shared>,
    paused: Arc<AtomicBool>,
    main: Mutex<Option<MainLoop>>,
}

impl XboxPresence {
    pub fn new(shared: Arc<Shared>) -> Self {
        Self { shared, paused: Arc::default(), main: Mutex::new(None) }
    }
}

impl Feature for XboxPresence {
    fn info(&self) -> FeatureInfo {
        FeatureInfo {
            id: "xbox-presence",
            name: "Xbox presence",
            description: "Show what you're playing on Xbox in your Discord status",
            icon: "message-circle",
            enabled_by_default: false,
            kind: Kind::Feature,
            uses: &[xbox_live::ACCOUNT_ID, "discord"],
        }
    }

    fn defaults(&self) -> Value {
        let d = Settings::default();
        json!({
            "discordClientId": d.discord_client_id,
            "ignoredTitles": d.ignored_titles,
            "checkWithoutXboxMode": d.check_without_xbox_mode,
        })
    }

    fn schema(&self, _ctx: &Ctx) -> Vec<Field> {
        let mut fields = Vec::new();
        // Once Xbox mode knows whether the Xbox is on, this has no effect
        if self.shared.power() == Power::Unknown {
            fields.push(
                Field::toggle("checkWithoutXboxMode", "Check without Xbox mode")
                    .help("Keep checking Xbox Live now and then, since nothing says when the Xbox is on"),
            );
        }
        fields.extend([
            Field::list("ignoredTitles", "Ignored title IDs")
                .help("Dashboard apps that shouldn't count as playing")
                .section("Advanced"),
            Field::text("discordClientId", "Discord application ID")
                .placeholder("Discord's Xbox app")
                .section("Advanced"),
        ]);
        fields
    }

    fn actions(&self, ctx: &Ctx) -> Vec<ActionDef> {
        let mut actions = Vec::new();
        if self.shared.token(ctx, &self.shared.client_id()).is_some() {
            actions.push(if self.paused.load(Ordering::SeqCst) {
                ActionDef::new("pause", "Resume presence").icon("play").active(true)
            } else {
                ActionDef::new("pause", "Pause presence").icon("pause")
            });
        }
        actions
    }

    fn run(&self, ctx: &Ctx, action: &str) -> anyhow::Result<()> {
        if action == "pause" {
            self.paused.fetch_xor(true, Ordering::SeqCst);
            self.shared.nudge();
            ctx.refresh();
            return Ok(());
        }
        bail!("Unknown action {action}")
    }

    fn start(&self, ctx: &Ctx) {
        let done = Arc::new(AtomicBool::new(false));
        let (c, shared, paused, flag) = (ctx.clone(), self.shared.clone(), self.paused.clone(), done.clone());
        let worker = Worker::spawn("xbox-presence", move |token| {
            Runner::new(c, shared, paused, token).run();
            flag.store(true, Ordering::SeqCst);
        });
        *self.main.lock() = Some(MainLoop { worker, done });
        ctx.refresh();
    }

    fn stop(&self, _ctx: &Ctx) {
        // Taken out first so the lock is released before anything calls back into actions()
        let main = self.main.lock().take();
        let Some(main) = main else { return };
        main.worker.signal();
        // The worker clears Discord and closes the pipe as it exits. It can be stuck in a network
        // call or a Discord read, so don't wait for it beyond a few seconds.
        let deadline = Instant::now() + STOP_WAIT;
        while !main.done.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if main.done.load(Ordering::SeqCst) {
            main.worker.stop();
        }
    }

    fn settings_changed(&self, _ctx: &Ctx) {
        self.shared.nudge();
    }
}

#[derive(Clone, PartialEq)]
struct Shown {
    id: String,
    detail: Option<String>,
}

struct Runner {
    ctx: Ctx,
    shared: Arc<Shared>,
    paused: Arc<AtomicBool>,
    token: StopToken,
    xbox: Xbox,
    client_id: String,
    discord: Option<Discord>,
    discord_client_id: String,
    /// What Discord is showing: `None` when unknown, `Some(None)` when cleared.
    last: Option<Option<Shown>>,
    last_write: Instant,
    started_ms: u64,
    auth_seen: u64,
    nudge_seen: u64,
    last_error: Option<String>,
    /// The status last given to the host: `None` when unknown, `Some(None)` when cleared.
    shown_status: Option<Option<(String, Tone)>>,
}

impl Runner {
    fn new(ctx: Ctx, shared: Arc<Shared>, paused: Arc<AtomicBool>, token: StopToken) -> Self {
        let settings: Settings = ctx.settings();
        let client_id = shared.client_id();
        let auth_seen = shared.auth_gen.load(Ordering::SeqCst);
        let mut xbox = Xbox::new(&client_id);
        xbox.set_refresh_token(shared.token(&ctx, &client_id));
        Self {
            ctx,
            shared,
            paused,
            token,
            xbox,
            client_id,
            discord: None,
            discord_client_id: settings.discord_id(),
            last: None,
            last_write: Instant::now(),
            started_ms: 0,
            auth_seen,
            nudge_seen: 0,
            last_error: None,
            shown_status: None,
        }
    }

    fn run(mut self) {
        while !self.token.stopped() {
            self.nudge_seen = self.shared.nudge.load(Ordering::SeqCst);
            let wait = self.tick();
            if self.sleep(wait) {
                break;
            }
        }
        if let Some(mut discord) = self.discord.take() {
            let _ = discord.set_activity(None);
            self.ctx.broadcast("discord-disconnected", Value::Null);
        }
    }

    /// True if the worker should stop.
    fn sleep(&self, dur: Duration) -> bool {
        let end = Instant::now() + dur;
        loop {
            if self.shared.nudge.load(Ordering::SeqCst) != self.nudge_seen {
                return false;
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            if self.token.sleep(left.min(SLICE)) {
                return true;
            }
        }
    }

    /// Only calls into the host when the status changes: clearing it rebuilds the whole UI.
    fn status(&mut self, part: Option<(&str, Tone)>) {
        let part = part.map(|(t, tone)| (t.to_string(), tone));
        if self.shown_status.as_ref() == Some(&part) {
            return;
        }
        match &part {
            Some((text, tone)) => self.ctx.set_status(text.clone(), *tone),
            None => self.ctx.clear_status(),
        }
        self.shown_status = Some(part);
    }

    /// One pass of the loop. Returns how long to wait before the next.
    fn tick(&mut self) -> Duration {
        let settings: Settings = self.ctx.settings();
        self.apply_settings(&settings);

        let generation = self.shared.auth_gen.load(Ordering::SeqCst);
        if generation != self.auth_seen {
            self.auth_seen = generation;
            self.xbox.set_refresh_token(self.shared.token(&self.ctx, &self.client_id));
        }

        if !self.xbox.signed_in() {
            self.clear_activity();
            if self.shared.sign_in_pending() {
                self.status(Some(("Waiting for you to sign in", Tone::Idle)));
            } else if self.shared.expired.load(Ordering::SeqCst) {
                self.status(Some(("Sign-in expired", Tone::Warn)));
            } else {
                self.status(Some(("Not signed in", Tone::Warn)));
            }
            return IDLE;
        }

        if self.paused.load(Ordering::SeqCst) {
            self.clear_activity();
            self.status(Some(("Presence paused", Tone::Idle)));
            return IDLE;
        }

        // Xbox mode nudges this worker when the Xbox turns on or off
        let unattended = settings.check_without_xbox_mode.then_some(NO_CONSOLE_INTERVAL);
        let Some(interval) = self.shared.poll_every(INTERVAL, unattended) else {
            self.clear_activity();
            match self.shared.power() {
                Power::Off => self.status(None),
                _ => self.status(Some((xbox_live::NEEDS_XBOX_MODE, Tone::Idle))),
            }
            return IDLE;
        };

        let presence = match self.xbox.presence() {
            Ok(p) => p,
            Err(e) => return self.xbox_failed(e),
        };
        self.save_rotated_token();

        if self.discord.is_none() {
            match Discord::connect(&self.discord_client_id) {
                Ok(d) => {
                    self.ctx.info(format!("Connected to Discord as {}", d.username));
                    self.ctx.broadcast("discord-connected", json!({ "username": d.username }));
                    self.discord = Some(d);
                    self.last = None;
                }
                Err(DiscordError::NotRunning) => {
                    self.status(Some(("Discord isn't running", Tone::Warn)));
                    return RETRY;
                }
                Err(e) => {
                    self.report(format!("Couldn't connect to Discord: {e}"));
                    self.status(Some(("Couldn't connect to Discord", Tone::Warn)));
                    return RETRY;
                }
            }
        }

        let game = xbox_live::current_game(&presence, &settings.ignored());

        if !self.sync(game.as_ref()) {
            return DISCORD_RECONNECT;
        }
        self.last_error = None;

        match &game {
            Some(g) => self.status(Some((&format!("Playing {}", g.name), Tone::Active))),
            None => self.status(None),
        }
        interval
    }

    fn apply_settings(&mut self, settings: &Settings) {
        let ms = self.shared.client_id();
        if ms != self.client_id {
            self.xbox = Xbox::new(&ms);
            self.xbox.set_refresh_token(self.shared.token(&self.ctx, &ms));
            self.client_id = ms;
        }
        let discord_id = settings.discord_id();
        if discord_id != self.discord_client_id {
            self.discord_client_id = discord_id;
            self.drop_discord(true);
        }
    }

    /// Puts the game on Discord if it changed (or it has been a while). False if Discord went away.
    fn sync(&mut self, game: Option<&Game>) -> bool {
        let key = game.map(|g| Shown { id: g.id.clone(), detail: g.detail.clone() });
        let changed = self.last.as_ref() != Some(&key);
        if !changed && self.last_write.elapsed() < KEEPALIVE {
            return true;
        }

        let result = match game {
            None => {
                self.started_ms = 0;
                self.discord_mut().set_activity(None)
            }
            Some(g) => {
                let same_game = matches!(&self.last, Some(Some(shown)) if shown.id == g.id);
                if !same_game {
                    self.started_ms = now_ms();
                }
                let (image, warning) = self.xbox.game_image(&g.id);
                if let Some(w) = warning {
                    self.ctx.warn(w);
                }
                let state = g.detail.clone().unwrap_or_else(|| format!("on {}", xbox_live::device_name(&g.device)));
                let activity =
                    Activity { name: &g.name, state: &state, start_ms: self.started_ms, image: image.as_deref() };
                self.discord_mut().set_activity(Some(&activity))
            }
        };

        match result {
            Ok(()) => {
                if changed {
                    match game {
                        None => self.ctx.info("Not playing anything, cleared presence"),
                        Some(g) => match &g.detail {
                            Some(d) => self.ctx.info(format!("Now showing: {} — {d}", g.name)),
                            None => self.ctx.info(format!("Now showing: {}", g.name)),
                        },
                    }
                }
                self.last = Some(key);
                self.last_write = Instant::now();
                true
            }
            Err(DiscordError::Rejected(m)) => {
                self.report(format!("Discord refused the activity: {m}"));
                true
            }
            Err(e) => {
                self.ctx.warn(format!("Discord disconnected, waiting for it to come back ({e})"));
                self.drop_discord(false);
                self.status(Some(("Discord isn't running", Tone::Warn)));
                false
            }
        }
    }

    fn discord_mut(&mut self) -> &mut Discord {
        self.discord.as_mut().expect("Discord is connected before syncing")
    }

    fn clear_activity(&mut self) {
        if self.last == Some(None) || self.discord.is_none() {
            return;
        }
        match self.discord_mut().set_activity(None) {
            Ok(()) => {
                self.last = Some(None);
                self.last_write = Instant::now();
                self.started_ms = 0;
                self.ctx.info("Cleared presence");
            }
            Err(DiscordError::Rejected(_)) => {}
            Err(_) => self.drop_discord(false),
        }
    }

    fn drop_discord(&mut self, clear_first: bool) {
        if let Some(mut discord) = self.discord.take() {
            if clear_first {
                let _ = discord.set_activity(None);
            }
            self.ctx.broadcast("discord-disconnected", Value::Null);
        }
        self.last = None;
    }

    fn xbox_failed(&mut self, e: XboxError) -> Duration {
        match e {
            XboxError::SignInExpired => {
                self.shared.expire(&self.ctx, &self.client_id);
                self.xbox.set_refresh_token(None);
                Duration::ZERO
            }
            XboxError::Failed(m) => {
                self.report(m);
                self.status(Some(("Can't reach Xbox Live, retrying", Tone::Warn)));
                RETRY
            }
        }
    }

    /// Microsoft sometimes replaces the refresh token when it's used. Keep the new one, unless the
    /// user signed out or in while this pass was running.
    fn save_rotated_token(&mut self) {
        if let Some(token) = self.xbox.take_rotated()
            && self.shared.auth_gen.load(Ordering::SeqCst) == self.auth_seen
        {
            self.shared.save_token(&self.ctx, &self.client_id, Some(&token));
        }
    }

    /// Logs a problem unless it's the same one as last time, so a long outage isn't a wall of text.
    fn report(&mut self, message: String) {
        if self.last_error.as_deref() != Some(&message) {
            self.ctx.warn(message.clone());
            self.last_error = Some(message);
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_discord_id_uses_discords_xbox_app() {
        assert_eq!(Settings::default().discord_id(), "622174530214821906");
        let own = Settings { discord_client_id: " 42 ".into(), ..Settings::default() };
        assert_eq!(own.discord_id(), "42");
    }

    #[test]
    fn ignored_titles_drop_blank_lines() {
        let s = Settings { ignored_titles: vec![" 1 ".into(), "".into(), "  ".into()], ..Settings::default() };
        assert_eq!(s.ignored(), vec!["1".to_string()]);
    }

    #[test]
    fn defaults_match_the_settings_struct() {
        let from_json: Settings = serde_json::from_value(XboxPresence::new(Arc::default()).defaults()).unwrap();
        assert_eq!(from_json, Settings::default());
    }
}
