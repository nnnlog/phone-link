//! The notifications the server shows for the app, and what each of the server's signals does to them:
//! a response to hand on, and the activation token of a click.

use std::collections::HashMap;

use phonelink_features::shared::notifier::NotificationResponse;

use super::content::{DEFAULT_ACTION, INLINE_REPLY_ACTION};
use super::local::LocalHandler;
use super::watch::ServerSignal;

/// `NotificationClosed` reasons: the notification expired (servers that keep a history still show it),
/// or the user dismissed it.
pub(super) const CLOSED_EXPIRED: u32 = 1;
pub(super) const CLOSED_BY_USER: u32 = 2;

/// A notification the server shows for the app.
pub(super) struct Posted {
    /// The engine's id of the notification, or the app's for one it posted itself.
    pub(super) id: String,
    pub(super) user_info: HashMap<String, String>,
    /// The action that takes the inline reply.
    pub(super) reply_action: Option<String>,
    /// A click, a button or a reply answered it.
    pub(super) answered: bool,
    /// What answers a notification the app posted itself; `None` for the engine's.
    pub(super) local: Option<LocalHandler>,
}

/// A response to hand on, outside the lock.
pub(super) enum Delivery {
    Engine { id: String, user_info: HashMap<String, String>, response: NotificationResponse },
    Local { handler: LocalHandler, action: String },
}

/// What one server signal did: a response to hand on, and the activation token of a click on a
/// notification (which brings a window forward).
#[derive(Default)]
pub(super) struct SignalOutcome {
    pub(super) delivery: Option<Delivery>,
    pub(super) token: Option<String>,
}

/// The notifications the server shows for the app, by the server's id and the engine's.
#[derive(Default)]
pub(super) struct Shown {
    pub(super) by_server_id: HashMap<u32, Posted>,
    pub(super) server_ids: HashMap<String, u32>,
    /// The `ActivationToken` of a shown notification, kept until the signal it precedes arrives.
    pub(super) tokens: HashMap<u32, String>,
}

impl Shown {
    pub(super) fn remove(&mut self, server_id: u32) -> Option<Posted> {
        self.tokens.remove(&server_id);
        let posted = self.by_server_id.remove(&server_id)?;
        if self.server_ids.get(&posted.id) == Some(&server_id) {
            self.server_ids.remove(&posted.id);
        }
        Some(posted)
    }

    /// The server shows `posted` as `server_id`, in place of `replaced` and of the notification it showed
    /// before under the same id.
    pub(super) fn insert(&mut self, server_id: u32, replaced: Option<u32>, posted: Posted) {
        if let Some(previous) = replaced {
            self.remove(previous);
        }
        let previous = self.server_ids.insert(posted.id.clone(), server_id).filter(|previous| *previous != server_id);
        if let Some(previous) = previous {
            self.tokens.remove(&previous);
            self.by_server_id.remove(&previous);
        }
        self.by_server_id.insert(server_id, posted);
    }

    /// The server that showed them is gone: its ids name nothing any more.
    pub(super) fn forget_all(&mut self) {
        *self = Self::default();
    }

    /// Applies one server signal, in the order the server sent them. A click, button or reply answers
    /// its notification, so the `NotificationClosed` that GNOME and Plasma send right after it is no
    /// dismissal. An expired notification stays known: a server with a history still shows it, and the
    /// engine may withdraw or replace it.
    pub(super) fn handle(&mut self, signal: ServerSignal) -> SignalOutcome {
        match signal {
            ServerSignal::ActivationToken { server_id, token } => {
                if self.by_server_id.contains_key(&server_id) {
                    self.tokens.insert(server_id, token);
                }
                SignalOutcome::default()
            }
            ServerSignal::ActionInvoked { server_id, action } => {
                let token = self.tokens.remove(&server_id);
                let Some(posted) = self.by_server_id.get_mut(&server_id) else { return SignalOutcome::default() };
                let clicked = action == DEFAULT_ACTION;
                if let Some(handler) = posted.local.clone() {
                    posted.answered = true;
                    return SignalOutcome {
                        delivery: Some(Delivery::Local { handler, action }),
                        token: token.filter(|_| clicked),
                    };
                }
                let response = match action.as_str() {
                    DEFAULT_ACTION => NotificationResponse::Activated,
                    INLINE_REPLY_ACTION => return SignalOutcome::default(),
                    _ => NotificationResponse::Action { action_id: action },
                };
                posted.answered = true;
                SignalOutcome { delivery: Some(engine_delivery(posted, response)), token: token.filter(|_| clicked) }
            }
            ServerSignal::Replied { server_id, text } => {
                self.tokens.remove(&server_id);
                let Some(posted) = self.by_server_id.get_mut(&server_id) else { return SignalOutcome::default() };
                let Some(action_id) = posted.reply_action.clone().filter(|_| posted.local.is_none()) else {
                    return SignalOutcome::default();
                };
                posted.answered = true;
                SignalOutcome {
                    delivery: Some(engine_delivery(posted, NotificationResponse::Reply { action_id, text })),
                    token: None,
                }
            }
            ServerSignal::Closed { server_id, reason } => {
                self.tokens.remove(&server_id);
                if reason == CLOSED_EXPIRED {
                    return SignalOutcome::default();
                }
                let delivery = self
                    .remove(server_id)
                    .filter(|p| reason == CLOSED_BY_USER && !p.answered && p.local.is_none())
                    .map(|p| Delivery::Engine {
                        id: p.id,
                        user_info: p.user_info,
                        response: NotificationResponse::Dismissed,
                    });
                SignalOutcome { delivery, token: None }
            }
        }
    }
}

fn engine_delivery(posted: &Posted, response: NotificationResponse) -> Delivery {
    Delivery::Engine { id: posted.id.clone(), user_info: posted.user_info.clone(), response }
}
