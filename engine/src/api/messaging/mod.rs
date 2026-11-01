//! Messaging for the UI ([`crate::features::messaging`]).
//!
//! It covers conversations and their messages (parts listed without their bytes, read one part at a
//! time), the messages sent to a new conversation with their addresses, what the new-message
//! composer allows, sending and retrying, media downloads, the mark-read triggers, the PC settings
//! and the messages permission prompt.

use std::sync::Arc;

use crate::api::engine::Engine;
use crate::features::messaging::{
    Attachment, ConversationPreview, ConversationView, MessageKind, MessageView, Messaging, MessagingError,
    MessagingEvent, NewConversationView, NewMessageLimits as FeatureLimits, PartRef, PartState, PartView, PcSettings,
    ProximityState, Recipient, SendMessageFailureReason, SendOutcome, SendStatus, SendTarget,
    consts::{mailbox, rcs_message_type},
    parse::SubscriptionRow,
};

use super::{FeatureApiError, RequestOutcome, core_of, features_of};

impl From<MessagingError> for FeatureApiError {
    fn from(e: MessagingError) -> Self {
        match e {
            MessagingError::NoPhone => FeatureApiError::NoPhone,
            MessagingError::AppGone => FeatureApiError::NotStarted,
            MessagingError::App(e) => e.into(),
            MessagingError::Store(e) => e.into(),
            MessagingError::UnknownConversation(_) => FeatureApiError::UnknownItem { what: "conversation".into() },
            MessagingError::UnknownSend(_) => FeatureApiError::UnknownItem { what: "pending send".into() },
            MessagingError::UnknownPart(_) => FeatureApiError::UnknownItem { what: "part".into() },
            e @ MessagingError::NotDownloadable => FeatureApiError::NotAvailable { reason: e.to_string() },
            e @ (MessagingError::NothingToSend
            | MessagingError::TooManyAttachments { .. }
            | MessagingError::UnknownAttachmentType
            | MessagingError::NotAnImage { .. }
            | MessagingError::NotFailed(_)) => FeatureApiError::InvalidInput { reason: e.to_string() },
            MessagingError::NotAllowed(reason) => FeatureApiError::NotAvailable { reason: reason.into() },
            e @ MessagingError::EdgeFailed(_) => FeatureApiError::Internal { message: e.to_string() },
        }
    }
}

/// An address with its contact.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MessageContact {
    pub address: String,
    /// The contact's name, when the phone's contacts have one.
    pub name: Option<String>,
    /// The contact the address belongs to.
    pub contact_id: Option<i64>,
    /// The contact whose photo shows for the address — the first of its contacts with one, which need
    /// not be `contact_id` — read with `ContactsApi::thumbnail`.
    pub photo_contact_id: Option<i64>,
}

impl From<&Recipient> for MessageContact {
    fn from(r: &Recipient) -> Self {
        Self {
            address: r.address.clone(),
            name: r.name.clone(),
            contact_id: r.contact_id,
            photo_contact_id: r.photo_contact_id,
        }
    }
}

/// The latest message of a conversation, for the list's preview line.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ConversationPreviewLine {
    /// Empty when the message has only attachments.
    pub body: String,
    pub attachment: bool,
    /// The sender of a received message; `None` for a sent one.
    pub from: Option<MessageContact>,
}

impl From<&ConversationPreview> for ConversationPreviewLine {
    fn from(p: &ConversationPreview) -> Self {
        Self { body: p.body.clone(), attachment: p.attachment, from: p.from.as_ref().map(Into::into) }
    }
}

/// One conversation of the list.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Conversation {
    pub thread_id: i64,
    pub recipients: Vec<MessageContact>,
    pub is_group: bool,
    /// Unix ms of the latest message.
    pub latest_ms: i64,
    pub preview: Option<ConversationPreviewLine>,
    /// Unread on this Mac.
    pub unread: i64,
    pub muted: bool,
    pub hidden: bool,
    /// The composer may send here.
    pub can_reply: bool,
    pub attachments_allowed: bool,
    /// The SIM a send uses, `-1` when none applies.
    pub active_subscription: i64,
    /// Composer limits; `None` = unlimited.
    pub max_text_length: Option<i32>,
    pub max_recipients: Option<i32>,
}

