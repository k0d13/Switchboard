//! The Xbox account: one sign-in that every Xbox feature uses, shown under Accounts in settings.
//! The refresh token is stored once. A custom Microsoft client ID gets a sign-in of its own, since a
//! token only works with the client it was issued to.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use anyhow::bail;
use serde::Deserialize;
use serde_json::{Value, json};

use super::Shared;
use super::api::{self as xbox_live, Msa, Poll, Xbox, XboxError};
use crate::core::feature::{ActionDef, Feature, FeatureInfo, Field, Kind, Prompt, Tone};
use crate::core::host::Ctx;
use crate::core::worker::{StopToken, Worker};

pub const ID: &str = "xbox-account";

/// Secrets group, kept from before the account had a page of its own so existing sign-ins carry over.
const GROUP: &str = "xbox";

pub struct Pending {
    id: u64,
    worker: Worker,
}

fn token_key(client_id: &str) -> String {
    match client_id.trim() {
        "" => "refreshToken".into(),
        id => format!("refreshToken.{id}"),
    }
}

impl Shared {
    /// The Microsoft client ID the features should sign in with.
    pub fn client_id(&self) -> String {
        self.client_id.lock().clone()
    }

    pub fn token(&self, ctx: &Ctx, client_id: &str) -> Option<String> {
        ctx.shared_secret(GROUP, &token_key(client_id))
    }

    /// Saves a token from a sign-in or a rotation. Doesn't count as the sign-in changing.
    pub fn save_token(&self, ctx: &Ctx, client_id: &str, token: Option<&str>) {
        ctx.set_shared_secret(GROUP, &token_key(client_id), token);
    }

    pub fn auth_changed(&self) {
        *self.gamertag.lock() = None;
        self.auth_gen.fetch_add(1, Ordering::SeqCst);
        self.nudge();
    }

    pub fn sign_in_pending(&self) -> bool {
        self.sign_in.lock().is_some()
    }

    /// Xbox Live rejected the saved sign-in. Forgets it and tells the user once, however many
    /// features notice.
    pub fn expire(&self, ctx: &Ctx, client_id: &str) {
        self.save_token(ctx, client_id, None);
        *self.gamertag.lock() = None;
        if !self.expired.swap(true, Ordering::SeqCst) {
            ctx.warn("Xbox sign-in expired, sign in again");
            ctx.notify("Xbox", "Your Xbox sign-in expired. Sign in again under Accounts in Switchboard's settings.");
        }
        self.show_status();
        ctx.refresh();
    }

    fn account_ctx(&self) -> Option<Ctx> {
        self.account.lock().clone()
    }

    /// Puts the sign-in state on the account. Takes every value out before calling the host,
    /// which asks `actions()` for the current state and takes the same locks.
    fn show_status(&self) {
        let Some(ctx) = self.account_ctx() else { return };
        let client_id = self.client_id();
        let gamertag = self.gamertag.lock().clone();
        let (text, tone) = if self.sign_in_pending() {
            ("Waiting for you to sign in".to_string(), Tone::Idle)
        } else if self.token(&ctx, &client_id).is_some() {
            match gamertag {
                Some(tag) => (format!("Signed in as {tag}"), Tone::Ok),
                None => ("Signed in".to_string(), Tone::Ok),
            }
        } else if self.expired.load(Ordering::SeqCst) {
            ("Sign-in expired".to_string(), Tone::Warn)
        } else {
            ("Not signed in".to_string(), Tone::Idle)
        };
        ctx.set_status(text, tone);
    }

    /// Asks Xbox Live who's signed in, on a thread of its own. Also notices an expired sign-in
    /// before any feature needs it.
    fn look_up_gamertag(self: &Arc<Self>) {
        let Some(ctx) = self.account_ctx() else { return };
        let client_id = self.client_id();
        let Some(token) = self.token(&ctx, &client_id) else {
            self.show_status();
            return;
        };
        let shared = self.clone();
        std::thread::spawn(move || {
            let generation = shared.auth_gen.load(Ordering::SeqCst);
            let mut xbox = Xbox::new(&client_id);
            xbox.set_refresh_token(Some(token));
            let result = xbox.xsts();
            // The user signed out or in meanwhile, so this answer is about someone else
            if shared.auth_gen.load(Ordering::SeqCst) != generation {
                return;
            }
            if let Some(rotated) = xbox.take_rotated() {
                shared.save_token(&ctx, &client_id, Some(&rotated));
            }
            match result {
                Ok(auth) => {
                    ctx.info(format!("Signed in to Xbox as {}", auth.gamertag));
                    *shared.gamertag.lock() = Some(auth.gamertag);
                    shared.show_status();
                }
                Err(XboxError::SignInExpired) => shared.expire(&ctx, &client_id),
                Err(XboxError::Failed(m)) => ctx.warn(format!("Couldn't look up the Xbox account: {m}")),
            }
        });
    }

