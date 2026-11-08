//! New-message notifications (macOS: a `UserNotifications` notification with a
//! `UNTextInputNotificationAction`) and the responder that sends the notification's inline reply.
//!
//! Per row (evaluated in the item's transaction): `Inbox`, unread on the phone, timestamped within 5
//! minutes of now, in a known conversation that is neither muted nor hidden; an MMS also needs a part
//! and a sender. Per message batch (after the commit): the PC message-notification setting and consent
//! are on, the app window is not focused, and the proximity gate (`0x80`) is not blocking.
//!
//! A notification whose message holds an authentication code offers Copy instead of the reply (differs
//! from Phone Link on Windows): the code is found in the title and body of a message at most a minute
//! old, or offered by the phone's messages app in its own copy action ([`codes`]).

mod codes;

pub(crate) use codes::CodePairing;

use std::collections::HashMap;
use std::sync::Arc;

use phonelink_core::sync::MutexExt;

use rusqlite::Connection;
use tracing::{debug, error, warn};

use phonelink_core::types::DcgClient;
use phonelink_features::shared::auth_code;
use phonelink_features::shared::notifier::{
    NotificationAction, NotificationResponder, NotificationResponse, SystemNotification, TextInput, copy,
};

use super::caps::Predicates;
use super::consts::{FEATURE, NONE_ID, mailbox, rcs_conversation_type};
use super::edge::{FileFamily, ProximityPhone, ProximityState};
use super::parse::{Item, MmsRow, RcsChatRow, RcsFileTransferRow, SmsRow, SubscriptionRow};
use super::store::{self, PendingSend};
use super::{Inner, MessagingEvent};

/// The kind of message a notification is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Kind {
    Sms,
    Mms,
    RcsChat,
    RcsFileTransfer,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Sms => "sms",
            Kind::Mms => "mms",
            Kind::RcsChat => "rcschat",
            Kind::RcsFileTransfer => "rcsfile",
        }
    }
}

/// A message that passed the per-row conditions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Candidate {
    pub kind: Kind,
    pub id: i64,
    /// The message's own thread id (the reply's `threadId`): for an RCS message its RCS
    /// conversation's id.
    pub thread_id: i64,
    /// The thread id the UI shows the conversation under — for an RCS message its RCS
    /// conversation's fallback id. Activating the notification opens it.
    pub shown_thread: i64,
    pub from: String,
    pub body: String,
    /// The message's timestamp, Unix ms.
    pub timestamp_ms: i64,
    /// The country of the message's subscription, for the contact lookup.
    pub region: Option<String>,
    pub image: Option<Vec<u8>>,
    pub can_reply: bool,
}

/// User-info keys handed back with a response.
mod info_key {
    pub(super) const PHONE: &str = "phone";
    /// The thread id the UI shows the conversation under: activating the notification opens it.
    /// Absent when no conversation applies (a send to a new conversation).
    pub(super) const THREAD: &str = "thread";
    /// The reply's `threadId`: the notified message's own thread id. Only with the reply action.
    pub(super) const REPLY_THREAD: &str = "replyThread";
    /// `"1"` when [`REPLY_THREAD`] is an RCS conversation's id (the notification of an RCS message).
    pub(super) const REPLY_RCS: &str = "replyRcs";
}

/// The reply action's id.
const REPLY_ACTION: &str = "reply";

fn recent(now_ms: i64, timestamp_ms: i64, recency_ms: i64) -> bool {
    now_ms.saturating_sub(timestamp_ms) <= recency_ms
}

fn region_of(subscriptions: &[SubscriptionRow], id: i64) -> Option<String> {
    subscriptions.iter().find(|s| s.id == id).map(|s| s.country_iso.clone()).filter(|c| !c.is_empty())
}

/// Not muted or hidden.
fn shown(c: &Connection, thread: i64) -> rusqlite::Result<bool> {
    let a = store::conversation_attr(c, thread)?;
    Ok(!a.muted && !a.hidden)
}

