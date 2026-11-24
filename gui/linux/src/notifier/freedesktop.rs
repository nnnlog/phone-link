//! The engine's notifier edge (`SystemNotifier`) on the freedesktop.org notification server.

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, OnceLock, Weak};
use std::thread;
use std::time::Duration;

use phonelink_core::sync::MutexExt;
use phonelink_engine::api::engine::Engine;
use phonelink_engine::edge::EdgeError;
use phonelink_features::shared::notifier::{NotificationResponse, SystemNotification, SystemNotifier};
use tracing::{info, warn};
use zbus::blocking::fdo::DBusProxy;
use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::names::BusName;
use zbus::zvariant::Value;

use super::content::{
    APP_ICON, APP_NAME, BODY_MARKUP, INLINE_REPLY_ACTION, SOUND, action_list, alert_hints, base_hints, body_text,
    supports,
};
use super::server::{NOTIFICATIONS, NOTIFICATIONS_PATH, Server};
use super::shown::Posted;
use super::watch::{deliver_all, owner_changes, server_signals, watch};
use crate::dbus::{self, Bus};

/// The longest a call to the notification server may take, so that a server that hangs holds up no
/// notification for long.
const CALL_TIMEOUT: Duration = Duration::from_secs(2);

/// What takes the engine's responses: the engine, or a test.
pub(super) type Respond = Box<dyn Fn(String, HashMap<String, String>, NotificationResponse) + Send>;

/// The freedesktop.org notification server: notifications on the desktop, and what the user does with
/// them for the engine. The server is reached through the session bus when a notification is posted,
/// so one that starts after the app (or restarts) is used.
pub(crate) struct FreedesktopNotifier {
    bus: Connection,
    server: Arc<Server>,
    engine: Arc<OnceLock<Weak<Engine>>>,
}

impl FreedesktopNotifier {
    /// Connects to the session bus and watches the notification server, whether or not it runs yet.
    pub(crate) fn connect() -> zbus::Result<Self> {
        let engine: Arc<OnceLock<Weak<Engine>>> = Arc::default();
        let delivered = engine.clone();
        Self::responding(Box::new(move |id, user_info, response| deliver(&delivered, id, user_info, response)), engine)
    }

    /// Connects as [`connect`](Self::connect) does, handing the engine's responses to `respond`.
    pub(super) fn responding(respond: Respond, engine: Arc<OnceLock<Weak<Engine>>>) -> zbus::Result<Self> {
        let bus = dbus::connect(Bus::Session, Some(CALL_TIMEOUT))?;
        let server = Arc::new(Server::default());

        // Every message the connection receives, in order, made before the match rules so that none is
        // missed: the server's signals and the bus's word of the server's owner changing.
        let messages = MessageIterator::from(&bus);
        let dbus = DBusProxy::new(&bus)?;
        dbus.add_match_rule(server_signals()?)?;
        dbus.add_match_rule(owner_changes()?)?;
        let owner = match dbus.get_name_owner(BusName::try_from(NOTIFICATIONS)?) {
            Ok(owner) => Some(owner),
            Err(zbus::fdo::Error::NameHasNoOwner(_)) => None,
            Err(e) => return Err(e.into()),
        };
        if owner.is_none() {
            info!("no notification server runs yet; one is looked for with each notification");
        }

        let (deliveries, received) = mpsc::channel();
        let watched = server.clone();
        thread::spawn(move || watch(messages, owner, &watched, &deliveries));
        thread::spawn(move || deliver_all(&received, respond.as_ref()));
        Ok(Self { bus, server, engine })
    }

    /// Hands the user's responses to the engine's notifications to `engine` from now on.
    pub(crate) fn deliver_responses_to(&self, engine: &Arc<Engine>) {
        let _ = self.engine.set(Arc::downgrade(engine));
    }

