//! Sending — the send request, composition rules and the send response: pending sends with the
//! persistent never-reused `id` and the `tempId` GUID, retry with the same ids, the target and
//! `subscriptionId` rules, attachments within the carrier limits through the imaging edge,
//! `partNames`, the composer gates, and the outcome's effect on the permission state.
//!
//! The composition rules, which comments in this feature cite by number:
//! 1. Pending send: a persistent pending send `{id, tempId, threadId}` is stored before the request
//!    is built; a retry of a failed message resends the same `id` and `tempId`.
//! 2. Nothing to send: no attachments and empty text send nothing.
//! 3. Target: an existing conversation by `threadId` (with `recipients` only for a Google RCS
//!    conversation on a phone with `Participants`); a new conversation by `recipients` without
//!    `threadId`; a notification inline reply in the notified message's thread.
//! 4. `subscriptionId`: the user's SIM choice for the conversation, else the subscription of the
//!    conversation's last message, else the default-for-SMS subscription — each only when the phone
//!    lists it — else `-1`.
//! 5. Attachments: at most 2 per message, images only (an image-family content type, as given or
//!    detected).
//! 6. Carrier limits of the active subscription (defaults when the phone reports none): images are
//!    scaled and compressed to fit; the composer limits the text length and the participant count.
//! 7. `partNames`: the attachment's name made a valid Windows file name, or a default by content
//!    type, with the content type's extension.
//! 8. `partContentTypes`: the attachment's MIME type, detected from the bytes when unknown.
//! 9. Composer gates: attachments need the subscription's MMS support and the PC setting "send
//!    MMS"; a group conversation needs group MMS; a reply in an RCS group needs RCS support.
//! 10. The PC never states SMS versus MMS versus RCS; the phone decides.

use std::sync::Arc;

use rusqlite::Connection;
use tracing::{debug, info, warn};

use phonelink_app::{AppLayer, AppOutcome, AppRequest};
use phonelink_core::codec::valueset::{Value, ValueSet};
use phonelink_core::types::{DcgClient, new_guid_lower};

use super::caps::Predicates;
use super::consts::{GOOGLE_RCS_SUFFIX, MAX_ATTACHMENTS, NONE_ID, carrier, key, rcs_conversation_type, request_type};
use super::edge::FileFamily;
use super::outbound::chat_request;
use super::parse::SubscriptionRow;
use super::store::{self, PendingSend, SendOrigin, SendStatus, SentPart};
use super::{Inner, Messaging, MessagingError, MessagingEvent};

/// Where a message goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendTarget {
    /// An existing conversation, by the thread id the UI shows it under.
    Conversation { thread_id: i64 },
    /// A new conversation to these addresses, as the user entered or picked them.
    NewConversation { recipients: Vec<String> },
}

/// An attachment as the user added it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// The file name, empty for a pasted or dropped image.
    pub name: String,
    /// The MIME type when known; otherwise it is detected from the bytes.
    pub content_type: Option<String>,
    pub bytes: Vec<u8>,
}

/// What a send did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendOutcome {
    /// The pending send's `id`.
    pub send_id: i64,
    pub status: SendStatus,
    pub outcome: AppOutcome,
}

/// Characters invalid in Windows file names: control characters `0x00`–`0x1F` and `" < > | : * ? \ /`.
fn is_invalid_file_name_char(c: char) -> bool {
    (c as u32) < 0x20 || matches!(c, '"' | '<' | '>' | '|' | ':' | '*' | '?' | '\\' | '/')
}

/// Rule 7: the name with invalid characters removed; without a name a base name by the content
/// type's family followed by `-1`; then the content type's extension unless the name already ends
/// with it (case-insensitively).
pub(super) fn part_name(inner: &Inner, name: &str, content_type: &str) -> String {
    let mut out: String = name.chars().filter(|c| !is_invalid_file_name_char(*c)).collect();
    if name.is_empty() {
        let cfg = &inner.config;
        let base = match inner.edges.file_types.family(content_type.to_string()) {
            FileFamily::Image => "image",
            FileFamily::Video if cfg.video_parts_enabled => "video",
            FileFamily::Audio if cfg.audio_parts_enabled => "audio",
            FileFamily::Document if cfg.document_parts_enabled => "document",
            FileFamily::VCard if cfg.vcard_parts_enabled => "contact",
            FileFamily::Text => "text",
            _ => "file",
        };
        out = format!("{base}{NONE_ID}");
    }

    if let Some(ext) = inner.edges.file_types.file_extension(content_type.to_string()).filter(|e| !e.is_empty()) {
        if !out.to_lowercase().ends_with(&ext.to_lowercase()) {
            out.push_str(&ext);
        }
    }

    out
}

