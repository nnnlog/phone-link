//! Vocabulary of the messaging feature.
//!
//! It covers the route, `contentType`s, `ValueSet` keys, `MessagingCapabilities`, the identity,
//! timestamp and other enumerations, `SendMessageFailureReason` (of the send response) and the
//! protocol's numeric limits.

/// The feature's store name and notification-id prefix.
pub const FEATURE: &str = "messaging";

/// The App route of every messaging request.
pub const ROUTE: &str = "/chat/v1";

/// The `contentType` of every messaging request and of phone-pushed messaging content.
pub const CONTENT_TYPE: &str = "messaging";

/// Elements of `subcontentTypes` (sync and media download requests).
pub mod sub {
    pub const MESSAGES: &str = "messages";
    pub const MESSAGES_MMS: &str = "messagesMms";
    pub const MESSAGES_MMS_MEDIA: &str = "messagesMmsMedia";
    pub const CONVERSATIONS: &str = "conversations";
    pub const SUBSCRIPTIONS: &str = "subscriptions";
    pub const MESSAGES_RCS_CHAT: &str = "messagesRcsChat";
    pub const MESSAGES_RCS_FILE_TRANSFER: &str = "messagesRcsFileTransfer";
    pub const CONVERSATIONS_RCS: &str = "conversationsRcs";
}