impl From<&ConversationView> for Conversation {
    fn from(v: &ConversationView) -> Self {
        Self {
            thread_id: v.thread_id,
            recipients: v.recipients.iter().map(Into::into).collect(),
            is_group: v.is_group,
            latest_ms: v.latest_ms,
            preview: v.preview.as_ref().map(Into::into),
            unread: v.pc_unread,
            muted: v.muted,
            hidden: v.hidden,
            can_reply: v.can_reply,
            attachments_allowed: v.attachments_allowed,
            active_subscription: v.active_subscription,
            max_text_length: v.max_text_length,
            max_recipients: v.max_recipients,
        }
    }
}

/// The kind of a message: SMS, MMS, an RCS chat or file, or a send the phone has not confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MessageType {
    Sms,
    Mms,
    RcsChat,
    RcsFileTransfer,
    /// Sent from this Mac; the phone has not confirmed it yet.
    PendingSend,
}

/// A part of a message: what `download_part` names and `part_bytes` reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum MessagePartRef {
    MmsPart {
        id: i64,
    },
    RcsFile {
        id: i64,
    },
    /// An attachment of a message sent from this Mac (never downloaded).
    SentPart {
        send_id: i64,
        index: u32,
    },
}

impl From<PartRef> for MessagePartRef {
    fn from(p: PartRef) -> Self {
        match p {
            PartRef::MmsPart(id) => MessagePartRef::MmsPart { id },
            PartRef::RcsFile(id) => MessagePartRef::RcsFile { id },
            PartRef::SentPart { send_id, index } => MessagePartRef::SentPart { send_id, index },
        }
    }
}

impl From<MessagePartRef> for PartRef {
    fn from(p: MessagePartRef) -> Self {
        match p {
            MessagePartRef::MmsPart { id } => PartRef::MmsPart(id),
            MessagePartRef::RcsFile { id } => PartRef::RcsFile(id),
            MessagePartRef::SentPart { send_id, index } => PartRef::SentPart { send_id, index },
        }
    }
}

/// What a part shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MessagePartState {
    /// An image with its bytes.
    Image,
    /// Audio or video with its bytes.
    Playable,
    /// A document with its bytes.
    Openable,
    /// Offered for download.
    Downloadable,
    /// Not supported here.
    Unsupported,
}

/// One attachment of a message, without its bytes (`part_bytes` reads them).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MessagePart {
    pub part: MessagePartRef,
    pub content_type: String,
    pub name: String,
    pub size: i64,
    pub state: MessagePartState,
    /// This Mac holds the part's bytes.
    pub has_bytes: bool,
}

/// Why the phone could not send a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SendFailure {
    AirplaneMode,
    NoSim,
    MessageSizeExceeded,
    MmsNotSupported,
    MmsDisabled,
    MmsGroupMessageDisabled,
    MmsTooManyRecipients,
    MmsSubjectSizeExceeded,
    MmsAudioNotAllowed,
    MmsTransactionSizeExceeded,
    Unknown,
    ActionRequired,
    Empty,
    RcsSendUnavailable,
    PhoneTimeout,
    RadioOff,
    NoService,
    NoRecipients,
    BluetoothNotConnected,
    /// A reason outside the known ones, as the phone sent it.
    Other {
        code: i32,
    },
}

impl From<i32> for SendFailure {
    fn from(code: i32) -> Self {
        use SendMessageFailureReason as R;
        match R::from_i32(code) {
            R::AirplaneMode => SendFailure::AirplaneMode,
            R::NoSim => SendFailure::NoSim,
            R::MessageSizeExceeded => SendFailure::MessageSizeExceeded,
            R::MmsNotSupported => SendFailure::MmsNotSupported,
            R::MmsDisabled => SendFailure::MmsDisabled,
            R::MmsGroupMessageDisabled => SendFailure::MmsGroupMessageDisabled,
            R::MmsTooManyRecipients => SendFailure::MmsTooManyRecipients,
            R::MmsSubjectSizeExceeded => SendFailure::MmsSubjectSizeExceeded,
            R::MmsAudioNotAllowed => SendFailure::MmsAudioNotAllowed,
            R::MmsTransactionSizeExceeded => SendFailure::MmsTransactionSizeExceeded,
            R::Unknown => SendFailure::Unknown,
            R::ActionRequired => SendFailure::ActionRequired,
            R::Empty => SendFailure::Empty,
            R::RcsSendUnavailable => SendFailure::RcsSendUnavailable,
            R::PhoneTimeout => SendFailure::PhoneTimeout,
            R::RadioOff => SendFailure::RadioOff,
            R::NoService => SendFailure::NoService,
            R::NoRecipients => SendFailure::NoRecipients,
            R::BluetoothNotConnected => SendFailure::BluetoothNotConnected,
            R::NoFailure | R::Other(_) => SendFailure::Other { code },
        }
    }
}