/// The carrier limits of the active subscription (defaults for `≤ 0` values or no subscription).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CarrierLimits {
    pub max_image_width: i32,
    pub max_image_height: i32,
    pub max_message_size: i32,
    /// `None`: unlimited.
    pub max_text_length: Option<i32>,
    /// `None`: unlimited (a `recipientLimits` value `≤ 1`).
    pub max_recipients: Option<i32>,
}

impl CarrierLimits {
    pub(super) fn of(sub: Option<&SubscriptionRow>) -> Self {
        let positive = |v: Option<i32>, default: i32| v.filter(|v| *v > 0).unwrap_or(default);
        Self {
            max_image_width: positive(sub.map(|s| s.max_image_width), carrier::MAX_IMAGE_WIDTH),
            max_image_height: positive(sub.map(|s| s.max_image_height), carrier::MAX_IMAGE_HEIGHT),
            max_message_size: positive(sub.map(|s| s.max_message_size), carrier::MAX_MESSAGE_SIZE),
            max_text_length: sub.map(|s| s.max_message_text_length).filter(|v| *v > 0),
            max_recipients: sub.map(|s| s.recipient_limit).filter(|v| *v > 1),
        }
    }
}

/// Rule 4: the user's SIM choice in the conversation, else the subscription of its last message,
/// else the default-for-SMS subscription — each only when that subscription exists — else `-1`.
pub(super) fn active_subscription(
    c: &Connection,
    thread_id: Option<i64>,
    subscriptions: &[SubscriptionRow],
) -> rusqlite::Result<Option<SubscriptionRow>> {
    let exists = |id: i64| subscriptions.iter().find(|s| s.id == id).cloned();
    if let Some(t) = thread_id {
        if let Some(found) = store::conversation_attr(c, t)?.selected_subscription.and_then(exists) {
            return Ok(Some(found));
        }
        if let Some(found) = store::last_message_subscription(c, t)?.and_then(exists) {
            return Ok(Some(found));
        }
    }
    Ok(subscriptions.iter().find(|s| s.is_default_sms).cloned())
}

/// A Google RCS conversation: one of its recipient addresses ends with `@rcs.google.com`.
fn is_google_rcs(recipients: &[String]) -> bool {
    recipients.iter().any(|a| a.to_ascii_lowercase().ends_with(GOOGLE_RCS_SUFFIX))
}

/// `recipients` for an existing conversation: the participants' normalised addresses of a Google
/// RCS conversation on a phone that reports `Participants`, else none.
pub(super) fn existing_conversation_recipients(
    c: &Connection,
    thread_id: i64,
    p: &Predicates,
) -> rusqlite::Result<Vec<String>> {
    let Some(conversation) = store::conversation(c, thread_id)? else { return Ok(Vec::new()) };
    if p.participants() && is_google_rcs(&conversation.recipients) {
        return Ok(store::participants(c, thread_id)?.into_iter().map(|(_, normalized)| normalized).collect());
    }
    Ok(Vec::new())
}

/// What a send's outcome does besides its status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OutcomeEffect {
    /// A send or a retry in the conversation view: the outcome updates the phone messages
    /// permission.
    Permission,
    /// A notification reply: a failure raises "failed to send" while message notifications are on.
    FailureNotification,
    /// A send-message-API send: nothing follows its outcome, as on Windows.
    Nothing,
}

/// The send request of a pending send.
fn send_request(p: &Predicates, send: &PendingSend, recipients: &[String], trigger: &str) -> AppRequest {
    let mut keys = ValueSet::new()
        .with(key::REQUEST_TYPE, request_type::SEND)
        .with(key::ID, send.id)
        .with(key::TEMP_ID, send.temp_id.clone())
        // The subscription id goes out as an Int32.
        .with(key::SUBSCRIPTION_ID, send.subscription_id as i32);

    if send.thread_id != NONE_ID {
        keys.insert(key::THREAD_ID, send.thread_id);
    }
    if !recipients.is_empty() {
        keys.insert(key::RECIPIENTS, recipients.to_vec());
    }
    if !send.body.is_empty() {
        keys.insert(key::BODY, send.body.clone());
    }
    if !send.parts.is_empty() {
        let names: Vec<String> = send.parts.iter().map(|p| p.name.clone()).collect();
        let types: Vec<String> = send.parts.iter().map(|p| p.content_type.clone()).collect();
        let bytes: Vec<u8> = send.parts.iter().flat_map(|p| p.bytes.iter().copied()).collect();
        let counts: Vec<u32> = send.parts.iter().map(|p| u32::try_from(p.bytes.len()).unwrap_or(u32::MAX)).collect();

        keys.insert(key::PART_COUNT, i32::try_from(send.parts.len()).unwrap_or(i32::MAX));
        keys.insert(key::PART_NAMES, names);
        keys.insert(key::PART_CONTENT_TYPES, types);
        keys.insert(key::PART_BYTES, Value::UInt8Array(bytes));
        keys.insert(key::PART_BYTE_COUNTS, counts);
    }

    chat_request(keys, p, true, trigger, "SendMessage")
}