/// The rows of `item` that pass the per-row conditions (called after the item's rows are stored).
pub(super) fn candidates(
    c: &Connection,
    item: &Item,
    now_ms: i64,
    recency_ms: i64,
    p: Predicates,
    families: &dyn Fn(&str) -> FileFamily,
) -> rusqlite::Result<Vec<Candidate>> {
    let scan = Scan { c, subscriptions: store::subscriptions(c)?, now_ms, recency_ms, p, families };
    let mut out = Vec::new();

    if let Some(rows) = &item.sms {
        scan.sms(&rows.upserts, &mut out)?;
    }
    if let Some(rows) = &item.mms {
        scan.mms(&rows.upserts, &mut out)?;
    }
    if let Some(rows) = &item.rcs_chats {
        scan.rcs_chats(&rows.upserts, &mut out)?;
    }
    if let Some(rows) = &item.rcs_file_transfers {
        scan.rcs_file_transfers(&rows.upserts, &mut out)?;
    }

    Ok(out)
}

/// What the per-row conditions of one item read.
struct Scan<'a> {
    c: &'a Connection,
    subscriptions: Vec<SubscriptionRow>,
    now_ms: i64,
    recency_ms: i64,
    p: Predicates,
    families: &'a dyn Fn(&str) -> FileFamily,
}

impl Scan<'_> {
    /// `Inbox`, unread on the phone and recent.
    fn new_inbound(&self, m: i32, read: bool, ts: i64) -> bool {
        m == mailbox::INBOX && !read && recent(self.now_ms, ts, self.recency_ms)
    }

    /// The fallback id of RCS conversation `thread`, and whether a reply may go to it.
    fn rcs_reply(&self, thread: i64) -> rusqlite::Result<Option<(i64, bool)>> {
        Ok(store::rcs_conversation(self.c, thread)?.map(|conv| {
            (
                conv.fallback_id,
                self.p.rcs_send() && conv.conversation_type != rcs_conversation_type::INACTIVE_GROUP_CHAT,
            )
        }))
    }

    fn sms(&self, rows: &[SmsRow], out: &mut Vec<Candidate>) -> rusqlite::Result<()> {
        let c = self.c;
        for r in rows {
            if !self.new_inbound(r.mailbox, r.read, r.timestamp_ms) || store::conversation(c, r.thread_id)?.is_none() {
                continue;
            }

            if shown(c, r.thread_id)? {
                out.push(Candidate {
                    kind: Kind::Sms,
                    id: r.id,
                    thread_id: r.thread_id,
                    shown_thread: r.thread_id,
                    from: r.from_address.clone(),
                    body: r.body.clone(),
                    timestamp_ms: r.timestamp_ms,
                    region: region_of(&self.subscriptions, r.subscription_id),
                    image: None,
                    can_reply: true,
                });
            }
        }
        Ok(())
    }

    fn mms(&self, rows: &[MmsRow], out: &mut Vec<Candidate>) -> rusqlite::Result<()> {
        let (c, families) = (self.c, self.families);
        for r in rows {
            if !self.new_inbound(r.mailbox, r.read, r.timestamp_ms) {
                continue;
            }
            let Some(stored) = store::mms_by_id(c, r.id)? else { continue };
            if stored.parts.is_empty() || stored.row.from_address.is_empty() {
                continue;
            }
            if store::conversation(c, r.thread_id)?.is_none() || !shown(c, r.thread_id)? {
                continue;
            }

            let text: Vec<&str> = stored
                .parts
                .iter()
                .filter(|part| families(&part.content_type) == FileFamily::Text)
                .map(|part| part.text.as_str())
                .collect();
            let image = stored
                .parts
                .iter()
                .rev()
                .find(|part| families(&part.content_type) == FileFamily::Image)
                .and_then(|part| part.bytes.clone());

            out.push(Candidate {
                kind: Kind::Mms,
                id: r.id,
                thread_id: r.thread_id,
                shown_thread: r.thread_id,
                from: stored.row.from_address.clone(),
                body: text.join(" "),
                timestamp_ms: r.timestamp_ms,
                region: region_of(&self.subscriptions, r.subscription_id),
                image,
                can_reply: true,
            });
        }
        Ok(())
    }

    fn rcs_chats(&self, rows: &[RcsChatRow], out: &mut Vec<Candidate>) -> rusqlite::Result<()> {
        for r in rows {
            let m = &r.common;
            if !self.new_inbound(m.mailbox, m.read, m.timestamp_ms) {
                continue;
            }
            let Some((shown_thread, can_reply)) = self.rcs_reply(m.thread_id)? else { continue };
            if shown(self.c, shown_thread)? {
                out.push(Candidate {
                    kind: Kind::RcsChat,
                    id: m.id,
                    thread_id: m.thread_id,
                    shown_thread,
                    from: m.from_address.clone(),
                    body: r.body.clone(),
                    timestamp_ms: m.timestamp_ms,
                    region: region_of(&self.subscriptions, m.subscription_id),
                    image: None,
                    can_reply,
                });
            }
        }
        Ok(())
    }

    fn rcs_file_transfers(&self, rows: &[RcsFileTransferRow], out: &mut Vec<Candidate>) -> rusqlite::Result<()> {
        let families = self.families;
        for r in rows {
            let m = &r.common;
            if !self.new_inbound(m.mailbox, m.read, m.timestamp_ms) {
                continue;
            }
            let Some((shown_thread, can_reply)) = self.rcs_reply(m.thread_id)? else { continue };
            if shown(self.c, shown_thread)? {
                let image = (families(&m.content_type) == FileFamily::Image).then(|| r.bytes.clone()).flatten();
                out.push(Candidate {
                    kind: Kind::RcsFileTransfer,
                    id: m.id,
                    thread_id: m.thread_id,
                    shown_thread,
                    from: m.from_address.clone(),
                    body: r.file_name.clone(),
                    timestamp_ms: m.timestamp_ms,
                    region: region_of(&self.subscriptions, m.subscription_id),
                    image,
                    can_reply,
                });
            }
        }
        Ok(())
    }
}

