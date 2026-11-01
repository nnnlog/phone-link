//! What the messages UI shows, read from the store.
//!
//! It covers conversations (the SMS/MMS thread with the RCS conversations merged into it by their
//! fallback id), their messages and parts with the download state of the media download request,
//! pending sends — those to a new conversation with the addresses they went to — what the
//! new-message composer allows, and the up-to-date state of the sync anchors.
//!
//! Names come from the contacts lookup (the address when none).
//!
//! The views leave part bytes in the store: a message lists its parts with whether they have bytes,
//! and [`Messaging::part_bytes`] reads one part's.

use rusqlite::Connection;

use phonelink_core::types::DcgClient;

use super::consts::{NONE_ID, mailbox, rcs_conversation_type, rcs_message_type};
use super::edge::FileFamily;
use super::outbound::PartRef;
use super::parse::SubscriptionRow;
use super::send::{self, CarrierLimits};
use super::store::{self, SendStatus};
use super::subject;
use super::{Inner, Messaging, MessagingError, Predicates};

/// Up to date / syncing, by the sync anchors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncState {
    /// Every stored anchor has `current ≥ expected`.
    pub up_to_date: bool,
    /// The messages UI shows its syncing state.
    pub syncing: bool,
}

/// One address with its contact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipient {
    pub address: String,
    /// The contact's name, when the lookup found one (never empty: an empty name is no name).
    pub name: Option<String>,
    /// The contact the lookup found.
    pub contact_id: Option<i64>,
    /// The contact whose photo shows for the address (read by contact id), when one has a photo.
    pub photo_contact_id: Option<i64>,
}

/// One conversation as the list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationView {
    /// The SMS/MMS thread id, or for a conversation that exists only as RCS its RCS conversation's
    /// fallback id.
    pub thread_id: i64,
    pub recipients: Vec<Recipient>,
    pub is_group: bool,
    /// Unix ms of the latest inbox or sent message.
    pub latest_ms: i64,
    pub pc_unread: i64,
    pub phone_unread: i64,
    pub muted: bool,
    pub hidden: bool,
    /// The merged RCS conversation's `RcsConversationType`, when there is one.
    pub rcs_type: Option<i32>,
    /// The composer may send here: rule 9, the RCS group rule, and no more participants than rule 6
    /// allows (the send refuses the same cases).
    pub can_reply: bool,
    /// Attachments are offered (rule 9).
    pub attachments_allowed: bool,
    /// The subscription a send uses (rule 4), `-1` when none applies.
    pub active_subscription: i64,
    /// Composer limits (rule 6): `None` = unlimited.
    pub max_text_length: Option<i32>,
    pub max_recipients: Option<i32>,
    /// The latest message, for the list's preview line; `None` when the conversation has none.
    pub preview: Option<ConversationPreview>,
}

/// The latest message of a conversation as the list previews it: the message the conversation's
/// `latest_ms` comes from, shown as the conversation view shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationPreview {
    /// Its text (an MMS's text parts joined as its body is); empty when it has only attachments.
    pub body: String,
    /// It carries an attachment: an MMS part other than text, or an RCS file.
    pub attachment: bool,
    /// The sender of an inbox message, with its contact name; `None` for a sent message.
    pub from: Option<Recipient>,
}

/// What a part shows (the media download table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PartState {
    /// An MMS text part: no attachment, its text is part of the message body.
    Hidden,
    /// An image with its bytes.
    Image,
    /// Audio or video with its bytes (their flags on).
    Playable,
    /// A document with its bytes (its flag on).
    Openable,
    /// Offered for download.
    Downloadable,
    /// Not supported: no download.
    Unsupported,
}

/// One MMS part or RCS file (or an attachment of a pending send), its bytes left in the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartView {
    /// What a download names and where [`Messaging::part_bytes`] reads the bytes.
    pub part: PartRef,
    pub content_type: String,
    pub name: String,
    pub size: i64,
    pub state: PartState,
    /// The store holds the part's bytes.
    pub has_bytes: bool,
}

