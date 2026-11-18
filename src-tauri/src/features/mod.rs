//! Every feature and account, in the order they appear in the tray and settings. Adding one: create its
//! folder or file here and add a line to `all`.

use std::sync::Arc;

use crate::core::feature::Feature;

mod discord;
mod xbox_live;
pub mod xbox_captures;
pub mod xbox_mode;
pub mod xbox_presence;

pub fn all() -> Vec<Arc<dyn Feature>> {
    // The Xbox features share a sign-in (the Xbox account) and what's known about the console
    let xbox = Arc::new(xbox_live::Shared::default());
    // Accounts come first so they start before the features that use them: the Xbox account sets
    // the client ID the Xbox features sign in with.
    vec![
        Arc::new(xbox_live::XboxAccount::new(xbox.clone())),
        Arc::new(discord::DiscordAccount::new()),
        Arc::new(xbox_mode::XboxMode::new(xbox.clone())),
        Arc::new(xbox_presence::XboxPresence::new(xbox.clone())),
        Arc::new(xbox_captures::XboxCaptures::new(xbox)),
    ]
}