    fn set_client_id(self: &Arc<Self>, client_id: &str) {
        let client_id = client_id.trim().to_string();
        {
            let mut current = self.client_id.lock();
            if *current == client_id {
                return;
            }
            *current = client_id;
        }
        self.expired.store(false, Ordering::SeqCst);
        self.auth_changed();
        self.look_up_gamertag();
    }

    /// Sign in, Sign out or Cancel sign-in, whichever fits.
    fn account_action(&self, ctx: &Ctx) -> ActionDef {
        if self.token(ctx, &self.client_id()).is_some() {
            ActionDef::new("sign-out", "Sign out").icon("log-out").no_hotkey().no_flyout()
        } else if self.sign_in_pending() {
            ActionDef::new("cancel-sign-in", "Cancel sign-in").icon("x").no_hotkey().no_flyout()
        } else {
            ActionDef::new("sign-in", "Sign in").icon("log-in").no_hotkey().no_flyout()
        }
    }

    fn sign_out(&self, ctx: &Ctx) {
        self.cancel_sign_in(ctx);
        self.save_token(ctx, &self.client_id(), None);
        self.expired.store(false, Ordering::SeqCst);
        self.auth_changed();
        ctx.info("Signed out of Xbox");
        self.show_status();
        ctx.refresh();
    }

    fn sign_in(self: &Arc<Self>, ctx: &Ctx) -> anyhow::Result<()> {
        let client_id = self.client_id();
        if self.token(ctx, &client_id).is_some() || self.sign_in_pending() {
            return Ok(());
        }
        let msa = Msa::new(&client_id);
        let agent = xbox_live::agent();
        let code = xbox_live::request_device_code(&agent, &msa).map_err(anyhow::Error::msg)?;

        let prompt = Prompt {
            title: "Sign in to Xbox".into(),
            message: "Open the link and enter this code.".into(),
            code: Some(code.user_code.clone()),
            url: Some(code.verification_uri.clone()),
        };
        // Never call into the host (set_prompt, refresh, logging) while holding this lock: the host
        // asks `actions()` for the current state, which takes the same lock.
        {
            let mut slot = self.sign_in.lock();
            if slot.is_some() {
                return Ok(());
            }
            let id = self.sign_in_ids.fetch_add(1, Ordering::SeqCst) + 1;
            let (c, shared) = (ctx.clone(), self.clone());
            let worker = Worker::spawn("xbox-sign-in", move |token| {
                sign_in_worker(c, shared, token, id, client_id, agent, msa, code)
            });
            *slot = Some(Pending { id, worker });
        }
        ctx.set_prompt(Some(prompt));

        self.expired.store(false, Ordering::SeqCst);
        self.nudge();
        self.show_status();
        ctx.refresh();
        Ok(())
    }

    fn cancel_sign_in(&self, ctx: &Ctx) {
        let pending = self.sign_in.lock().take();
        if let Some(p) = pending {
            p.worker.signal();
            ctx.set_prompt(None);
        }
        self.nudge();
        self.show_status();
        ctx.refresh();
    }
}

