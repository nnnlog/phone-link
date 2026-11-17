//! The shell: the app's own state (foreground, focus, reconnect, quitting), the phone's status and its
//! controls (Do Not Disturb, ringer, ring, hotspot), the phone's wallpaper, the sections the sidebar
//! lists, general settings, the account (its phones and PCs, signing out, this computer's
//! registration), pairing a phone, and the log.
//!
//! `query` names: `settings`, `wallpaper`, `autostart`, `qr`, `logs`, `pairing`, `shell.*`,
//! `account.*`.
//! `perform` names: `app.*`, `settings.*`, `hotspot.*`, `status.*`, `signOut`, `signIn.loadFailed`,
//! `autostart.set`, `shell.*`, `account.*`, `pairing.*`, `logs.*`.
//! `areaEvent` names (area `shell`): `quit`, `showPairing`, `paired`, `hotspotJoin`, `promptWithdrawn`,
//! `showSettings`, `connectCompleted`.

mod account;
mod app;
mod pairing;
mod pairing_view;
mod routing;
mod settings;
mod state;
mod status;
#[cfg(test)]
mod tests;

pub(super) use state::State;