    pub(super) fn proxy(&self) -> zbus::Result<Proxy<'static>> {
        Proxy::new(&self.bus, NOTIFICATIONS, NOTIFICATIONS_PATH, NOTIFICATIONS)
    }

    /// Whether the server says notifications are inhibited (the desktop's do not disturb), read from it
    /// now. A server that does not say, or none, counts as not inhibited.
    pub(crate) fn inhibited(&self) -> bool {
        self.proxy().and_then(|proxy| proxy.get_property::<bool>("Inhibited")).unwrap_or(false)
    }

    /// The running server's capabilities, read from it the first time they are needed; fails when no
    /// server answers.
    pub(super) fn capabilities(&self) -> Result<Vec<String>, EdgeError> {
        let generation = {
            let capabilities = self.server.capabilities.lock_or_recover();
            if let Some(known) = &capabilities.1 {
                return Ok(known.clone());
            }
            capabilities.0
        };

        let read: Vec<String> =
            self.proxy().and_then(|proxy| proxy.call("GetCapabilities", &())).map_err(notifier_err)?;
        let mut capabilities = self.server.capabilities.lock_or_recover();
        if capabilities.0 == generation {
            capabilities.1 = Some(read.clone());
        }
        Ok(read)
    }

    /// Asks the server to show a notification, in place of `replaces`; its id on the server.
    pub(super) fn notify(
        &self,
        replaces: Option<u32>,
        title: &str,
        body: &str,
        actions: Vec<String>,
        hints: HashMap<&str, Value>,
    ) -> Result<u32, EdgeError> {
        self.proxy()
            .and_then(|proxy| {
                proxy.call("Notify", &(APP_NAME, replaces.unwrap_or(0), APP_ICON, title, body, actions, hints, -1i32))
            })
            .map_err(notifier_err)
    }

    pub(super) fn remember(&self, server_id: u32, replaced: Option<u32>, posted: Posted) {
        self.server.shown().insert(server_id, replaced, posted);
    }

    /// The server's id of the shown notification `id`.
    pub(super) fn server_id(&self, id: &str) -> Option<u32> {
        self.server.shown().server_ids.get(id).copied()
    }
}

fn deliver(
    engine: &OnceLock<Weak<Engine>>,
    id: String,
    user_info: HashMap<String, String>,
    response: NotificationResponse,
) {
    let Some(engine) = engine.get().and_then(Weak::upgrade) else { return };
    if let Err(e) = engine.notification_response(id, user_info, response) {
        warn!(error = %e, "a notification response was not delivered");
    }
}

fn notifier_err(e: zbus::Error) -> EdgeError {
    EdgeError::Unexpected { message: format!("notifications: {e}") }
}

impl SystemNotifier for FreedesktopNotifier {
    fn post(&self, notification: SystemNotification) -> Result<(), EdgeError> {
        let capabilities = self.capabilities()?;
        let (actions, reply) = action_list(&notification.actions, supports(&capabilities, INLINE_REPLY_ACTION));
        let mut hints = base_hints();
        hints.extend(alert_hints(notification.alert, supports(&capabilities, SOUND)));
        if let Some(input) = reply.and_then(|r| r.text_input.as_ref()) {
            hints.insert("x-kde-reply-placeholder-text", Value::from(input.placeholder.clone()));
            hints.insert("x-kde-reply-submit-button-text", Value::from(input.button_title.clone()));
        }
        if let Some(path) =
            notification.image.as_deref().and_then(|bytes| crate::images::by_content("notification-images", bytes))
        {
            hints.insert("image-path", Value::from(path));
        }

        let replaces = self.server_id(&notification.id);
        let body =
            body_text(supports(&capabilities, BODY_MARKUP), notification.subtitle.as_deref(), &notification.body);
        let server_id = self.notify(replaces, &notification.title, &body, actions, hints)?;
        let posted = Posted {
            id: notification.id.clone(),
            user_info: notification.user_info,
            reply_action: reply.map(|r| r.id.clone()),
            answered: false,
            local: None,
        };
        self.remember(server_id, replaces, posted);
        Ok(())
    }

    /// Closes each shown one; one the server no longer has (it answers with an error) is forgotten, and
    /// any other failure is logged, so that the rest are still closed.
    fn withdraw(&self, ids: Vec<String>) -> Result<(), EdgeError> {
        let server_ids: Vec<u32> = {
            let shown = self.server.shown();
            ids.iter().filter_map(|id| shown.server_ids.get(id).copied()).collect()
        };
        if server_ids.is_empty() {
            return Ok(());
        }

        let proxy = self.proxy().map_err(notifier_err)?;
        for server_id in server_ids {
            match proxy.call::<_, _, ()>("CloseNotification", &(server_id,)) {
                Ok(()) => {}
                Err(e @ zbus::Error::MethodError(..)) => {
                    self.server.shown().remove(server_id);
                    info!(error = %e, "the notification server no longer shows a withdrawn notification");
                }
                Err(e) => warn!(error = %e, "a notification was not withdrawn"),
            }
        }
        Ok(())
    }

    fn is_authorized(&self) -> bool {
        true
    }
}