impl Inner {
    /// The proximity gate: the phone requires a Bluetooth Classic connection for sending and none is
    /// active.
    pub(super) fn proximity_blocking(&self, phone: &DcgClient, p: &Predicates) -> bool {
        if !p.requires_proximity() {
            return false;
        }

        let names = self
            .app
            .upgrade()
            .and_then(|a| a.phone_record())
            .map(|r| [r.name, r.display_name].into_iter().flatten().collect())
            .unwrap_or_default();
        self.edges.proximity.state(ProximityPhone { dcg_client_id: phone.id.clone(), names })
            != ProximityState::Connected
    }

    /// The notification's title: the contact's name, else the address.
    fn display_name(&self, phone: &DcgClient, address: &str, region: Option<&str>) -> String {
        self.contacts
            .as_ref()
            .and_then(|c| c.lookup(phone, address, region))
            .and_then(|m| m.display_name)
            .unwrap_or_else(|| address.to_string())
    }
}

/// What a response hands back: the phone, the conversation to open (`shown_thread`, none when it is
/// `-1`), and for a notification with the reply action the reply's thread and whether it is an RCS
/// conversation's id.
fn user_info(phone: &DcgClient, shown_thread: i64, reply: Option<(i64, bool)>) -> HashMap<String, String> {
    let mut info = HashMap::from([(info_key::PHONE.to_string(), phone.to_string())]);

    if shown_thread != NONE_ID {
        info.insert(info_key::THREAD.to_string(), shown_thread.to_string());
    }
    if let Some((thread, rcs)) = reply {
        info.insert(info_key::REPLY_THREAD.to_string(), thread.to_string());
        if rcs {
            info.insert(info_key::REPLY_RCS.to_string(), "1".to_string());
        }
    }

    info
}

/// Posts the candidates when the batch conditions hold.
pub(super) fn post(inner: &Arc<Inner>, phone: &DcgClient, candidates: Vec<Candidate>, focused: bool) {
    if !batch_allowed(inner, phone, focused) {
        return;
    }

    let now_ms = inner.now_ms();
    for c in candidates {
        post_candidate(inner, phone, c, now_ms);
    }
}

