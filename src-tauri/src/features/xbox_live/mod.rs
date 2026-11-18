//! What the Xbox features share: the Xbox Live client, the Xbox account (one sign-in, shown under
//! Accounts in settings), and whether the Xbox is on.
//!
//! When both are on, Xbox mode's watcher tells presence whether the Xbox is on, so presence only
//! polls Xbox Live while it is. On its own, presence polls less often all the time.

mod account;
mod api;

pub use account::{ID as ACCOUNT_ID, XboxAccount};
pub use api::*;

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

use crate::core::host::Ctx;

/// Whether the Xbox is on, as last seen on the network by Xbox mode.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Power {
    /// Xbox mode is off or has no console chosen.
    Unknown = 0,
    Off = 1,
    On = 2,
}

/// What the Xbox features share: the sign-in, and whether the Xbox is on.
#[derive(Default)]
pub struct Shared {
    power: AtomicU8,
    /// Bumped to wake the presence worker early.
    pub nudge: AtomicU64,

    /// The saved sign-in was rejected, as opposed to the user never having signed in.
    pub expired: AtomicBool,
    /// Bumped whenever the refresh token is changed from outside the presence worker.
    pub auth_gen: AtomicU64,
    sign_in_ids: AtomicU64,
    sign_in: Mutex<Option<account::Pending>>,
    /// The Microsoft client ID from the account's settings, blank for the Xbox app's own.
    client_id: Mutex<String>,
    /// Who's signed in, once Xbox Live has said.
    gamertag: Mutex<Option<String>>,
    /// The account's own handle, for showing its status and the sign-in code.
    account: Mutex<Option<Ctx>>,
}

impl Shared {
    pub fn nudge(&self) {
        self.nudge.fetch_add(1, Ordering::SeqCst);
    }

    pub fn power(&self) -> Power {
        match self.power.load(Ordering::SeqCst) {
            1 => Power::Off,
            2 => Power::On,
            _ => Power::Unknown,
        }
    }

    /// How often to poll Xbox Live, if at all. `on` while the Xbox is on and `None` while it's off.
    /// When Xbox mode isn't watching a console there's no telling, so `unattended` applies: the
    /// user's "Check without Xbox mode" choice, `None` meaning don't.
    pub fn poll_every(&self, on: Duration, unattended: Option<Duration>) -> Option<Duration> {
        match self.power() {
            Power::On => Some(on),
            Power::Off => None,
            Power::Unknown => unattended,
        }
    }

    pub fn set_power(&self, power: Power) {
        if self.power.swap(power as u8, Ordering::SeqCst) != power as u8 {
            self.nudge();
        }
    }
}

/// Shown by the features that poll when there's nothing to tell them the Xbox is on.
pub const NEEDS_XBOX_MODE: &str = "Turn on Xbox mode or Check without Xbox mode";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polling_follows_the_power_state() {
        let (on, away) = (Duration::from_secs(5), Some(Duration::from_secs(15)));
        let shared = Shared::default();
        assert_eq!(shared.poll_every(on, away), away);
        assert_eq!(shared.poll_every(on, None), None);
        shared.set_power(Power::On);
        assert_eq!(shared.poll_every(on, None), Some(on));
        shared.set_power(Power::Off);
        assert_eq!(shared.poll_every(on, away), None);
    }
}