/// `ValueSet` keys of the messaging feature.
pub mod key {
    // Common request keys.
    pub const CORRELATION_VECTOR: &str = "correlationVector";
    pub const CONTENT_TYPE: &str = "contentType";
    pub const CONFIGURATION: &str = "configuration";
    pub const SUBCONTENT_TYPES: &str = "subcontentTypes";
    pub const REQUEST_TYPE: &str = "requestType";
    // `configuration` keys.
    pub const MESSAGING_SYNC: &str = "MessagingSync";
    pub const MESSAGING_MASS_TEXT: &str = "MessagingMassText";
    pub const MESSAGING_SEND: &str = "MessagingSend";
    pub const MESSAGING_RCS_RECEIVE: &str = "MessagingRcsReceive";
    pub const MESSAGING_RCS_SEND: &str = "MessagingRcsSend";
    pub const MESSAGING_RCS_UNSUPPORTED_MESSAGE_TYPES: &str = "MessagingRcsUnsupportedMessageTypes";
    // Media download.
    pub const MESSAGING_RCS_FILE_TRANSFER_MEDIA_IDS: &str = "messagingRcsFileTransferMediaIds";
    pub const REQUESTED_ITEMS: &str = "requestedItems";
    // Send.
    pub const ID: &str = "id";
    pub const TEMP_ID: &str = "tempId";
    pub const SUBSCRIPTION_ID: &str = "subscriptionId";
    pub const THREAD_ID: &str = "threadId";
    pub const RECIPIENTS: &str = "recipients";
    pub const BODY: &str = "body";
    pub const PART_COUNT: &str = "partCount";
    pub const PART_NAMES: &str = "partNames";
    pub const PART_CONTENT_TYPES: &str = "partContentTypes";
    pub const PART_BYTES: &str = "partBytes";
    pub const PART_BYTE_COUNTS: &str = "partByteCounts";
    pub const SEND_MESSAGE_FAILURE_REASON: &str = "sendMessageFailureReason";
    // Mark conversation as read.
    pub const TIMESTAMP: &str = "timestamp";
    // Connect response.
    pub const RESULT: &str = "result";
    pub const PERMISSIONS: &str = "permissions";
    pub const CAPABILITIES: &str = "capabilities";
    pub const SETTINGS: &str = "settings";
    pub const MESSAGING: &str = "messaging";
    pub const MMS_AUTO_DOWNLOAD_ENABLED: &str = "mmsAutoDownloadEnabled";
    // Metadata message and response.
    pub const METADATA: &str = "metadata";
    pub const CONTRACT_VERSION: &str = "contractVersion";
    pub const ITEM_COUNT: &str = "itemCount";
    pub const SMS_IDS: &str = "messagingSmsIds";
    pub const SMS_CHECKSUMS: &str = "messagingSmsChecksums";
    pub const UNREAD_SMS_IDS: &str = "messagingUnreadSmsIds";
    pub const MMS_IDS: &str = "messagingMmsIds";
    pub const MMS_CHECKSUMS: &str = "messagingMmsChecksums";
    pub const UNREAD_MMS_IDS: &str = "messagingUnreadMmsIds";
    pub const RCS_CHAT_IDS: &str = "messagingRcsChatIds";
    pub const RCS_CHAT_CHECKSUMS: &str = "messagingRcsChatChecksums";
    pub const RCS_FILE_TRANSFER_IDS: &str = "messagingRcsFileTransferIds";
    pub const RCS_FILE_TRANSFER_CHECKSUMS: &str = "messagingRcsFileTransferChecksums";
    pub const CONVERSATION_IDS: &str = "messagingConversationIds";
    pub const CONVERSATION_CHECKSUMS: &str = "messagingConversationChecksums";
    pub const RCS_CONVERSATION_IDS: &str = "messagingRcsConversationIds";
    pub const RCS_CONVERSATION_CHECKSUMS: &str = "messagingRcsConversationChecksums";
    pub const SUBSCRIPTION_IDS: &str = "messagingSubscriptionIds";
    pub const SUBSCRIPTION_CHECKSUMS: &str = "messagingSubscriptionChecksums";
    pub const MMS_PART_IDS: &str = "messagingMmsPartIds";
    pub const SEQUENCE_NUMBERS: &str = "sequenceNumbers";
    pub const ACKS: &str = "acks";
    // Item sub-objects.
    pub const SMS_MESSAGES: &str = "smsMessages";
    pub const MMS_MESSAGES: &str = "mmsMessages";
    pub const MMS_PARTS: &str = "mmsParts";
    pub const RCS_CHATS: &str = "rcsChats";
    pub const RCS_FILE_TRANSFERS: &str = "rcsFileTransfers";
    pub const CONVERSATIONS: &str = "conversations";
    pub const RCS_CONVERSATIONS: &str = "rcsConversations";
    pub const CANONICAL_ADDRESSES: &str = "canonicalAddresses";
    pub const SUBSCRIPTIONS: &str = "subscriptions";
    // Row arrays.
    pub const MESSAGE_IDS: &str = "messageIds";
    pub const THREAD_IDS: &str = "threadIds";
    pub const DATES: &str = "dates";
    pub const FROM_ADDRESSES: &str = "fromAddresses";
    pub const TO_ADDRESSES: &str = "toAddresses";
    pub const BODIES: &str = "bodies";
    pub const SUBJECTS: &str = "subjects";
    pub const SUBJECT_CHARSETS: &str = "subjectCharsets";
    pub const MESSAGE_TYPES: &str = "messageTypes";
    pub const MESSAGE_BOX_TYPES: &str = "messageBoxTypes";
    pub const READ_FLAGS: &str = "readFlags";
    pub const CHECKSUMS: &str = "checksums";
    pub const ACTIONS: &str = "actions";
    pub const SUBSCRIPTION_IDS_ROW: &str = "subscriptionIds";
    pub const GROUP_IDS: &str = "groupIds";
    pub const PART_IDS: &str = "partIds";
    pub const PART_SEQUENCE_NUMBERS: &str = "sequenceNumbers";
    pub const CONTENT_IDS: &str = "contentIds";
    pub const CONTENT_TYPES: &str = "contentTypes";
    pub const NAMES: &str = "names";
    pub const TEXTS: &str = "texts";
    pub const CHARSETS: &str = "charsets";
    pub const SIZES: &str = "sizes";
    pub const CONTENT_BYTE_SIZES: &str = "contentByteSizes";
    pub const CONTENT_BYTES: &str = "contentBytes";
    pub const SESSION_IDS: &str = "sessionIds";
    pub const RCS_MESSAGE_TYPES: &str = "rcsMessageTypes";
    pub const ROW_RECIPIENTS: &str = "recipients";
    pub const IS_USER_ACTION_REQUIRED: &str = "isUserActionRequired";
    pub const FILE_NAMES: &str = "fileNames";
    pub const FILE_SIZES: &str = "fileSizes";
    pub const FILE_BYTE_COUNTS: &str = "fileByteCounts";
    pub const FILE_BYTES: &str = "fileBytes";
    pub const IDS: &str = "ids";
    pub const RECIPIENT_IDS: &str = "recipientIds";
    pub const HAS_RCS: &str = "hasRcs";
    pub const ADDRESSES: &str = "addresses";
    pub const FALLBACK_IDS: &str = "fallbackIds";
    pub const RCS_CONVERSATION_TYPES: &str = "rcsConversationTypes";
    pub const SIM_SLOT_INDICES: &str = "simSlotIndices";
    pub const COUNTRY_ISOS: &str = "countryIsos";
    pub const NUMBERS: &str = "numbers";
    pub const IS_MMS_ENABLEDS: &str = "isMmsEnableds";
    pub const IS_GROUP_MMS_ENABLEDS: &str = "isGroupMmsEnableds";
    pub const IS_ROAMINGS: &str = "isRoamings";
    pub const ALLOW_AUDIO_ATTACHMENTS: &str = "allowAudioAttachments";
    pub const IS_MULTIPART_SMS_ENABLEDS: &str = "isMultipartSmsEnableds";
    pub const SEND_MULTIPART_SMS_AS_SEPARATE: &str = "shouldSendMultipartSmsAsSeparateMessages";
    pub const MAX_MESSAGE_SIZES: &str = "maxMessageSizes";
    pub const RECIPIENT_LIMITS: &str = "recipientLimits";
    pub const MAX_IMAGE_HEIGHTS: &str = "maxImageHeights";
    pub const MAX_IMAGE_WIDTHS: &str = "maxImageWidths";
    pub const MAX_MESSAGE_TEXT_LENGTHS: &str = "maxMessageTextLengths";
    pub const SMS_MULTIPART_TO_MMS_TEXT_THRESHOLDS: &str = "smsMultipartToMmsTextThresholds";
    pub const SMS_TO_MMS_TEXT_LENGTH_THRESHOLDS: &str = "smsToMmsTextLengthThresholds";
    pub const MAX_SUBJECT_LENGTHS: &str = "maxSubjectLengths";
    pub const IS_DEFAULT_DATA_SUBSCRIPTIONS: &str = "isDefaultDataSubscriptions";
    pub const IS_DEFAULT_SMS_SUBSCRIPTIONS: &str = "isDefaultSmsSubscriptions";
    pub const IS_DEFAULT_SUBSCRIPTIONS: &str = "isDefaultSubscriptions";
    pub const IS_DEFAULT_VOICE_SUBSCRIPTIONS: &str = "isDefaultVoiceSubscriptions";
    pub const IS_RCS_SUPPORTED: &str = "isRcsSupported";
    pub const MAX_RCS_MESSAGE_SIZE: &str = "maxRcsMessageSize";
    pub const MAX_RCS_FILE_SIZE: &str = "maxRcsFileSize";
    // `settings`.
    pub const SETTINGS_CAPABILITIES: &str = "capabilities";
    pub const MMS_AUTO_DOWNLOAD_AVAILABLE: &str = "mmsAutoDownloadAvailable";
    pub const RCS_SEND_AVAILABLE: &str = "rcsSendAvailable";
    pub const MARK_AS_READ_AVAILABLE: &str = "markAsReadAvailable";
}