/// The state of a message sent from this Mac.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MessageSendState {
    Sending,
    Sent,
    Failed { reason: SendFailure },
}

impl From<SendStatus> for MessageSendState {
    fn from(s: SendStatus) -> Self {
        match s {
            SendStatus::Sending => MessageSendState::Sending,
            SendStatus::Sent => MessageSendState::Sent,
            SendStatus::Failed(code) => MessageSendState::Failed { reason: code.into() },
        }
    }
}

/// The phone mailbox a message is in (`MessageMailboxType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MessageMailbox {
    /// Received.
    Inbox,
    /// Sent (also a message sent from this Mac).
    Sent,
    /// A draft on the phone.
    Draft,
    /// Waiting on the phone to be sent.
    Outbox,
    /// The phone could not send it.
    Failed,
    /// Queued on the phone to be sent.
    Queued,
    /// `Unknown` (−1), or a value outside the type.
    Unknown,
}

impl MessageMailbox {
    fn of(code: i32) -> Self {
        match code {
            mailbox::INBOX => MessageMailbox::Inbox,
            mailbox::SENT => MessageMailbox::Sent,
            mailbox::DRAFT => MessageMailbox::Draft,
            mailbox::OUTBOX => MessageMailbox::Outbox,
            mailbox::FAILED => MessageMailbox::Failed,
            mailbox::QUEUED => MessageMailbox::Queued,
            _ => MessageMailbox::Unknown,
        }
    }
}

/// What an RCS group event says happened (`RcsMessageType` 1–8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MessageGroupEvent {
    /// The phone's user joined.
    JoinedSelf,
    /// The event's contact joined.
    Joined,
    /// The phone's user left.
    LeftSelf,
    /// The event's contact left.
    Left,
    AllParticipantsLeft,
    LeaderChanged,
    RenamedByLeader,
    /// The event's contact was removed by the leader.
    RemovedByLeader,
}

impl MessageGroupEvent {
    /// The event of an `rcsMessageTypes` value; `None` outside the group events 1–8.
    fn of(code: i32) -> Option<Self> {
        if !(rcs_message_type::FIRST_GROUP_EVENT..=rcs_message_type::LAST_SHOWN_GROUP_EVENT).contains(&code) {
            return None;
        }

        Some(match code {
            1 => MessageGroupEvent::JoinedSelf,
            2 => MessageGroupEvent::Joined,
            3 => MessageGroupEvent::LeftSelf,
            4 => MessageGroupEvent::Left,
            5 => MessageGroupEvent::AllParticipantsLeft,
            6 => MessageGroupEvent::LeaderChanged,
            7 => MessageGroupEvent::RenamedByLeader,
            _ => MessageGroupEvent::RemovedByLeader,
        })
    }
}

/// One message of a conversation.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Message {
    pub kind: MessageType,
    pub id: i64,
    pub timestamp_ms: i64,
    /// Received (inbox) rather than sent from the phone or this Mac.
    pub incoming: bool,
    /// The phone mailbox it is in (`Sent` for a message sent from this Mac).
    pub mailbox: MessageMailbox,
    /// The sender of a received message, the recipient of a sent one.
    pub contact: MessageContact,
    pub body: String,
    pub subject: String,
    pub read: bool,
    pub subscription_id: i64,
    /// For an RCS group event ("X joined") rather than a message: what happened. Its `contact` is
    /// the event's sender, its `body` what the phone wrote about it (may be empty).
    pub group_event: Option<MessageGroupEvent>,
    pub parts: Vec<MessagePart>,
    /// For a message sent from this Mac: its state.
    pub send_state: Option<MessageSendState>,
}

fn part_state(s: PartState) -> Option<MessagePartState> {
    match s {
        PartState::Hidden => None,
        PartState::Image => Some(MessagePartState::Image),
        PartState::Playable => Some(MessagePartState::Playable),
        PartState::Openable => Some(MessagePartState::Openable),
        PartState::Downloadable => Some(MessagePartState::Downloadable),
        PartState::Unsupported => Some(MessagePartState::Unsupported),
    }
}

fn part(p: &PartView) -> Option<MessagePart> {
    Some(MessagePart {
        part: p.part.into(),
        content_type: p.content_type.clone(),
        name: p.name.clone(),
        size: p.size,
        state: part_state(p.state)?,
        has_bytes: p.has_bytes,
    })
}

