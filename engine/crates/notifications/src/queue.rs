//! The persistent action queue: actions and replies (operation 5) are stored before sending; one
//! sender per phone takes the oldest unsent entry, drops it unsent once it is older than the expiry
//! (10 minutes), otherwise rebuilds the request with a fresh `correlationVector`, sends it and
//! waits for its outcome before the next — no retry. Unexpired entries left by a previous run are
//! drained when the sender starts (differs from Phone Link on Windows). Entries go with their
//! notification (removal, session change, clear all).

use std::sync::{Arc, Weak};

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use phonelink_app::AppOutcome;
use phonelink_core::types::{DcgClient, random_nonzero_hex};

use super::item::NotificationItem;
use super::outbound::NotificationsRequest;
use super::presentation;
use super::store::{self, QueueEntry, QueueFinal};
use super::wire::{INLINE_REPLY_EXTERNAL_ID, names, operation};
use super::{Inner, NotificationsError, NotificationsEvent};

/// The stored request record — enough to rebuild the request keys (local only; never on the wire).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueuedRequest {
    pub operation: i32,
    pub key: String,
    pub action_index: i32,
    pub is_action_inline_reply: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_reply_message: Option<String>,
    pub yp_trigger_id: String,
    pub yp_scenario_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub importance: Option<String>,
    /// A reply typed into a posted system notification: a non-*Success* final status posts the
    /// failure notice.
    #[serde(default)]
    pub banner_originated: bool,
}

impl QueuedRequest {
    /// Operation 5 for `key`'s action `action_index`; `reply` = the reply text of a reply action.
    pub(crate) fn launch(key: &str, action_index: i32, reply: Option<&str>, trigger: &str) -> Self {
        Self {
            operation: operation::LAUNCH_NOTIFICATION,
            key: key.to_string(),
            action_index,
            is_action_inline_reply: reply.is_some(),
            inline_reply_message: reply.map(str::to_string),
            yp_trigger_id: trigger.to_string(),
            yp_scenario_id: names::SCENARIO_NOTIFICATIONS.to_string(),
            package_name: None,
            importance: None,
            banner_originated: false,
        }
    }
}

impl Inner {
    /// A non-reply action of a list item, queued (trigger `NotificationsFeedAction`).
    pub(crate) fn press_action(self: &Arc<Self>, key: &str, action_index: i32) -> Result<(), NotificationsError> {
        let (_, phone) = self.selected()?;
        let item = self.stored_item(&phone, key)?;

        if !item.actions().iter().any(|a| !a.inline_reply && a.index == action_index) {
            return Err(NotificationsError::UnknownAction);
        }

        let mut request = QueuedRequest::launch(key, action_index, None, names::TRIGGER_FEED_ACTION);
        request.package_name = Some(item.package_name.clone());
        request.importance = item.importance().map(|i| i.to_string());
        self.enqueue(&phone, &request)
    }

    /// A reply from the list's reply box, queued (trigger `NotificationsFeedInlineReply`).
    pub(crate) fn list_reply(
        self: &Arc<Self>,
        key: &str,
        action_index: i32,
        text: &str,
    ) -> Result<(), NotificationsError> {
        let (_, phone) = self.selected()?;
        let item = self.stored_item(&phone, key)?;

        if !item.actions().iter().any(|a| a.inline_reply && a.index == action_index) {
            return Err(NotificationsError::UnknownAction);
        }

        let mut request = QueuedRequest::launch(key, action_index, Some(text), names::TRIGGER_FEED_INLINE_REPLY);
        request.package_name = Some(item.package_name.clone());
        request.importance = item.importance().map(|i| i.to_string());
        self.enqueue(&phone, &request)
    }

    fn stored_item(&self, phone: &DcgClient, key: &str) -> Result<NotificationItem, NotificationsError> {
        let row = store::row_by_key(&*self.db(phone)?, key)?.ok_or(NotificationsError::UnknownNotification)?;
        NotificationItem::parse(&row.json).map_err(|_| NotificationsError::UnknownNotification)
    }

    /// Stores `request` for `phone` and wakes the phone's sender.
    pub(crate) fn enqueue(
        self: &Arc<Self>,
        phone: &DcgClient,
        request: &QueuedRequest,
    ) -> Result<(), NotificationsError> {
        let db = self.db(phone)?;

        let external_id2 = if request.is_action_inline_reply {
            INLINE_REPLY_EXTERNAL_ID.to_string()
        } else {
            request.action_index.to_string()
        };

        let record = serde_json::to_string(request).expect("a queued request record serializes");
        // The request id is the correlation id of the request as first built; the one sent is fresh.
        store::enqueue(&db, &random_nonzero_hex(8), &request.key, &external_id2, &record, self.now_ms())?;

        self.wake_queue(phone);
        Ok(())
    }