/// The kind of a shown message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageKind {
    Sms,
    Mms,
    RcsChat,
    RcsFileTransfer,
    /// A message sent from the PC that the phone has not acked yet.
    PendingSend,
}

/// One message of a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageView {
    pub kind: MessageKind,
    pub id: i64,
    pub timestamp_ms: i64,
    /// `MessageMailboxType` (`Sent` for a pending send).
    pub mailbox: i32,
    pub from: Recipient,
    pub body: String,
    pub subject: String,
    pub read_on_phone: bool,
    pub read_on_pc: bool,
    pub subscription_id: i64,
    /// For an RCS group event rather than a message: its `rcsMessageTypes` value (1–8).
    pub group_event: Option<i32>,
    pub parts: Vec<PartView>,
    /// The state of a pending send.
    pub send_status: Option<SendStatus>,
}

/// A message sent from the PC to a new conversation (composition rule 3: no `threadId`)
/// that the phone has not acked yet, with the addresses it was sent to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewConversationView {
    /// The addresses as the user entered or picked them, with their contacts.
    pub recipients: Vec<Recipient>,
    pub message: MessageView,
}

/// What the new-message composer allows: rules 6 and 9 for a new conversation, which sends from
/// the default-for-SMS subscription (rule 4 without a conversation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewMessageLimits {
    /// Attachments are offered: that subscription's `isMmsEnableds` (true without one) and the PC
    /// setting "send MMS".
    pub attachments_allowed: bool,
    /// More than one recipient may be sent to: that subscription's `isGroupMmsEnableds` (true
    /// without one).
    pub group_allowed: bool,
    /// `None` = unlimited.
    pub max_text_length: Option<i32>,
    /// `None` = unlimited.
    pub max_recipients: Option<i32>,
    /// The subscription the send uses, `-1` when none applies.
    pub active_subscription: i64,
}

/// The download state of a part.
pub(super) fn part_state(inner: &Inner, content_type: &str, has_bytes: bool, is_mms: bool) -> PartState {
    let cfg = &inner.config;
    let family = inner.edges.file_types.family(content_type.to_string());

    if is_mms && family == FileFamily::Text {
        return PartState::Hidden;
    }

    let supported = match family {
        FileFamily::Text | FileFamily::Image => true,
        FileFamily::VCard => cfg.vcard_parts_enabled,
        FileFamily::Audio => cfg.audio_parts_enabled,
        FileFamily::Video => cfg.video_parts_enabled,
        FileFamily::Document => cfg.document_parts_enabled,
        FileFamily::Other => false,
    };
    if !supported {
        return PartState::Unsupported;
    }

    match (family, has_bytes) {
        (FileFamily::Image, true) => PartState::Image,
        (FileFamily::Audio | FileFamily::Video, true) => PartState::Playable,
        (FileFamily::Document, true) => PartState::Openable,
        _ => PartState::Downloadable,
    }
}

/// The state of one stored part; `None` when the part is unknown. A pending send's attachment is
/// always an image with its bytes (rule 5).
pub(super) fn part_state_of(c: &Connection, inner: &Inner, part: PartRef) -> rusqlite::Result<Option<PartState>> {
    Ok(match part {
        PartRef::MmsPart(id) => {
            store::mms_part(c, id)?.map(|p| part_state(inner, &p.content_type, p.bytes.is_some(), true))
        }
        PartRef::RcsFile(id) => store::rcs_file_transfer(c, id)?
            .map(|f| part_state(inner, &f.row.common.content_type, f.row.bytes.is_some(), false)),
        PartRef::SentPart { send_id, index } => store::sent_part_bytes(c, send_id, index)?.map(|_| PartState::Image),
    })
}