impl Inner {
    /// Rules 5, 6 and 8 off the async runtime: the imaging and file-type edges block (`ImageIO`
    /// re-encodes in a quality loop), so they run on the runtime's blocking pool.
    async fn prepare_attachments(
        self: &Arc<Self>,
        attachments: Vec<Attachment>,
        limits: CarrierLimits,
    ) -> Result<Vec<SentPart>, MessagingError> {
        if attachments.len() > MAX_ATTACHMENTS {
            return Err(MessagingError::TooManyAttachments { max: MAX_ATTACHMENTS });
        }
        if attachments.is_empty() {
            return Ok(Vec::new());
        }

        let inner = Arc::clone(self);
        match self.runtime.spawn_blocking(move || inner.fit_attachments(attachments, limits)).await {
            Ok(parts) => parts,
            Err(source) => Err(MessagingError::EdgeFailed(super::EdgeFailure { edge: "attachment imaging", source })),
        }
    }

    /// Content types (detected when unknown; undetectable ⇒ the send is blocked), each an image (rule
    /// 5: anything else is refused), then each image fitted within the carrier limits with a byte
    /// budget of `ratio × its size`. Blocking.
    fn fit_attachments(
        &self,
        attachments: Vec<Attachment>,
        limits: CarrierLimits,
    ) -> Result<Vec<SentPart>, MessagingError> {
        let mut typed = Vec::with_capacity(attachments.len());
        for a in attachments {
            let content_type = match a.content_type.filter(|t| !t.is_empty()) {
                Some(t) => t,
                None => self
                    .edges
                    .imaging
                    .sniff_content_type(a.bytes.clone())
                    .ok_or(MessagingError::UnknownAttachmentType)?,
            };

            // Rule 5: only images (GIFs included) are accepted; the app's type lookup decides the
            // image family.
            if self.edges.file_types.family(content_type.clone()) != FileFamily::Image {
                return Err(MessagingError::NotAnImage { content_type });
            }
            typed.push((a.name, content_type, a.bytes));
        }

        let total: u64 = typed.iter().map(|(_, _, b)| b.len() as u64).sum();
        let ratio = limits.max_message_size as f32 / total.max(1) as f32;
        let mut parts = Vec::with_capacity(typed.len());
        for (name, content_type, bytes) in typed {
            let budget = (bytes.len() as f32 * ratio) as u64;
            let width = u32::try_from(limits.max_image_width).unwrap_or(0);
            let height = u32::try_from(limits.max_image_height).unwrap_or(0);

            let (bytes, content_type) = match self.edges.imaging.fit_image(bytes.clone(), width, height, budget) {
                Ok(Some(fitted)) => (fitted.bytes, fitted.content_type),
                Ok(None) => (bytes, content_type),
                Err(e) => {
                    // As on Windows, a failed resize sends the original.
                    warn!(error = %e, "attachment not fitted to the carrier limits; sent as it is");
                    (bytes, content_type)
                }
            };

            let name = part_name(self, &name, &content_type);
            parts.push(SentPart { name, content_type, bytes });
        }

        Ok(parts)
    }

    /// Sends a stored pending send and records its outcome; `effect` says what else the outcome does.
    pub(super) async fn send_pending(
        self: &Arc<Self>,
        app: &AppLayer,
        phone: &DcgClient,
        send: &PendingSend,
        trigger: &str,
        effect: OutcomeEffect,
    ) -> Result<SendOutcome, MessagingError> {
        let record = app.phone_record().ok_or(MessagingError::NoPhone)?;
        let p = self.predicates(phone, &record)?;
        let db = self.db(phone)?;

        let recipients = if send.thread_id == NONE_ID || send.origin == SendOrigin::SendMessageApi {
            send.recipients.clone()
        } else {
            db.read(|c| existing_conversation_recipients(c, send.thread_id, &p))?
        };

        let response = app.request(send_request(&p, send, &recipients, trigger)).await?;
        let status = if response.outcome.is_success() {
            SendStatus::Sent
        } else {
            let reason = response.values().and_then(|v| v.i32(key::SEND_MESSAGE_FAILURE_REASON)).unwrap_or(11);
            SendStatus::Failed(reason)
        };

        log_send_status(send.id, &response.outcome, status);
        let still_pending = db.transaction(|tx| store::set_send_status(tx, send.id, status))?;
        if !still_pending {
            debug!(send_id = send.id, "send answered after its ack");
        }

        self.emit(MessagingEvent::SendChanged { phone: phone.clone(), send_id: send.id });
        match effect {
            OutcomeEffect::Permission => self.apply_permission_outcome(app, phone, &response.outcome),
            OutcomeEffect::FailureNotification => {
                if !response.outcome.is_success() {
                    super::notify::post_send_failed(self, phone, send);
                }
            }
            OutcomeEffect::Nothing => {}
        }

        Ok(SendOutcome { send_id: send.id, status, outcome: response.outcome })
    }

