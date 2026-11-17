//! The notification server's signals and the changes of its owner, read from one ordered stream, and
//! the responses they make handed on in order.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};

use phonelink_features::shared::notifier::NotificationResponse;
use tracing::{info, warn};
use zbus::blocking::MessageIterator;
use zbus::message::Type as MessageType;
use zbus::names::OwnedUniqueName;
use zbus::{MatchRule, Message};

use super::activation::keep_activation_token;
use super::server::{NOTIFICATIONS, NOTIFICATIONS_PATH, Server};
use super::shown::Delivery;

const BUS: &str = "org.freedesktop.DBus";
const BUS_PATH: &str = "/org/freedesktop/DBus";
const NAME_OWNER_CHANGED: &str = "NameOwnerChanged";

/// One signal of the notification server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ServerSignal {
    ActionInvoked { server_id: u32, action: String },
    Replied { server_id: u32, text: String },
    Closed { server_id: u32, reason: u32 },
    ActivationToken { server_id: u32, token: String },
}

impl ServerSignal {
    /// The signal `member` of the server's interface, with its arguments; `None` for a member the app
    /// does not handle or arguments of another type.
    pub(super) fn parse(member: &str, message: &Message) -> Option<Self> {
        let body = message.body();
        let signal = match member {
            "ActionInvoked" => {
                let (server_id, action) = body.deserialize::<(u32, String)>().ok()?;
                Self::ActionInvoked { server_id, action }
            }
            "NotificationReplied" => {
                let (server_id, text) = body.deserialize::<(u32, String)>().ok()?;
                Self::Replied { server_id, text }
            }
            "NotificationClosed" => {
                let (server_id, reason) = body.deserialize::<(u32, u32)>().ok()?;
                Self::Closed { server_id, reason }
            }
            "ActivationToken" => {
                let (server_id, token) = body.deserialize::<(u32, String)>().ok()?;
                Self::ActivationToken { server_id, token }
            }
            _ => return None,
        };
        Some(signal)
    }
}

/// The match rule of the server's signals: the bus routes those of whichever connection owns the
/// server's name.
pub(super) fn server_signals() -> zbus::Result<MatchRule<'static>> {
    Ok(MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(NOTIFICATIONS)?
        .path(NOTIFICATIONS_PATH)?
        .interface(NOTIFICATIONS)?
        .build())
}

/// The match rule of the server's name changing owner: a server starting, stopping or replaced.
pub(super) fn owner_changes() -> zbus::Result<MatchRule<'static>> {
    Ok(MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(BUS)?
        .path(BUS_PATH)?
        .interface(BUS)?
        .member(NAME_OWNER_CHANGED)?
        .arg(0, NOTIFICATIONS)?
        .build())
}

/// Reads the server's signals and its owner changes from one ordered stream on one thread, so that a
/// response is never overtaken by the `NotificationClosed` that follows it, nor a new server's signals
/// by the change that started it. Signals from anyone but the server's owner are dropped.
pub(super) fn watch(
    messages: MessageIterator,
    mut owner: Option<OwnedUniqueName>,
    server: &Server,
    deliveries: &Sender<Delivery>,
) {
    for message in messages {
        let message = match message {
            Ok(message) => message,
            Err(e) => {
                warn!(error = %e, "a message from the session bus was not read");
                continue;
            }
        };
        if message.message_type() != MessageType::Signal {
            continue;
        }
        let header = message.header();
        let (Some(interface), Some(member)) = (header.interface(), header.member()) else { continue };
        let sender = header.sender().map(|sender| sender.as_str());

        if interface.as_str() == BUS && member.as_str() == NAME_OWNER_CHANGED && sender == Some(BUS) {
            let Ok((name, _, new_owner)) = message.body().deserialize::<(String, String, String)>() else { continue };
            if name != NOTIFICATIONS {
                continue;
            }
            owner = OwnedUniqueName::try_from(new_owner).ok();
            server.changed();
            info!(
                is_running = owner.is_some(),
                "the notification server changed; the notifications it showed are forgotten"
            );
            continue;
        }

        let from_server = interface.as_str() == NOTIFICATIONS
            && header.path().is_some_and(|path| path.as_str() == NOTIFICATIONS_PATH)
            && sender.is_some_and(|sender| owner.as_ref().is_some_and(|owner| owner.as_str() == sender));
        if !from_server {
            continue;
        }
        let Some(signal) = ServerSignal::parse(member.as_str(), &message) else { continue };
        let outcome = server.shown().handle(signal);
        if let Some(token) = outcome.token {
            keep_activation_token(token);
        }
        if let Some(delivery) = outcome.delivery
            && deliveries.send(delivery).is_err()
        {
            warn!("the notification responses stopped; a response was dropped");
        }
    }
    warn!("the session bus connection closed; notification responses no longer arrive");
}

/// Hands each response on, in order, off the thread that reads the bus: an engine feature may post or
/// withdraw notifications while it handles one.
pub(super) fn deliver_all(
    received: &Receiver<Delivery>,
    respond: &(dyn Fn(String, HashMap<String, String>, NotificationResponse) + Send),
) {
    for delivery in received {
        match delivery {
            Delivery::Engine { id, user_info, response } => respond(id, user_info, response),
            Delivery::Local { handler, action } => handler(&action),
        }
    }
}
