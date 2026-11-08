//! Pairing new-message notifications with the codes the phone's messages app offers.
//!
//! The message and the phone's notification about it arrive on separate paths, in either order. Each
//! side is kept for a while, in memory only: a posted notification without Copy whose body has a 4 to
//! 10 digit run, and an offer that found no such notification. Whichever comes second completes the
//! pair; the code must be a whole digit run of the notification's body.

use phonelink_core::types::DcgClient;
use phonelink_features::shared::auth_code::{self, MAX_DIGITS, MIN_DIGITS};
use phonelink_features::shared::notifier::{SystemNotification, copy};

/// How long a posted notification or an offer waits for the other side, and how old a message may be for
/// the detector to read it.
pub(crate) const KEEP_MS: i64 = 60_000;

struct Posted {
    phone: DcgClient,
    at_ms: i64,
    notification: SystemNotification,
}

struct Offer {
    phone: DcgClient,
    at_ms: i64,
    code: String,
}

/// The kept posted notifications and offers, oldest first.
#[derive(Default)]
pub(crate) struct CodePairing {
    posted: Vec<Posted>,
    offers: Vec<Offer>,
}

impl CodePairing {
    /// The code of the first kept offer of `phone` that is a whole digit run of `body`; the offer is
    /// used up.
    pub(crate) fn take_offer(&mut self, phone: &DcgClient, body: &str, now_ms: i64) -> Option<String> {
        self.drop_expired(now_ms);

        let i = self.offers.iter().position(|o| &o.phone == phone && auth_code::has_digit_run(body, &o.code))?;
        Some(self.offers.remove(i).code)
    }

    /// Keeps `notification`, just posted for `phone`, when it lacks Copy and its body could hold a code.
    pub(crate) fn keep_posted(&mut self, phone: &DcgClient, notification: SystemNotification, now_ms: i64) {
        self.drop_expired(now_ms);
        if !keepable(&notification) {
            return;
        }

        self.posted.push(Posted { phone: phone.clone(), at_ms: now_ms, notification });
    }

    /// `code` offered for `phone`: the first kept notification whose body has it as a whole digit run,
    /// no longer kept; without one, the offer is kept.
    pub(crate) fn offer(&mut self, phone: &DcgClient, code: &str, now_ms: i64) -> Option<SystemNotification> {
        self.drop_expired(now_ms);

        let found =
            self.posted.iter().position(|p| &p.phone == phone && auth_code::has_digit_run(&p.notification.body, code));
        match found {
            Some(i) => Some(self.posted.remove(i).notification),
            None => {
                self.offers.push(Offer { phone: phone.clone(), at_ms: now_ms, code: code.to_string() });
                None
            }
        }
    }

    /// Stops keeping the notification `id`: the user activated, dismissed or answered it.
    pub(crate) fn forget(&mut self, id: &str) {
        self.posted.retain(|p| p.notification.id != id);
    }

    fn drop_expired(&mut self, now_ms: i64) {
        let fresh = |at_ms: i64| now_ms.saturating_sub(at_ms) <= KEEP_MS;
        self.posted.retain(|p| fresh(p.at_ms));
        self.offers.retain(|o| fresh(o.at_ms));
    }
}

/// Without Copy, and a body with a run of 4 to 10 digits.
pub(crate) fn keepable(notification: &SystemNotification) -> bool {
    !copy::offered(notification)
        && auth_code::digit_runs(&notification.body).any(|run| (MIN_DIGITS..=MAX_DIGITS).contains(&run.len()))
}

#[cfg(test)]
mod tests;