/// What sending needs to know about a shown conversation.
pub(super) struct ConversationShape {
    pub recipient_count: usize,
    pub rcs_type: Option<i32>,
}

pub(super) fn conversation_shape(c: &Connection, thread_id: i64) -> rusqlite::Result<Option<ConversationShape>> {
    let sms = store::conversation(c, thread_id)?;
    let rcs = store::rcs_conversations_for(c, thread_id)?;
    if sms.is_none() && rcs.is_empty() {
        return Ok(None);
    }

    let recipients = match &sms {
        Some(s) => s.recipients.len(),
        None => rcs.first().map_or(0, |r| r.recipients.len()),
    };
    Ok(Some(ConversationShape { recipient_count: recipients, rcs_type: rcs.first().map(|r| r.conversation_type) }))
}

/// The country of subscription `id` when it names one (the contact lookup's region).
fn region_of(subscriptions: &[SubscriptionRow], id: i64) -> Option<String> {
    subscriptions.iter().find(|s| s.id == id).map(|s| s.country_iso.clone()).filter(|c| !c.is_empty())
}

impl Inner {
    fn recipient(&self, phone: &DcgClient, address: &str, region: Option<&str>) -> Recipient {
        let found = self.contacts.as_ref().and_then(|c| c.lookup(phone, address, region));
        Recipient {
            address: address.to_string(),
            photo_contact_id: found.as_ref().and_then(|m| m.photo_contact_id),
            contact_id: found.as_ref().map(|m| m.contact_id),
            name: found.and_then(|m| m.display_name),
        }
    }

    /// The list preview of `latest`: its text as [`Messaging::messages`] shows it (an MMS's hidden
    /// text parts joined by a space) and whether it carries an attachment.
    fn preview(
        &self,
        phone: &DcgClient,
        latest: store::LatestMessage,
        latest_parts: Vec<store::ShownPart>,
        region: Option<&str>,
    ) -> ConversationPreview {
        let (body, attachment) = match latest.table {
            store::LatestTable::Sms | store::LatestTable::RcsChat => (latest.body, false),
            store::LatestTable::RcsFile => (String::new(), true),
            store::LatestTable::Mms => {
                let mut text = Vec::new();
                let mut attachment = false;
                for p in &latest_parts {
                    if part_state(self, &p.content_type, p.has_bytes, true) == PartState::Hidden {
                        text.push(p.text.as_str());
                    } else {
                        attachment = true;
                    }
                }
                (text.join(" "), attachment)
            }
        };

        let from = (latest.mailbox == mailbox::INBOX).then(|| self.recipient(phone, &latest.from_address, region));
        ConversationPreview { body, attachment, from }
    }

    /// A pending send as a message of its conversation (sent from this PC; its own contact empty).
    fn pending_view(s: store::ListedSend) -> MessageView {
        let send_id = s.id;
        MessageView {
            kind: MessageKind::PendingSend,
            id: s.id,
            timestamp_ms: s.created_ms,
            mailbox: mailbox::SENT,
            from: Recipient { address: String::new(), name: None, contact_id: None, photo_contact_id: None },
            body: s.body,
            subject: String::new(),
            read_on_phone: true,
            read_on_pc: true,
            subscription_id: s.subscription_id,
            group_event: None,
            parts: s
                .parts
                .into_iter()
                .enumerate()
                .map(|(i, p)| PartView {
                    part: PartRef::SentPart { send_id, index: u32::try_from(i).expect("at most two parts per send") },
                    content_type: p.content_type,
                    name: p.name,
                    size: p.size,
                    state: PartState::Image,
                    has_bytes: true,
                })
                .collect(),
            send_status: Some(s.status),
        }
    }
}

/// One conversation as the store holds it, before it is shown.
struct ConversationRow {
    thread_id: i64,
    recipients: Vec<String>,
    rcs_type: Option<i32>,
    counts: store::ConversationCounts,
    attr: store::ConversationAttr,
    subscription: Option<SubscriptionRow>,
    latest: Option<store::LatestMessage>,
    latest_parts: Vec<store::ShownPart>,
}