/// The per-batch conditions: the PC setting and consent on, the window not focused, the proximity gate
/// not blocking.
fn batch_allowed(inner: &Inner, phone: &DcgClient, focused: bool) -> bool {
    let state = match inner.db(phone).and_then(|db| db.read(store::phone_state)) {
        Ok(s) => {
            inner.settings_failing.clear();
            s
        }
        Err(e) => {
            if inner.settings_failing.starts() {
                error!(error = %e, "messaging settings not read; message alerts are not shown");
            }
            return false;
        }
    };

    if !state.settings.message_notifications || !state.settings.pc_consent {
        debug!("message notifications not posted; they are off on this Mac");
        return false;
    }

    if focused {
        return false;
    }

    let Some(record) = inner.app.upgrade().and_then(|a| a.phone_record()) else { return false };
    let p = Predicates::new(inner.effective_caps(phone, state.capabilities, &record), inner.contract_version(&record));
    if inner.proximity_blocking(phone, &p) {
        debug!("message notifications not posted; no Bluetooth Classic connection to the phone");
        return false;
    }

    true
}

/// Posts the notification of one candidate: with Copy when its message holds a code, else kept a while
/// for the phone's offer of one.
fn post_candidate(inner: &Inner, phone: &DcgClient, c: Candidate, now_ms: i64) {
    let title = inner.display_name(phone, &c.from, c.region.as_deref());
    let actions = if c.can_reply {
        vec![NotificationAction {
            id: REPLY_ACTION.into(),
            title: "Reply".into(),
            text_input: Some(TextInput { button_title: "Send".into(), placeholder: "Reply".into() }),
        }]
    } else {
        Vec::new()
    };

    let rcs = matches!(c.kind, Kind::RcsChat | Kind::RcsFileTransfer);
    let fresh = now_ms.saturating_sub(c.timestamp_ms) <= codes::KEEP_MS;
    let detected = fresh.then(|| auth_code::find_code(&[title.as_str(), c.body.as_str()])).flatten();
    let mut notification = SystemNotification {
        id: format!("{FEATURE}/{phone}/{}/{}", c.kind.name(), c.id),
        thread_id: Some(format!("{FEATURE}/{phone}/{}", c.shown_thread)),
        title,
        subtitle: None,
        body: c.body,
        alert: true,
        actions,
        user_info: user_info(phone, c.shown_thread, c.can_reply.then_some((c.thread_id, rcs))),
        image: c.image,
    };

    let code = detected.or_else(|| inner.code_pairing.lock_or_panic().take_offer(phone, &notification.body, now_ms));
    if let Some(code) = &code {
        copy::offer(&mut notification, code);
    }

    let kept = codes::keepable(&notification).then(|| notification.clone());
    match inner.notifier.post(notification) {
        Ok(()) => {
            inner.post_failing.clear();
            if let Some(kept) = kept {
                inner.code_pairing.lock_or_panic().keep_posted(phone, kept, now_ms);
            }
        }
        Err(e) => {
            if inner.post_failing.starts() {
                warn!(error = %e, "message notification not posted");
            }
        }
    }
}

/// The phone's messages app offered `code`: a kept notification of `phone` whose body has it is posted
/// again with Copy, silently; without one, the offer waits for the notification.
pub(super) fn code_offered(inner: &Inner, phone: &DcgClient, code: &str) {
    let found = inner.code_pairing.lock_or_panic().offer(phone, code, inner.now_ms());
    let Some(mut notification) = found else { return };

    copy::offer(&mut notification, code);
    notification.alert = false;
    if let Err(e) = inner.notifier.post(notification) {
        warn!(error = %e, "message notification not posted again with Copy");
    }
}