/// `MessagingCapabilities` (Int32 flags).
pub mod caps {
    pub const MMS_AUTO_DOWNLOAD_ON_DEVICE: u32 = 0x1;
    pub const RCS_RECEIVE: u32 = 0x2;
    pub const RCS_SEND: u32 = 0x4;
    pub const MARK_CONVERSATION_AS_READ: u32 = 0x8;
    pub const SEND: u32 = 0x10;
    pub const PARTICIPANTS: u32 = 0x20;
    pub const MARK_MESSAGE_AS_READ: u32 = 0x40;
    pub const SEND_REQUIRES_PROXIMITY: u32 = 0x80;
}

/// RCS, mark-conversation-as-read and the "before any report" default require the phone's
/// contract version to be at least this.
pub const MIN_CONTRACT_VERSION: f64 = 3.9;

/// `MessageMailboxType` (`messageTypes`, `messageBoxTypes`).
pub mod mailbox {
    pub const UNKNOWN: i32 = -1;
    pub const INBOX: i32 = 1;
    pub const SENT: i32 = 2;
    pub const DRAFT: i32 = 3;
    pub const OUTBOX: i32 = 4;
    pub const FAILED: i32 = 5;
    pub const QUEUED: i32 = 6;
}

/// `SyncAction` (`actions`): only `Delete` changes what a row does.
pub mod sync_action {
    pub const SYNC: i32 = -1;
    pub const CREATE: i32 = 0;
    pub const UPDATE: i32 = 1;
    pub const DELETE: i32 = 2;
}