/// Every conversation's row: the SMS/MMS threads, then the fallback ids of RCS-only conversations.
fn conversation_rows(c: &Connection) -> rusqlite::Result<Vec<ConversationRow>> {
    let subscriptions = store::subscriptions(c)?;
    let mut threads: Vec<i64> = store::all_conversations(c)?.into_iter().map(|s| s.thread_id).collect();

    for r in store::all_rcs_conversations(c)? {
        if !threads.contains(&r.fallback_id) {
            threads.push(r.fallback_id);
        }
    }

    let mut out = Vec::new();
    for t in threads {
        out.push(conversation_row(c, t, &subscriptions)?);
    }
    Ok(out)
}

/// The row of the conversation shown under thread `t`.
fn conversation_row(c: &Connection, t: i64, subscriptions: &[SubscriptionRow]) -> rusqlite::Result<ConversationRow> {
    let sms = store::conversation(c, t)?;
    let rcs = store::rcs_conversations_for(c, t)?;
    let counts = store::conversation_counts(c, t)?;
    let attr = store::conversation_attr(c, t)?;
    let subscription = send::active_subscription(c, Some(t), subscriptions)?;

    let recipients = match &sms {
        Some(s) => s.recipients.clone(),
        None => rcs.first().map(|r| r.recipients.clone()).unwrap_or_default(),
    };

    let latest = store::latest_message(c, t)?;
    let latest_parts = match &latest {
        Some(l) if l.table == store::LatestTable::Mms => store::shown_parts(c, l.id)?,
        _ => Vec::new(),
    };

    Ok(ConversationRow {
        thread_id: t,
        recipients,
        rcs_type: rcs.first().map(|r| r.conversation_type),
        counts,
        attr,
        subscription,
        latest,
        latest_parts,
    })
}

/// The RCS group rule: an RCS group conversation takes replies only while active, with RCS send on
/// and an RCS-capable subscription.
fn rcs_reply_allowed(rcs_type: Option<i32>, p: &Predicates, subscription: Option<&SubscriptionRow>) -> bool {
    match rcs_type {
        Some(t) if t == rcs_conversation_type::ACTIVE_GROUP_CHAT || t == rcs_conversation_type::INACTIVE_GROUP_CHAT => {
            // Without a subscription the "unknown" SIM applies, whose RCS send is off.
            t != rcs_conversation_type::INACTIVE_GROUP_CHAT
                && p.rcs_send()
                && subscription.is_some_and(|s| s.is_rcs_supported)
        }
        _ => true,
    }
}

/// What [`Messaging::messages`] reads from the store for one thread.
struct ThreadRows {
    sms: Vec<store::StoredSms>,
    mms: Vec<store::ShownMms>,
    rcs_chats: Vec<store::StoredRcsChat>,
    rcs_files: Vec<store::ShownRcsFile>,
    sends: Vec<store::ListedSend>,
    subscriptions: Vec<SubscriptionRow>,
}

/// The messages, RCS items and pending sends shown under `thread_id`, and the subscriptions.
fn thread_rows(c: &Connection, thread_id: i64) -> rusqlite::Result<ThreadRows> {
    let mut chats = Vec::new();
    let mut files = Vec::new();
    for r in store::rcs_conversations_for(c, thread_id)? {
        chats.extend(store::rcs_chats_in(c, r.id)?);
        files.extend(store::shown_rcs_files_in(c, r.id)?);
    }

    let mut sends = Vec::new();
    for s in store::listed_sends(c)? {
        if store::shown_thread(c, s.thread_id, s.origin)? == Some(thread_id) {
            sends.push(s);
        }
    }

    Ok(ThreadRows {
        sms: store::sms_in_thread(c, thread_id)?,
        mms: store::shown_mms_in_thread(c, thread_id)?,
        rcs_chats: chats,
        rcs_files: files,
        sends,
        subscriptions: store::subscriptions(c)?,
    })
}