impl From<&MessageView> for Message {
    fn from(v: &MessageView) -> Self {
        Self {
            kind: match v.kind {
                MessageKind::Sms => MessageType::Sms,
                MessageKind::Mms => MessageType::Mms,
                MessageKind::RcsChat => MessageType::RcsChat,
                MessageKind::RcsFileTransfer => MessageType::RcsFileTransfer,
                MessageKind::PendingSend => MessageType::PendingSend,
            },
            id: v.id,
            timestamp_ms: v.timestamp_ms,
            incoming: v.mailbox == mailbox::INBOX,
            mailbox: MessageMailbox::of(v.mailbox),
            contact: (&v.from).into(),
            body: v.body.clone(),
            subject: v.subject.clone(),
            read: v.read_on_phone || v.read_on_pc,
            subscription_id: v.subscription_id,
            group_event: v.group_event.and_then(MessageGroupEvent::of),
            parts: v.parts.iter().filter_map(part).collect(),
            send_state: v.send_status.map(Into::into),
        }
    }
}

/// A message sent from this Mac to a new conversation that the phone has not confirmed yet.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct NewConversationMessage {
    /// The addresses it was sent to, as the user entered or picked them, with their contacts.
    pub recipients: Vec<MessageContact>,
    pub message: Message,
}

impl From<&NewConversationView> for NewConversationMessage {
    fn from(v: &NewConversationView) -> Self {
        Self { recipients: v.recipients.iter().map(Into::into).collect(), message: (&v.message).into() }
    }
}

/// What the new-message composer allows: a new conversation sends from the default SIM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct NewMessageLimits {
    /// Images may be attached.
    pub attachments_allowed: bool,
    /// More than one recipient may be sent to.
    pub group_allowed: bool,
    /// The carrier's text limit in UTF-16 code units; `None` = unlimited.
    pub max_text_length: Option<i32>,
    /// The carrier's recipient limit; `None` = unlimited.
    pub max_recipients: Option<i32>,
    /// The SIM a new message sends from, `-1` when none applies.
    pub active_subscription: i64,
}

impl From<FeatureLimits> for NewMessageLimits {
    fn from(l: FeatureLimits) -> Self {
        Self {
            attachments_allowed: l.attachments_allowed,
            group_allowed: l.group_allowed,
            max_text_length: l.max_text_length,
            max_recipients: l.max_recipients,
            active_subscription: l.active_subscription,
        }
    }
}

/// A SIM of the phone.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SimSubscription {
    pub id: i64,
    pub sim_slot_index: i64,
    pub name: String,
    pub number: String,
    pub is_default_sms: bool,
}

impl From<&SubscriptionRow> for SimSubscription {
    fn from(s: &SubscriptionRow) -> Self {
        Self {
            id: s.id,
            sim_slot_index: s.sim_slot_index,
            name: s.name.clone(),
            number: s.number.clone(),
            is_default_sms: s.is_default_sms,
        }
    }
}

/// Where a message goes.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum MessageTarget {
    Conversation { thread_id: i64 },
    NewConversation { recipients: Vec<String> },
}

/// An attachment as the user added it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MessageAttachment {
    /// The file name; empty for a pasted image.
    pub name: String,
    /// The MIME type when known; otherwise it is detected from the bytes.
    pub content_type: Option<String>,
    pub bytes: Vec<u8>,
}

/// What a send did.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SendResult {
    pub send_id: i64,
    pub state: MessageSendState,
    pub outcome: RequestOutcome,
}

impl From<SendOutcome> for SendResult {
    fn from(o: SendOutcome) -> Self {
        Self { send_id: o.send_id, state: o.status.into(), outcome: o.outcome.into() }
    }
}

/// The PC settings of messaging (all default on).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct MessagingSettings {
    pub receive_mms: bool,
    pub auto_download_mms: bool,
    pub send_mms: bool,
    /// "Show messages on this Mac".
    pub pc_consent: bool,
    pub message_notifications: bool,
}

impl From<PcSettings> for MessagingSettings {
    fn from(s: PcSettings) -> Self {
        Self {
            receive_mms: s.receive_mms,
            auto_download_mms: s.auto_download_mms,
            send_mms: s.send_mms,
            pc_consent: s.pc_consent,
            message_notifications: s.message_notifications,
        }
    }
}