    /// Rule 1: stores the pending send (the next counter value, a new GUID) before the request.
    #[expect(clippy::too_many_arguments, reason = "independent inputs of one operation; not yet grouped")]
    fn store_pending(
        &self,
        phone: &DcgClient,
        thread_id: i64,
        origin: SendOrigin,
        body: &str,
        recipients: &[String],
        subscription_id: i64,
        parts: &[SentPart],
    ) -> Result<PendingSend, MessagingError> {
        let temp_id = new_guid_lower();
        let created = self.now_ms();
        let db = self.db(phone)?;

        let id = db.transaction(|tx| {
            store::insert_pending_send(
                tx,
                &temp_id,
                thread_id,
                created,
                origin,
                body,
                recipients,
                subscription_id,
                parts,
            )
        })?;

        let send = db.read(|c| store::pending_send(c, id))?.ok_or(MessagingError::UnknownSend(id))?;
        self.emit(MessagingEvent::SendChanged { phone: phone.clone(), send_id: id });
        Ok(send)
    }

    /// Rule 3, notification inline reply: `threadId` = the notified message's thread (for an RCS
    /// message, `rcs`, its RCS conversation's id), `subscriptionId = -1`, recipients by the
    /// Google-RCS rule, text only.
    pub(super) async fn send_notification_reply(
        self: &Arc<Self>,
        phone: &DcgClient,
        thread_id: i64,
        rcs: bool,
        text: &str,
    ) -> Result<SendOutcome, MessagingError> {
        let app = self.app()?;

        if app.selected_phone().as_ref() != Some(phone) {
            return Err(MessagingError::NoPhone);
        }
        if text.is_empty() {
            return Err(MessagingError::NothingToSend);
        }

        let origin = if rcs { SendOrigin::RcsNotificationReply } else { SendOrigin::NotificationReply };
        let send = self.store_pending(phone, thread_id, origin, text, &[], NONE_ID, &[])?;
        self.send_pending(&app, phone, &send, "ToastInlineReply", OutcomeEffect::FailureNotification).await
    }
}