impl Inner {
    /// `row` as the conversation list shows it.
    fn conversation_view(
        &self,
        phone: &DcgClient,
        p: &Predicates,
        send_mms: bool,
        row: ConversationRow,
    ) -> ConversationView {
        let ConversationRow { thread_id, recipients, rcs_type, counts, attr, subscription, latest, latest_parts } = row;

        let limits = CarrierLimits::of(subscription.as_ref());
        let is_group = recipients.len() > 1;
        let group_mms_ok = !is_group || subscription.as_ref().is_none_or(|s| s.is_group_mms_enabled);
        let rcs_ok = rcs_reply_allowed(rcs_type, p, subscription.as_ref());
        let within_limit = limits.max_recipients.is_none_or(|max| recipients.len() <= max as usize);
        let region = subscription.as_ref().map(|s| s.country_iso.clone()).filter(|c| !c.is_empty());

        ConversationView {
            thread_id,
            recipients: recipients.iter().map(|a| self.recipient(phone, a, region.as_deref())).collect(),
            is_group,
            latest_ms: counts.latest_ms,
            pc_unread: counts.pc_unread,
            phone_unread: counts.phone_unread,
            muted: attr.muted,
            hidden: attr.hidden,
            rcs_type,
            can_reply: !recipients.is_empty() && group_mms_ok && rcs_ok && within_limit,
            attachments_allowed: subscription.as_ref().is_none_or(|s| s.is_mms_enabled) && send_mms,
            active_subscription: subscription.as_ref().map_or(NONE_ID, |s| s.id),
            max_text_length: limits.max_text_length,
            max_recipients: limits.max_recipients,
            preview: latest.map(|l| self.preview(phone, l, latest_parts, region.as_deref())),
        }
    }

    /// An SMS as a message of its conversation.
    fn sms_view(&self, phone: &DcgClient, s: store::StoredSms, subscriptions: &[SubscriptionRow]) -> MessageView {
        let r = s.row;
        let counterpart = if r.mailbox == mailbox::INBOX { &r.from_address } else { &r.to_address };

        MessageView {
            kind: MessageKind::Sms,
            id: r.id,
            timestamp_ms: r.timestamp_ms,
            mailbox: r.mailbox,
            from: self.recipient(phone, counterpart, region_of(subscriptions, r.subscription_id).as_deref()),
            body: r.body,
            subject: r.subject,
            read_on_phone: r.read,
            read_on_pc: s.pc_read,
            subscription_id: r.subscription_id,
            group_event: None,
            parts: Vec::new(),
            send_status: None,
        }
    }

    /// An MMS as a message of its conversation: its hidden text parts joined as the body.
    fn mms_view(&self, phone: &DcgClient, m: store::ShownMms, subscriptions: &[SubscriptionRow]) -> MessageView {
        let r = m.row;
        let parts: Vec<PartView> = m
            .parts
            .iter()
            .map(|p| PartView {
                part: PartRef::MmsPart(p.part_id),
                content_type: p.content_type.clone(),
                name: p.name.clone(),
                size: p.size,
                state: part_state(self, &p.content_type, p.has_bytes, true),
                has_bytes: p.has_bytes,
            })
            .collect();

        let body: Vec<&str> = m
            .parts
            .iter()
            .zip(&parts)
            .filter(|(_, v)| v.state == PartState::Hidden)
            .map(|(p, _)| p.text.as_str())
            .collect();

        MessageView {
            kind: MessageKind::Mms,
            id: r.id,
            timestamp_ms: r.timestamp_ms,
            mailbox: r.mailbox,
            from: self.recipient(phone, &r.from_address, region_of(subscriptions, r.subscription_id).as_deref()),
            body: body.join(" "),
            subject: subject::mms_subject_text(&r.subject, r.subject_charset),
            read_on_phone: r.read,
            read_on_pc: m.pc_read,
            subscription_id: r.subscription_id,
            group_event: None,
            parts: parts.into_iter().filter(|p| p.state != PartState::Hidden).collect(),
            send_status: None,
        }
    }