impl From<MessagingSettings> for PcSettings {
    fn from(s: MessagingSettings) -> Self {
        PcSettings {
            receive_mms: s.receive_mms,
            auto_download_mms: s.auto_download_mms,
            send_mms: s.send_mms,
            pc_consent: s.pc_consent,
            message_notifications: s.message_notifications,
        }
    }
}

/// Messaging's state for the selected phone.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MessagingState {
    /// The phone allows messages on this Mac.
    pub phone_permission: bool,
    pub settings: MessagingSettings,
    pub up_to_date: bool,
    /// The list shows its syncing state.
    pub syncing: bool,
    pub last_updated_ms: Option<i64>,
    pub setup_completed_once: bool,
    /// For a phone that sends only over Bluetooth: its Bluetooth state, shown as an error when it
    /// is not `Connected`. `None` for other phones.
    pub proximity: Option<ProximityState>,
}

/// A change to messaging: re-read conversations, a send or the state, or open a conversation.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum MessagingChange {
    /// Conversations or their messages changed; `threads` lists the conversations touched (empty:
    /// re-read every conversation).
    ContentChanged {
        threads: Vec<i64>,
    },
    /// A message sent from this Mac changed state (or was confirmed and is gone).
    SendChanged {
        send_id: i64,
    },
    StateChanged,
    /// The phone's messages permission changed.
    PermissionChanged {
        granted: bool,
    },
    /// The user clicked a message notification: show this conversation.
    OpenConversation {
        thread_id: i64,
    },
}

impl From<&MessagingEvent> for MessagingChange {
    fn from(e: &MessagingEvent) -> Self {
        match e {
            MessagingEvent::ContentChanged { threads, .. } => {
                MessagingChange::ContentChanged { threads: threads.clone() }
            }
            MessagingEvent::SendChanged { send_id, .. } => MessagingChange::SendChanged { send_id: *send_id },
            MessagingEvent::SyncStateChanged { .. } | MessagingEvent::SettingsChanged { .. } => {
                MessagingChange::StateChanged
            }
            MessagingEvent::PermissionChanged { granted, .. } => {
                MessagingChange::PermissionChanged { granted: *granted }
            }
            MessagingEvent::OpenConversation { thread_id, .. } => {
                MessagingChange::OpenConversation { thread_id: *thread_id }
            }
        }
    }
}

/// Messaging of the selected phone.
#[derive(uniffi::Object)]
pub struct MessagingApi {
    engine: Arc<Engine>,
}

impl MessagingApi {
    pub(crate) fn new(engine: Arc<Engine>) -> Arc<Self> {
        Arc::new(Self { engine })
    }

    fn feature(&self) -> Result<Arc<Messaging>, FeatureApiError> {
        let core = core_of(&self.engine)?;
        Ok(features_of(&core)?.messaging.clone())
    }