    /// Starts `phone`'s sender on first use, or wakes it.
    pub(crate) fn wake_queue(self: &Arc<Self>, phone: &DcgClient) {
        let (notify, start) = self.with_phone(phone, |p| match &p.queue {
            Some(n) => (n.clone(), false),
            None => {
                let n = Arc::new(Notify::new());
                p.queue = Some(n.clone());
                (n, true)
            }
        });

        if start {
            self.runtime.spawn(sender(self.weak_self.clone(), phone.clone(), notify, self.shutdown.clone()));
        } else {
            notify.notify_one();
        }
    }

    /// Sends or drops the oldest unsent entry; `Ok(false)` when there is none or the phone is not
    /// the selected one.
    async fn send_next(self: &Arc<Self>, phone: &DcgClient) -> Result<bool, NotificationsError> {
        let app = self.app()?;
        if !app.selected_phone().is_some_and(|p| p.is_same_client(phone)) {
            return Ok(false);
        }

        let db = self.db(phone)?;
        let Some(entry) = store::next_unsent(&db)? else { return Ok(false) };
        let request: QueuedRequest = match serde_json::from_str(&entry.request) {
            Ok(r) => r,
            Err(e) => {
                error!(error = %e, "queued notification action does not decode; dropped");
                let corrupt = QueueFinal { status: "Corrupt".into(), expired: false };
                store::finish(&db, entry.id, &corrupt, self.now_ms())?;
                return Ok(true);
            }
        };

        let expiry_ms = i64::try_from(self.config.queue_expiry.as_millis()).unwrap_or(i64::MAX);
        if self.now_ms() > entry.time_added_ms.saturating_add(expiry_ms) {
            warn!("queued notification action expired unsent; dropped");
            let expired = QueueFinal { status: "Expired".into(), expired: true };
            if store::finish(&db, entry.id, &expired, self.now_ms())? {
                self.after_final(phone, &entry, &request, None);
            }
            return Ok(true);
        }

        let reply = request.is_action_inline_reply.then(|| request.inline_reply_message.as_deref().unwrap_or_default());
        let app_request = NotificationsRequest::launch_action(&entry.key, request.action_index, reply)
            .app_request(&request.yp_trigger_id, &request.yp_scenario_id);
        let outcome = app.request(app_request).await?.outcome;
        log_action_outcome(request.is_action_inline_reply, &outcome);
        let finished = QueueFinal { status: format!("{outcome:?}"), expired: false };

        // An entry deleted while its request was in flight (its notification was removed) has no
        // final status to report.
        if store::finish(&db, entry.id, &finished, self.now_ms())? {
            self.after_final(phone, &entry, &request, Some(outcome));
        }
        Ok(true)
    }

    fn after_final(&self, phone: &DcgClient, entry: &QueueEntry, request: &QueuedRequest, outcome: Option<AppOutcome>) {
        if request.banner_originated && !outcome.as_ref().is_some_and(AppOutcome::is_success) {
            self.post(presentation::reply_failed_notice(phone, &entry.key, request.package_name.as_deref()));
        }

        self.emit(NotificationsEvent::ActionFinished {
            phone: phone.clone(),
            key: entry.key.clone(),
            reply: request.is_action_inline_reply,
            outcome,
        });
    }
}

/// One phone's sender: drains on every wake while the phone is selected.
async fn sender(weak: Weak<Inner>, phone: DcgClient, notify: Arc<Notify>, shutdown: CancellationToken) {
    loop {
        loop {
            let Some(inner) = weak.upgrade() else { return };
            match inner.send_next(&phone).await {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => {
                    warn!(error = %e, "notification action queue paused until its next wake");
                    break;
                }
            }
        }

        tokio::select! {
            _ = notify.notified() => {}
            _ = shutdown.cancelled() => return,
        }
    }
}

fn log_action_outcome(is_reply: bool, outcome: &AppOutcome) {
    if outcome.is_success() {
        info!(is_reply, "notification action carried out on the phone");
    } else {
        warn!(is_reply, ?outcome, "notification action not carried out on the phone");
    }
}