    /// An RCS chat message (or a shown group event) as a message of its conversation.
    fn rcs_chat_view(
        &self,
        phone: &DcgClient,
        c: store::StoredRcsChat,
        subscriptions: &[SubscriptionRow],
    ) -> MessageView {
        let r = c.row;
        let event = (rcs_message_type::FIRST_GROUP_EVENT..=rcs_message_type::LAST_SHOWN_GROUP_EVENT)
            .contains(&r.common.rcs_message_type)
            .then_some(r.common.rcs_message_type);

        MessageView {
            kind: MessageKind::RcsChat,
            id: r.common.id,
            timestamp_ms: r.common.timestamp_ms,
            mailbox: r.common.mailbox,
            from: self.recipient(
                phone,
                &r.common.from_address,
                region_of(subscriptions, r.common.subscription_id).as_deref(),
            ),
            body: r.body,
            subject: String::new(),
            read_on_phone: r.common.read,
            read_on_pc: c.pc_read,
            subscription_id: r.common.subscription_id,
            group_event: event,
            parts: Vec::new(),
            send_status: None,
        }
    }

    /// An RCS file transfer as a message of its conversation with the file as its one part.
    fn rcs_file_view(
        &self,
        phone: &DcgClient,
        f: store::ShownRcsFile,
        subscriptions: &[SubscriptionRow],
    ) -> MessageView {
        let r = f.common;
        MessageView {
            kind: MessageKind::RcsFileTransfer,
            id: r.id,
            timestamp_ms: r.timestamp_ms,
            mailbox: r.mailbox,
            from: self.recipient(phone, &r.from_address, region_of(subscriptions, r.subscription_id).as_deref()),
            body: String::new(),
            subject: String::new(),
            read_on_phone: r.read,
            read_on_pc: f.pc_read,
            subscription_id: r.subscription_id,
            group_event: None,
            parts: vec![PartView {
                part: PartRef::RcsFile(r.id),
                state: part_state(self, &r.content_type, f.has_bytes, false),
                content_type: r.content_type,
                name: f.file_name,
                size: i64::from(f.file_size),
                has_bytes: f.has_bytes,
            }],
            send_status: None,
        }
    }
}

impl Messaging {
    /// Every conversation, newest first; mass-text copies and RCS group events are not counted.
    pub fn conversations(&self) -> Result<Vec<ConversationView>, MessagingError> {
        let inner = &self.inner;
        let (_, phone, record) = inner.selected()?;
        let p = inner.predicates(&phone, &record)?;
        let db = inner.db(&phone)?;
        let state = db.read(store::phone_state)?;
        let rows = db.read(conversation_rows)?;

        let mut views: Vec<ConversationView> =
            rows.into_iter().map(|row| inner.conversation_view(&phone, &p, state.settings.send_mms, row)).collect();
        views.sort_by(|a, b| b.latest_ms.cmp(&a.latest_ms).then(b.thread_id.cmp(&a.thread_id)));
        Ok(views)
    }