    fn run<T>(&self, f: impl FnOnce(&Messaging) -> Result<T, MessagingError>) -> Result<T, FeatureApiError> {
        let core = core_of(&self.engine)?;
        let _runtime = core.enter_runtime();
        let feature = features_of(&core)?.messaging.clone();
        Ok(f(&feature)?)
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl MessagingApi {
    /// The selected phone's messages permission, this Mac's messaging settings, and the sync state.
    ///
    /// Fails with `NoPhone` when no phone is selected.
    pub fn state(&self) -> Result<MessagingState, FeatureApiError> {
        self.run(|f: &Messaging| {
            let sync = f.sync_state()?;
            Ok(MessagingState {
                phone_permission: f.phone_permission()?,
                settings: f.pc_settings()?.into(),
                up_to_date: sync.up_to_date,
                syncing: sync.syncing,
                last_updated_ms: f.last_update_ms()?,
                setup_completed_once: f.setup_completed_once()?,
                proximity: f.proximity_state()?,
            })
        })
    }

    /// Every conversation, newest first.
    pub fn conversations(&self) -> Result<Vec<Conversation>, FeatureApiError> {
        self.run(|f| f.conversations()).map(|v| v.iter().map(Into::into).collect())
    }

    /// The messages of a conversation, oldest first, then those sent from this Mac and not yet
    /// confirmed. Parts come without their bytes (`part_bytes`).
    pub fn messages(&self, thread_id: i64) -> Result<Vec<Message>, FeatureApiError> {
        self.run(|f| f.messages(thread_id)).map(|v| v.iter().map(Into::into).collect())
    }

    /// The messages sent from this Mac to a new conversation ([`MessageTarget::NewConversation`])
    /// that the phone has not confirmed yet, oldest first, each with the addresses it went to: no
    /// conversation holds them until then, and a confirmed one is gone from here and in its
    /// conversation's `messages`.
    pub fn new_conversation_messages(&self) -> Result<Vec<NewConversationMessage>, FeatureApiError> {
        self.run(|f| f.new_conversation_messages()).map(|v| v.iter().map(Into::into).collect())
    }

    /// What the new-message composer allows.
    pub fn new_message_limits(&self) -> Result<NewMessageLimits, FeatureApiError> {
        self.run(|f| f.new_message_limits()).map(Into::into)
    }

    /// A part's bytes; `None` when this Mac holds none for it (a part offered for download).
    pub fn part_bytes(&self, part: MessagePartRef) -> Result<Option<Vec<u8>>, FeatureApiError> {
        self.run(|f| f.part_bytes(part.into()))
    }

    /// The phone's SIMs.
    pub fn subscriptions(&self) -> Result<Vec<SimSubscription>, FeatureApiError> {
        self.run(|f| f.subscriptions()).map(|v| v.iter().map(Into::into).collect())
    }

    /// Saves this Mac's messaging settings for the selected phone.
    ///
    /// The phone hears of them with the next sync, not at once.
    pub fn set_settings(&self, settings: MessagingSettings) -> Result<(), FeatureApiError> {
        self.run(|f| f.set_pc_settings(settings.into()))
    }

    /// Mutes or hides a conversation.
    pub fn set_conversation_flags(&self, thread_id: i64, muted: bool, hidden: bool) -> Result<(), FeatureApiError> {
        self.run(|f| f.set_conversation_flags(thread_id, muted, hidden))
    }

    /// The SIM a conversation sends from; `None` returns to the default.
    pub fn select_subscription(&self, thread_id: i64, subscription_id: Option<i64>) -> Result<(), FeatureApiError> {
        self.run(|f| f.select_subscription(thread_id, subscription_id))
    }

    /// Sends a message with up to two image attachments.
    pub async fn send(
        &self,
        target: MessageTarget,
        text: String,
        attachments: Vec<MessageAttachment>,
    ) -> Result<SendResult, FeatureApiError> {
        let target = match target {
            MessageTarget::Conversation { thread_id } => SendTarget::Conversation { thread_id },
            MessageTarget::NewConversation { recipients } => SendTarget::NewConversation { recipients },
        };
        let attachments = attachments
            .into_iter()
            .map(|a| Attachment { name: a.name, content_type: a.content_type, bytes: a.bytes })
            .collect();

        Ok(self.feature()?.send_message(target, &text, attachments).await?.into())
    }

    /// Sends a failed message again.
    pub async fn retry(&self, send_id: i64) -> Result<SendResult, FeatureApiError> {
        Ok(self.feature()?.retry_send(send_id).await?.into())
    }

    /// Asks the phone for a part's bytes and returns the request's outcome; on success the bytes
    /// arrive later with a `ContentChanged` change.
    pub async fn download_part(&self, part: MessagePartRef) -> Result<RequestOutcome, FeatureApiError> {
        Ok(self.feature()?.download_part(part.into()).await?.into())
    }

    /// Asks the phone to allow messages on this Mac.
    pub async fn request_permission(&self) -> Result<RequestOutcome, FeatureApiError> {
        Ok(self.feature()?.request_messages_permission().await?.into())
    }

    /// The conversation shown (`None`: none — also while the window shows no conversation, such as
    /// behind the messages-off or permission state). Starts the mark-read timer.
    pub fn conversation_selected(&self, thread_id: Option<i64>) -> Result<(), FeatureApiError> {
        self.run(|f| {
            f.on_conversation_selected(thread_id);
            Ok(())
        })
    }

    /// The window showing messages gained or lost focus; the mark-read triggers that need a focused
    /// window read the last value reported.
    pub fn window_focused(&self, focused: bool) -> Result<(), FeatureApiError> {
        self.run(|f| {
            f.on_window_focused(focused);
            Ok(())
        })
    }

    /// The messages view appeared or disappeared.
    pub fn messages_view_changed(&self, active: bool) -> Result<(), FeatureApiError> {
        self.run(|f| {
            f.on_messages_view_changed(active);
            Ok(())
        })
    }

    /// A new conversation's composer was replaced by the conversation it created.
    pub fn composed_conversation_replaced(&self, thread_id: i64) -> Result<(), FeatureApiError> {
        self.run(|f| {
            f.on_composed_conversation_replaced(thread_id);
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests;
