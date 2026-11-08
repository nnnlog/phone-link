//! What the user does with the system notifications this feature posts: a reply typed into a
//! notification becomes a queued operation 5 with `inlineReplyMessage` (trigger
//! `NotificationsToastInlineReply`); a click opens a local view and sends nothing; a dismissal
//! sends nothing; the "paused" notice's button turns throttling off for the phone.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use tracing::{debug, error, warn};

use phonelink_core::types::{DcgClient, Environment};
use phonelink_features::shared::notifier::{NotificationResponder, NotificationResponse};

use super::item::NotificationItem;
use super::presentation::info;
use super::queue::QueuedRequest;
use super::store;
use super::wire::names;
use super::{Inner, NotificationsEvent, OpenTarget};

pub(crate) struct Responder {
    inner: Weak<Inner>,
}

impl Responder {
    pub(crate) fn new(inner: &Arc<Inner>) -> Self {
        Self { inner: Arc::downgrade(inner) }
    }
}

impl NotificationResponder for Responder {
    fn on_response(&self, id: &str, user_info: &HashMap<String, String>, response: NotificationResponse) {
        match self.inner.upgrade() {
            Some(inner) => inner.on_response(id, user_info, response),
            None => debug!(id, "notification response dropped; the feature stopped"),
        }
    }
}

/// The phone a notification's response is for, from its user info; `None` (logged) when it has none.
fn response_phone(id: &str, user_info: &HashMap<String, String>) -> Option<DcgClient> {
    let phone_env = user_info.get(info::PHONE_ENV).and_then(|e| Environment::from_name(e));
    let (Some(phone_id), Some(env)) = (user_info.get(info::PHONE_ID), phone_env) else {
        warn!(id, "notification response without its phone dropped");
        return None;
    };
    Some(DcgClient::new(phone_id.clone(), env))
}

impl Inner {
    fn on_response(self: &Arc<Self>, id: &str, user_info: &HashMap<String, String>, response: NotificationResponse) {
        let Some(phone) = response_phone(id, user_info) else {
            return;
        };

        match user_info.get(info::KIND).map(String::as_str) {
            Some(info::KIND_NOTIFICATION) => {
                let Some(key) = user_info.get(info::KEY) else {
                    warn!(id, "phone notification response without its key dropped");
                    return;
                };
                self.on_notification_response(&phone, key, user_info, response);
            }
            Some(info::KIND_PAUSED) => self.on_paused_response(&phone, &response),
            Some(info::KIND_REPLY_FAILED) => {}
            _ => warn!(id, "notification response of an unknown kind dropped"),
        }
    }

    /// The "paused" notice's button turns throttling off for the phone.
    fn on_paused_response(&self, phone: &DcgClient, response: &NotificationResponse) {
        if matches!(response, NotificationResponse::Action { action_id } if action_id == info::DISABLE_THROTTLING_ACTION)
        {
            if let Err(e) = self.set_throttling_enabled(phone, false) {
                error!(error = %e, "notification throttling not turned off; the setting is not stored");
            }
        }
    }

    fn on_notification_response(
        self: &Arc<Self>,
        phone: &DcgClient,
        key: &str,
        user_info: &HashMap<String, String>,
        response: NotificationResponse,
    ) {
        match response {
            NotificationResponse::Reply { action_id, text } => {
                self.queue_inline_reply(phone, key, user_info, &action_id, &text);
            }
            NotificationResponse::Activated => {
                let target = self.open_target(phone, key);
                self.emit(NotificationsEvent::OpenRequested { phone: phone.clone(), key: key.to_string(), target });
            }
            NotificationResponse::Action { .. } => {
                warn!(
                    "button response on a phone notification dropped; its only button, Copy, never reaches the feature"
                );
            }
            NotificationResponse::Dismissed => debug!("phone notification dismissed on the Mac; nothing is sent"),
        }
    }

    /// Queues the operation 5 a reply typed into a phone notification becomes.
    fn queue_inline_reply(
        self: &Arc<Self>,
        phone: &DcgClient,
        key: &str,
        user_info: &HashMap<String, String>,
        action_id: &str,
        text: &str,
    ) {
        let index = action_id.strip_prefix(info::REPLY_ACTION_PREFIX).and_then(|rest| rest.split('/').next());
        let Some(index) = index.and_then(|i| i.parse::<i32>().ok()) else {
            warn!("phone notification reply through an unknown action dropped");
            return;
        };

        let mut request = QueuedRequest::launch(key, index, Some(text), names::TRIGGER_TOAST_INLINE_REPLY);
        request.package_name = user_info.get(info::PACKAGE_NAME).cloned();
        request.importance = user_info.get(info::IMPORTANCE).cloned();
        request.banner_originated = true;

        if let Err(e) = self.enqueue(phone, &request) {
            error!(error = %e, "phone notification reply not queued; it is not sent");
        }
    }

    /// Messaging for class 1, calls for class 3, else the notification view.
    fn open_target(&self, phone: &DcgClient, key: &str) -> OpenTarget {
        let row = match self.db(phone).and_then(|db| store::row_by_key(&db, key)) {
            Ok(row) => row,
            Err(e) => {
                error!(error = %e, "notifications store not read; the notification opens the notification view");
                None
            }
        };

        let item = row.and_then(|row| NotificationItem::parse(&row.json).ok());
        match item {
            Some(i) if i.is_messaging() => OpenTarget::Messaging,
            Some(i) if i.is_phone_call() => OpenTarget::Calls,
            _ => OpenTarget::Notifications,
        }
    }
}
