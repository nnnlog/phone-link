//! The notification server as the app knows it: its D-Bus names, the notifications it shows for the
//! app, and its capabilities.

use std::sync::{Mutex, MutexGuard};

use phonelink_core::sync::MutexExt;

use super::shown::Shown;

pub(super) const NOTIFICATIONS: &str = "org.freedesktop.Notifications";
pub(super) const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";

/// The notification server as the app knows it: what it shows, and what it can do. A server that
/// stops, or that another one replaces, takes both with it.
#[derive(Default)]
pub(super) struct Server {
    pub(super) shown: Mutex<Shown>,
    /// The server's capabilities, read when first needed; the number counts the servers seen, so that
    /// an answer from a server that has since gone is not kept.
    pub(super) capabilities: Mutex<(u64, Option<Vec<String>>)>,
}

impl Server {
    pub(super) fn shown(&self) -> MutexGuard<'_, Shown> {
        self.shown.lock_or_recover()
    }

    /// The server's name changed owner (a server started, stopped or was replaced): what the old one
    /// showed and could do is forgotten.
    pub(super) fn changed(&self) {
        self.shown().forget_all();
        let mut capabilities = self.capabilities.lock_or_recover();
        *capabilities = (capabilities.0 + 1, None);
    }
}
