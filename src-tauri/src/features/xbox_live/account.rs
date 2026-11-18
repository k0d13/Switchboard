//! The Xbox Live sign-in both features use. The refresh token is stored once, so signing in from
//! either feature signs in both. A custom Microsoft client ID gets a sign-in of its own, since a
//! token only works with the client it was issued to.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::Shared;
use super::api::{self as xbox_live, Msa, Poll};
use crate::core::feature::{ActionDef, Prompt};
use crate::core::host::Ctx;
use crate::core::worker::{StopToken, Worker};

/// Secrets group, so the token outlives either feature being turned off.
const GROUP: &str = "xbox";

pub struct Pending {
    id: u64,
    /// The feature the sign-in was started from, which shows the code.
    ctx: Ctx,
    worker: Worker,
}

fn token_key(client_id: &str) -> String {
    match client_id.trim() {
        "" => "refreshToken".into(),
        id => format!("refreshToken.{id}"),
    }
}

impl Shared {
    pub fn token(&self, ctx: &Ctx, client_id: &str) -> Option<String> {
        ctx.shared_secret(GROUP, &token_key(client_id))
    }

    /// Saves a token from a sign-in or a rotation. Doesn't count as the sign-in changing.
    pub fn save_token(&self, ctx: &Ctx, client_id: &str, token: Option<&str>) {
        ctx.set_shared_secret(GROUP, &token_key(client_id), token);
    }

    pub fn auth_changed(&self) {
        self.auth_gen.fetch_add(1, Ordering::SeqCst);
        self.nudge();
    }

    pub fn sign_in_pending(&self) -> bool {
        self.sign_in.lock().is_some()
    }

    /// Sign in, Sign out or Cancel sign-in, whichever fits.
    pub fn account_action(&self, ctx: &Ctx, client_id: &str) -> ActionDef {
        if self.token(ctx, client_id).is_some() {
            ActionDef::new("sign-out", "Sign out").icon("log-out").no_hotkey().no_flyout()
        } else if self.sign_in_pending() {
            ActionDef::new("cancel-sign-in", "Cancel sign-in").icon("x").no_hotkey().no_flyout()
        } else {
            ActionDef::new("sign-in", "Sign in").icon("log-in").no_hotkey().no_flyout()
        }
    }

    /// Runs one of the account actions. False if `action` isn't one.
    pub fn run_account_action(self: &Arc<Self>, ctx: &Ctx, client_id: &str, action: &str) -> anyhow::Result<bool> {
        match action {
            "sign-in" => self.sign_in(ctx, client_id)?,
            "cancel-sign-in" => self.cancel_sign_in(ctx),
            "sign-out" => {
                self.cancel_sign_in(ctx);
                self.save_token(ctx, client_id, None);
                self.expired.store(false, Ordering::SeqCst);
                self.auth_changed();
                ctx.info("Signed out of Xbox");
                ctx.refresh();
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn sign_in(self: &Arc<Self>, ctx: &Ctx, client_id: &str) -> anyhow::Result<()> {
        if self.token(ctx, client_id).is_some() || self.sign_in_pending() {
            return Ok(());
        }
        let msa = Msa::new(client_id);
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
            let (c, shared, client_id) = (ctx.clone(), self.clone(), client_id.to_string());
            let worker = Worker::spawn("xbox-sign-in", move |token| {
                sign_in_worker(c, shared, token, id, client_id, agent, msa, code)
            });
            *slot = Some(Pending { id, ctx: ctx.clone(), worker });
        }
        ctx.set_prompt(Some(prompt));

        self.expired.store(false, Ordering::SeqCst);
        self.nudge();
        ctx.refresh();
        Ok(())
    }

    /// Cancels a sign-in in progress, whichever feature started it.
    pub fn cancel_sign_in(&self, ctx: &Ctx) {
        let pending = self.sign_in.lock().take();
        if let Some(p) = pending {
            p.worker.signal();
            p.ctx.set_prompt(None);
        }
        self.nudge();
        ctx.refresh();
    }

    /// Cancels a sign-in only if this feature started it, for when the feature stops.
    pub fn cancel_own_sign_in(&self, ctx: &Ctx) {
        let pending = {
            let mut slot = self.sign_in.lock();
            if slot.as_ref().is_some_and(|p| p.ctx.id() == ctx.id()) { slot.take() } else { None }
        };
        if let Some(p) = pending {
            p.worker.signal();
            ctx.set_prompt(None);
            self.nudge();
        }
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
            ctx.info("Signed in to Xbox");
            shared.auth_changed();
        }
        Err(e) => {
            ctx.warn(e.clone());
            ctx.notify("Xbox sign-in", &e);
            shared.nudge();
        }
    }
    ctx.refresh();
}

#[cfg(test)]
mod tests {
    use super::token_key;

    #[test]
    fn custom_clients_get_their_own_sign_in() {
        assert_eq!(token_key(""), "refreshToken");
        assert_eq!(token_key("  "), "refreshToken");
        assert_eq!(token_key(" abc "), "refreshToken.abc");
    }
}
