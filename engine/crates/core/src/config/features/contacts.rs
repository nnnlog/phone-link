//! The options and timers of the `contacts` feature, at their defaults.

use std::time::Duration;

/// The flags, timers and limits of contacts, at their defaults.
///
/// The app has no contacts browser; its prompts — the new-message composer's and Settings ›
/// Messages' — ask for contacts access instead (`/permission` 2048, then `/contacts/v2`) and are
/// offered for every contacts-capable phone ([`Contacts::request_device_permission`]) (Phone Link on
/// Windows asks from its contacts view).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactsConfig {
    /// The messaging birthday banner, and the order in which contacts sharing an address are taken
    /// by the lookup (by id when on, by display name when off). On by default.
    pub birthday_text: bool,
    /// Timeout of `/contacts/v2` and of the contacts `/permission` request: 120 s, one attempt.
    pub request_timeout: Duration,
    /// How long the contacts view waits for the up-to-date state before it stops showing its
    /// syncing indicator: 2 minutes. UI only; no traffic.
    pub up_to_date_wait: Duration,
}

impl Default for ContactsConfig {
    fn default() -> Self {
        Self {
            birthday_text: true,
            request_timeout: Duration::from_secs(120),
            up_to_date_wait: Duration::from_secs(120),
        }
    }
}