/// `RcsMessageType` (`rcsMessageTypes`).
pub mod rcs_message_type {
    pub const UNKNOWN: i32 = 0;
    /// `GroupChatEvent_Participant_Joined_Self` … `GroupChatEvent_Participant_Removed_By_Leader`
    /// (1–8): group events, not messages of the conversation.
    pub const FIRST_GROUP_EVENT: i32 = 1;
    pub const LAST_SHOWN_GROUP_EVENT: i32 = 8;
    pub const GROUP_CHAT_EVENT_GROUP_CLOSED: i32 = 9;
    pub const GROUP_CHAT_EVENT_PARTICIPANT_INVITED: i32 = 10;
}

/// `RcsConversationType` (`rcsConversationTypes`).
pub mod rcs_conversation_type {
    pub const NONE: i32 = 0;
    pub const ONE_TO_ONE: i32 = 1;
    pub const ACTIVE_GROUP_CHAT: i32 = 2;
    pub const INACTIVE_GROUP_CHAT: i32 = 3;
}

/// `configuration.MessagingRcsUnsupportedMessageTypes`: participant-invited, group-closed.
pub const RCS_UNSUPPORTED_MESSAGE_TYPES: [i32; 2] =
    [rcs_message_type::GROUP_CHAT_EVENT_PARTICIPANT_INVITED, rcs_message_type::GROUP_CHAT_EVENT_GROUP_CLOSED];

/// `requestType` values the app sends.
pub mod request_type {
    pub const SEND: i32 = 2;
    pub const MARK_CONVERSATION_AS_READ: i32 = 3;
}

/// The anchor (sequence-number) types, in the order the connect request writes them.
pub mod anchor {
    pub const SMS: &str = "sms";
    pub const MMS: &str = "mms";
    pub const CONVERSATION: &str = "conversation";
    pub const SUBSCRIPTION: &str = "subscription";
    pub const RCS_CHAT: &str = "rcs_chat";
    pub const RCS_FILE_TRANSFER: &str = "rcs_filetransfer";
    pub const RCS_CONVERSATION: &str = "rcs_conversation";
    /// Every type.
    pub const ALL: [&str; 7] = [SMS, MMS, CONVERSATION, SUBSCRIPTION, RCS_CHAT, RCS_FILE_TRANSFER, RCS_CONVERSATION];
    /// The types read and written only under RCS receive.
    pub const RCS: [&str; 3] = [RCS_CHAT, RCS_FILE_TRANSFER, RCS_CONVERSATION];
}

/// `-1`: "none" for thread, subscription, group and anchor values.
pub const NONE_ID: i64 = -1;