impl Messaging {
    /// Sends a message from the conversation view (rules 1–10): `text` and up to two `attachments`
    /// to `target`. Waits for the phone's answer; the pending send stays until the phone acks it.
    pub async fn send_message(
        &self,
        target: SendTarget,
        text: &str,
        attachments: Vec<Attachment>,
    ) -> Result<SendOutcome, MessagingError> {
        let inner = &self.inner;
        let (app, phone, record) = inner.selected()?;
        let p = inner.predicates(&phone, &record)?;
        let db = inner.db(&phone)?;
        let state = db.read(store::phone_state)?;
        let subscriptions = db.read(store::subscriptions)?;

        let (thread_id, entered, recipient_count, rcs_group) = match &target {
            SendTarget::Conversation { thread_id } => {
                let view = db.read(|c| super::views::conversation_shape(c, *thread_id))?;
                let Some(shape) = view else { return Err(MessagingError::UnknownConversation(*thread_id)) };
                (*thread_id, Vec::new(), shape.recipient_count, shape.rcs_type)
            }
            SendTarget::NewConversation { recipients } => (NONE_ID, recipients.clone(), recipients.len(), None),
        };

        let thread = (thread_id != NONE_ID).then_some(thread_id);
        let subscription = db.read(|c| active_subscription(c, thread, &subscriptions))?;
        let limits = CarrierLimits::of(subscription.as_ref());

        // Rule 9: the composer gates.
        if recipient_count == 0 {
            return Err(MessagingError::NotAllowed("no participant"));
        }
        if !attachments.is_empty()
            && !(subscription.as_ref().is_none_or(|s| s.is_mms_enabled) && state.settings.send_mms)
        {
            return Err(MessagingError::NotAllowed("attachments are not offered"));
        }
        if recipient_count > 1 && subscription.as_ref().is_some_and(|s| !s.is_group_mms_enabled) {
            return Err(MessagingError::NotAllowed("group MMS is disabled on this SIM"));
        }
        if let Some(t) = rcs_group.filter(|t| {
            *t == rcs_conversation_type::ACTIVE_GROUP_CHAT || *t == rcs_conversation_type::INACTIVE_GROUP_CHAT
        }) {
            // Without a subscription the "unknown" SIM applies, whose RCS send is off.
            let sim_ok = subscription.as_ref().is_some_and(|s| s.is_rcs_supported);
            if !sim_ok || !p.rcs_send() || t == rcs_conversation_type::INACTIVE_GROUP_CHAT {
                return Err(MessagingError::NotAllowed("this RCS group cannot be replied to"));
            }
        }
        // The composer counts UTF-16 code units, as Phone Link on Windows does.
        if limits.max_text_length.is_some_and(|max| text.encode_utf16().count() > max as usize) {
            return Err(MessagingError::NotAllowed("the text is longer than the carrier allows"));
        }
        if limits.max_recipients.is_some_and(|max| recipient_count > max as usize) {
            return Err(MessagingError::NotAllowed("more participants than the carrier allows"));
        }

        // Rules 5, 6, 8, then rule 2.
        let parts = inner.prepare_attachments(attachments, limits).await?;
        if parts.is_empty() && text.is_empty() {
            return Err(MessagingError::NothingToSend);
        }

        let subscription_id = subscription.as_ref().map_or(NONE_ID, |s| s.id);
        let send = inner.store_pending(
            &phone,
            thread_id,
            SendOrigin::Conversation,
            text,
            &entered,
            subscription_id,
            &parts,
        )?;
        inner.send_pending(&app, &phone, &send, "SendDraftMessage", OutcomeEffect::Permission).await
    }

    /// Resends a failed message with the same `id` and `tempId` (rule 1: a retry is for a message
    /// that failed; any other pending send is refused with [`MessagingError::NotFailed`]). The retry
    /// is the conversation view's, whatever sent the message first: its outcome updates the phone
    /// messages permission and a failure raises no "failed to send" notification.
    pub async fn retry_send(&self, send_id: i64) -> Result<SendOutcome, MessagingError> {
        let (app, phone, _) = self.inner.selected()?;
        let db = self.inner.db(&phone)?;
        let send = db.read(|c| store::pending_send(c, send_id))?.ok_or(MessagingError::UnknownSend(send_id))?;

        if !matches!(send.status, SendStatus::Failed(_)) {
            return Err(MessagingError::NotFailed(send_id));
        }

        db.transaction(|tx| store::set_send_status(tx, send_id, SendStatus::Sending))?;
        self.inner.emit(MessagingEvent::SendChanged { phone: phone.clone(), send_id });
        self.inner.send_pending(&app, &phone, &send, "RetrySendingMessage", OutcomeEffect::Permission).await
    }

    /// The send-message API other features use ("reply with a message" of a call): `threadId`
    /// omitted, `recipients = [number]`, `subscriptionId = -1`, text only. `true` when the phone
    /// accepted it; a failure raises no notification, and the pending send offers a retry.
    pub async fn send_text_to(&self, number: &str, text: &str) -> Result<bool, MessagingError> {
        let (app, phone, _) = self.inner.selected()?;
        if text.is_empty() {
            return Err(MessagingError::NothingToSend);
        }

        let recipients = vec![number.to_string()];
        let send =
            self.inner.store_pending(&phone, NONE_ID, SendOrigin::SendMessageApi, text, &recipients, NONE_ID, &[])?;

        let outcome = self.inner.send_pending(&app, &phone, &send, "SendTextMessage", OutcomeEffect::Nothing).await?;
        Ok(outcome.status == SendStatus::Sent)
    }

    /// The pending sends of the selected phone, oldest first.
    pub fn pending_sends(&self) -> Result<Vec<PendingSend>, MessagingError> {
        let (_, phone, _) = self.inner.selected()?;
        Ok(self.inner.db(&phone)?.read(store::pending_sends)?)
    }
}

fn log_send_status(send_id: i64, outcome: &AppOutcome, status: SendStatus) {
    match status {
        SendStatus::Failed(reason) => {
            warn!(send_id, ?outcome, failure_kind = reason, "message send failed; the send is marked failed");
        }
        _ => info!(send_id, "message sent"),
    }
}

#[cfg(test)]
mod tests;