    /// The messages of the conversation shown under `thread_id`, oldest first, then its pending
    /// sends — those whose thread is an RCS conversation (an RCS notification reply) under that RCS
    /// conversation's fallback id ([`store::shown_thread`]). Mass-text copies (`groupIds` ≠ −1 and ≠
    /// their own id) are not shown. Parts come without their bytes ([`Messaging::part_bytes`]).
    pub fn messages(&self, thread_id: i64) -> Result<Vec<MessageView>, MessagingError> {
        let inner = &self.inner;
        let (_, phone, _) = inner.selected()?;
        let db = inner.db(&phone)?;
        let ThreadRows { sms, mms, rcs_chats, rcs_files, sends, subscriptions } =
            db.read(|c| thread_rows(c, thread_id))?;

        let mut out = Vec::new();
        for s in sms {
            if s.row.group_id != NONE_ID && s.row.group_id != s.row.id {
                continue;
            }
            out.push(inner.sms_view(&phone, s, &subscriptions));
        }
        for m in mms {
            out.push(inner.mms_view(&phone, m, &subscriptions));
        }
        for c in rcs_chats {
            out.push(inner.rcs_chat_view(&phone, c, &subscriptions));
        }
        for f in rcs_files {
            out.push(inner.rcs_file_view(&phone, f, &subscriptions));
        }

        out.sort_by(|a, b| a.timestamp_ms.cmp(&b.timestamp_ms).then(a.id.cmp(&b.id)));
        out.extend(sends.into_iter().map(Inner::pending_view));
        Ok(out)
    }

    /// The pending sends to a new conversation (no thread yet), oldest first, each with the
    /// addresses it was sent to. An acked one is gone: its message arrives in its conversation.
    pub fn new_conversation_messages(&self) -> Result<Vec<NewConversationView>, MessagingError> {
        let inner = &self.inner;
        let (_, phone, _) = inner.selected()?;
        let (sends, subscriptions) =
            inner.db(&phone)?.read(|c| Ok((store::listed_sends(c)?, store::subscriptions(c)?)))?;

        Ok(sends
            .into_iter()
            .filter(|s| s.thread_id == NONE_ID)
            .map(|s| {
                let region = region_of(&subscriptions, s.subscription_id);
                let recipients = s.recipients.iter().map(|a| inner.recipient(&phone, a, region.as_deref())).collect();
                NewConversationView { recipients, message: Inner::pending_view(s) }
            })
            .collect())
    }

    /// What the new-message composer allows (rules 6 and 9 for the default-for-SMS subscription).
    pub fn new_message_limits(&self) -> Result<NewMessageLimits, MessagingError> {
        let (_, phone, _) = self.inner.selected()?;
        let db = self.inner.db(&phone)?;
        let state = db.read(store::phone_state)?;
        let subscription = db.read(|c| send::active_subscription(c, None, &store::subscriptions(c)?))?;
        let limits = CarrierLimits::of(subscription.as_ref());

        Ok(NewMessageLimits {
            attachments_allowed: subscription.as_ref().is_none_or(|s| s.is_mms_enabled) && state.settings.send_mms,
            group_allowed: subscription.as_ref().is_none_or(|s| s.is_group_mms_enabled),
            max_text_length: limits.max_text_length,
            max_recipients: limits.max_recipients,
            active_subscription: subscription.as_ref().map_or(NONE_ID, |s| s.id),
        })
    }

    /// The bytes of a shown part: `None` when the store holds none for it (a part offered for
    /// download).
    pub fn part_bytes(&self, part: PartRef) -> Result<Option<Vec<u8>>, MessagingError> {
        let (_, phone, _) = self.inner.selected()?;
        let db = self.inner.db(&phone)?;
        match part {
            PartRef::MmsPart(id) => db.read(|c| store::mms_part_bytes(c, id))?.ok_or(MessagingError::UnknownPart(id)),
            PartRef::RcsFile(id) => db.read(|c| store::rcs_file_bytes(c, id))?.ok_or(MessagingError::UnknownPart(id)),
            PartRef::SentPart { send_id, index } => db
                .read(|c| store::sent_part_bytes(c, send_id, index))?
                .map(Some)
                .ok_or(MessagingError::UnknownSend(send_id)),
        }
    }

    /// The SIM subscriptions the phone reported.
    pub fn subscriptions(&self) -> Result<Vec<SubscriptionRow>, MessagingError> {
        let (_, phone, _) = self.inner.selected()?;
        Ok(self.inner.db(&phone)?.read(store::subscriptions)?)
    }
}