/// Addresses that end with this (case-insensitive) mark a Google RCS conversation and are never
/// participants.
pub const GOOGLE_RCS_SUFFIX: &str = "@rcs.google.com";

/// The representable range of SMS/MMS/RCS `dates` (Unix ms): 0001-01-01 … 9999-12-31T23:59:59.999Z.
pub const UNIX_MS_MIN: i64 = -62_135_596_800_000;
pub const UNIX_MS_MAX: i64 = 253_402_300_799_999;
/// Carrier-limit defaults when the phone reports `≤ 0` or no subscription applies (composition rule
/// 6).
pub mod carrier {
    pub const MAX_IMAGE_WIDTH: i32 = 1944;
    pub const MAX_IMAGE_HEIGHT: i32 = 2592;
    pub const MAX_MESSAGE_SIZE: i32 = 614_400;
}

/// At most this many attachments per message (rule 5).
pub const MAX_ATTACHMENTS: usize = 2;

/// The MIME type of MMS presentation parts, which are not shown as parts.
pub const SMIL_CONTENT_TYPE: &str = "application/smil";

/// `SendMessageFailureReason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SendMessageFailureReason {
    NoFailure,
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
    /// A value outside the enumeration, kept as received.
    Other(i32),
}

impl SendMessageFailureReason {
    pub fn from_i32(v: i32) -> Self {
        use SendMessageFailureReason::{
            ActionRequired, AirplaneMode, BluetoothNotConnected, Empty, MessageSizeExceeded, MmsAudioNotAllowed,
            MmsDisabled, MmsGroupMessageDisabled, MmsNotSupported, MmsSubjectSizeExceeded, MmsTooManyRecipients,
            MmsTransactionSizeExceeded, NoFailure, NoRecipients, NoService, NoSim, Other, PhoneTimeout, RadioOff,
            RcsSendUnavailable, Unknown,
        };
        match v {
            0 => NoFailure,
            1 => AirplaneMode,
            2 => NoSim,
            3 => MessageSizeExceeded,
            4 => MmsNotSupported,
            5 => MmsDisabled,
            6 => MmsGroupMessageDisabled,
            7 => MmsTooManyRecipients,
            8 => MmsSubjectSizeExceeded,
            9 => MmsAudioNotAllowed,
            10 => MmsTransactionSizeExceeded,
            11 => Unknown,
            12 => ActionRequired,
            13 => Empty,
            14 => RcsSendUnavailable,
            15 => PhoneTimeout,
            16 => RadioOff,
            17 => NoService,
            18 => NoRecipients,
            19 => BluetoothNotConnected,
            other => Other(other),
        }
    }

    pub fn as_i32(self) -> i32 {
        use SendMessageFailureReason::{
            ActionRequired, AirplaneMode, BluetoothNotConnected, Empty, MessageSizeExceeded, MmsAudioNotAllowed,
            MmsDisabled, MmsGroupMessageDisabled, MmsNotSupported, MmsSubjectSizeExceeded, MmsTooManyRecipients,
            MmsTransactionSizeExceeded, NoFailure, NoRecipients, NoService, NoSim, Other, PhoneTimeout, RadioOff,
            RcsSendUnavailable, Unknown,
        };
        match self {
            NoFailure => 0,
            AirplaneMode => 1,
            NoSim => 2,
            MessageSizeExceeded => 3,
            MmsNotSupported => 4,
            MmsDisabled => 5,
            MmsGroupMessageDisabled => 6,
            MmsTooManyRecipients => 7,
            MmsSubjectSizeExceeded => 8,
            MmsAudioNotAllowed => 9,
            MmsTransactionSizeExceeded => 10,
            Unknown => 11,
            ActionRequired => 12,
            Empty => 13,
            RcsSendUnavailable => 14,
            PhoneTimeout => 15,
            RadioOff => 16,
            NoService => 17,
            NoRecipients => 18,
            BluetoothNotConnected => 19,
            Other(v) => v,
        }
    }
}

#[cfg(test)]
mod tests;