#[allow(clippy::too_many_arguments)]
fn sign_in_worker(
    ctx: Ctx,
    shared: Arc<Shared>,
    token: StopToken,
    id: u64,
    client_id: String,
    agent: ureq::Agent,
    msa: Msa,
    code: xbox_live::DeviceCode,
) {
    let deadline = Instant::now() + Duration::from_secs(code.expires_in.unwrap_or(900));
    let mut wait = Duration::from_secs(code.interval.unwrap_or(5).max(1));
    let mut transient_failures = 0;

    let outcome: Result<xbox_live::Tokens, String> = loop {
        if token.sleep(wait) {
            return; // cancelled, which already cleared the prompt
        }
        if Instant::now() >= deadline {
            break Err("Sign-in code expired, start again to get a new one".into());
        }
        match xbox_live::poll_device_code(&agent, &msa, &code) {
            Ok(Poll::Pending) => transient_failures = 0,
            Ok(Poll::SlowDown) => wait += Duration::from_secs(5),
            Ok(Poll::Done(tokens)) => break Ok(tokens),
            Ok(Poll::Transient(e)) => {
                transient_failures += 1;
                if transient_failures >= 5 {
                    break Err(e);
                }
            }
            Err(e) => break Err(e),
        }
    };
    if token.stopped() {
        return;
    }

    let outcome = outcome.and_then(|t| t.refresh_token.ok_or_else(|| "Microsoft didn't return a refresh token".to_string()));
    {
        let mut slot = shared.sign_in.lock();
        if slot.as_ref().is_some_and(|p| p.id == id) {
            *slot = None;
        }
    }
    ctx.set_prompt(None);
    match outcome {
        Ok(refresh) => {
            shared.save_token(&ctx, &client_id, Some(&refresh));
            shared.expired.store(false, Ordering::SeqCst);
            shared.auth_changed();
            shared.look_up_gamertag();
        }
        Err(e) => {
            ctx.warn(e.clone());
            ctx.notify("Xbox sign-in", &e);
            shared.nudge();
        }
    }
    shared.show_status();
    ctx.refresh();
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Settings {
    ms_client_id: String,
}

/// The Xbox account's page under Accounts: signing in and out, and which Microsoft client to use.
pub struct XboxAccount {
    shared: Arc<Shared>,
}

impl XboxAccount {
    pub fn new(shared: Arc<Shared>) -> Self {
        Self { shared }
    }
}

impl Feature for XboxAccount {
    fn info(&self) -> FeatureInfo {
        FeatureInfo {
            id: ID,
            name: "Xbox",
            description: "The Microsoft account the Xbox features sign in with",
            icon: "gamepad",
            enabled_by_default: true,
            kind: Kind::Account,
            uses: &[],
        }
    }

    fn defaults(&self) -> Value {
        let d = Settings::default();
        json!({ "msClientId": d.ms_client_id })
    }

    fn schema(&self, _ctx: &Ctx) -> Vec<Field> {
        vec![
            Field::text("msClientId", "Microsoft client ID")
                .placeholder("Xbox app's public client")
                .help("Your own Azure app registration's client ID. Needs its own sign-in.")
                .section("Advanced"),
        ]
    }

    fn actions(&self, ctx: &Ctx) -> Vec<ActionDef> {
        vec![self.shared.account_action(ctx)]
    }

    fn run(&self, ctx: &Ctx, action: &str) -> anyhow::Result<()> {
        match action {
            "sign-in" => self.shared.sign_in(ctx)?,
            "cancel-sign-in" => self.shared.cancel_sign_in(ctx),
            "sign-out" => self.shared.sign_out(ctx),
            _ => bail!("Unknown action {action}"),
        }
        Ok(())
    }

    fn start(&self, ctx: &Ctx) {
        let s: Settings = ctx.settings();
        *self.shared.client_id.lock() = s.ms_client_id.trim().to_string();
        *self.shared.account.lock() = Some(ctx.clone());
        self.shared.look_up_gamertag();
    }

    fn stop(&self, _ctx: &Ctx) {
        let pending = self.shared.sign_in.lock().take();
        if let Some(p) = pending {
            p.worker.signal();
        }
        *self.shared.account.lock() = None;
    }

    fn settings_changed(&self, ctx: &Ctx) {
        let s: Settings = ctx.settings();
        self.shared.set_client_id(&s.ms_client_id);
        ctx.refresh();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_clients_get_their_own_sign_in() {
        assert_eq!(token_key(""), "refreshToken");
        assert_eq!(token_key("  "), "refreshToken");
        assert_eq!(token_key(" abc "), "refreshToken.abc");
    }

    #[test]
    fn defaults_match_settings_default() {
        let shared = Arc::new(Shared::default());
        let from_json: Settings = serde_json::from_value(XboxAccount::new(shared).defaults()).unwrap();
        assert_eq!(from_json, Settings::default());
    }
}