/// The "failed to send" notification of a notification reply or the send-message API, when message
/// notifications are on. It belongs to the conversation the UI shows the send under (for an RCS
/// reply its RCS conversation's fallback id).
pub(super) fn post_send_failed(inner: &Arc<Inner>, phone: &DcgClient, send: &PendingSend) {
    let state = inner.db(phone).and_then(|db| {
        let on = db.read(store::phone_state)?.settings.message_notifications;
        Ok((on, db.read(|c| store::shown_thread(c, send.thread_id, send.origin))?))
    });

    let shown = match state {
        Ok((true, shown)) => shown.unwrap_or(NONE_ID),
        Ok((false, _)) => return,
        Err(e) => {
            error!(error = %e, "messaging store not read; no send-failure notification is posted");
            return;
        }
    };

    let notification = SystemNotification {
        id: format!("{FEATURE}/{phone}/failed/{}", send.id),
        thread_id: (shown != NONE_ID).then(|| format!("{FEATURE}/{phone}/{shown}")),
        title: "Message not sent".into(),
        subtitle: None,
        body: String::new(),
        alert: true,
        actions: Vec::new(),
        user_info: user_info(phone, shown, None),
        image: None,
    };

    if let Err(e) = inner.notifier.post(notification) {
        warn!(error = %e, "send-failure notification not posted");
    }
}

/// Handles responses to `messaging/…` notifications.
pub(super) struct Responder {
    inner: std::sync::Weak<Inner>,
}

impl Responder {
    pub(super) fn new(inner: &Arc<Inner>) -> Self {
        Self { inner: Arc::downgrade(inner) }
    }
}

/// A thread id of `user_info`: `None` when the key is absent, an error when its value is no thread id
/// (this module wrote every value: a broken invariant).
fn thread_of(user_info: &HashMap<String, String>, key: &'static str) -> Result<Option<i64>, BadThreadId> {
    user_info.get(key).map(|t| t.parse::<i64>().map_err(|source| BadThreadId { key, source })).transpose()
}

/// A notification's thread id that does not parse.
#[derive(Debug, thiserror::Error)]
#[error("{key}: {source}")]
struct BadThreadId {
    key: &'static str,
    source: std::num::ParseIntError,
}

impl NotificationResponder for Responder {
    fn on_response(&self, id: &str, user_info: &HashMap<String, String>, response: NotificationResponse) {
        let Some(inner) = self.inner.upgrade() else { return };
        if !matches!(response, NotificationResponse::Action { .. }) {
            inner.code_pairing.lock_or_panic().forget(id);
        }

        let Some(app) = inner.app.upgrade() else { return };
        let Some(phone) = app.selected_phone().filter(|p| user_info.get(info_key::PHONE) == Some(&p.to_string()))
        else {
            debug!(id, "response to a messaging notification of another phone; dropped");
            return;
        };

        match response {
            NotificationResponse::Reply { action_id, text } if action_id == REPLY_ACTION => {
                reply(inner, id, user_info, phone, text);
            }
            NotificationResponse::Activated => open_conversation(&inner, id, user_info, phone),
            other => debug!(id, response_kind = ?other, "messaging notification response ignored"),
        }
    }
}

/// The inline reply: sends `text` to the notified message's thread on the feature's runtime.
fn reply(inner: Arc<Inner>, id: &str, user_info: &HashMap<String, String>, phone: DcgClient, text: String) {
    let thread = match thread_of(user_info, info_key::REPLY_THREAD) {
        Ok(Some(t)) => t,
        Ok(None) => {
            error!(id, "messaging notification reply without its thread; dropped (broken invariant)");
            return;
        }
        Err(e) => {
            error!(id, error = %e, "messaging notification reply with a malformed thread; dropped (broken invariant)");
            return;
        }
    };
    let rcs = user_info.get(info_key::REPLY_RCS).is_some_and(|v| v == "1");

    // The router may call from any thread: the reply runs on the feature's runtime.
    let runtime = inner.runtime.clone();
    runtime.spawn(async move {
        if let Err(e) = inner.send_notification_reply(&phone, thread, rcs, &text).await {
            warn!(error = %e, "messaging notification reply not sent");
        }
    });
}

/// A click on the notification: opens its conversation.
fn open_conversation(inner: &Inner, id: &str, user_info: &HashMap<String, String>, phone: DcgClient) {
    match thread_of(user_info, info_key::THREAD) {
        Ok(Some(thread)) => inner.emit(MessagingEvent::OpenConversation { phone, thread_id: thread }),
        Ok(None) => debug!(id, "messaging notification of no conversation activated"),
        Err(e) => error!(id, error = %e, "messaging notification with a malformed thread; dropped (broken invariant)"),
    }
}

#[cfg(test)]
mod tests;
