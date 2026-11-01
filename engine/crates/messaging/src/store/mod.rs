//! The messaging store: one SQLite database per phone holding the local state the protocol depends
//! on (the app's own persistent store; the delta, completion and anchor rules are implemented
//! explicitly).
//!
//! Every function takes a [`Connection`]; callers that change state pass the `Transaction` of
//! [`phonelink_features::FeatureDb::transaction`] so one inbound message is applied whole or not at all.

use rusqlite::{Connection, params};

mod anchors;
mod attributes;
mod conversation_views;
mod delta;
mod message_views;
mod participants;
mod pending;
mod rows;

#[cfg(feature = "test-support")]
pub use anchors::{AnchorRow, anchors};
pub use anchors::{complete_anchors, current_anchor, is_up_to_date, set_expected_anchor};
pub use attributes::{ConversationAttr, conversation_attr, set_conversation_attr};
pub use conversation_views::{
    ConversationCounts, LatestMessage, LatestTable, all_conversations, all_rcs_conversations, conversation,
    conversation_counts, last_message_subscription, latest_message, mark_read_on_pc, rcs_conversation,
    rcs_conversations_for, unified_thread_of_rcs,
};
#[cfg(feature = "test-support")]
pub use conversation_views::{StoredConversation, StoredRcsConversation};
pub use delta::{Table, delete_rows, delta, mark_phone_read_except};
pub use message_views::{
    ShownMms, ShownPart, ShownRcsFile, StoredRcsChat, StoredSms, mms_by_id, mms_part, mms_part_bytes, rcs_chats_in,
    rcs_file_bytes, rcs_file_transfer, shown_mms_in_thread, shown_parts, shown_rcs_files_in, sms_in_thread,
};
#[cfg(feature = "test-support")]
pub use message_views::{StoredMms, StoredRcsFileTransfer, mms_parts};
#[cfg(feature = "test-support")]
pub use message_views::{ids_and_checksums, mms_in_thread, rcs_file_transfers_in};
pub use participants::{
    clear_participants, participants, participants_empty, set_rcs_participants, upsert_participant,
};
#[cfg(feature = "test-support")]
pub use pending::ListedSentPart;
pub use pending::{
    ListedSend, PendingSend, SendOrigin, SendStatus, SentPart, delete_pending_sends, fail_interrupted_sends,
    insert_pending_send, listed_sends, pending_send, pending_sends, sent_part_bytes, set_send_status, shown_thread,
};
pub use rows::{
    subscriptions, upsert_conversation, upsert_mms, upsert_mms_part, upsert_rcs_chat, upsert_rcs_conversation,
    upsert_rcs_file_transfer, upsert_sms, upsert_subscription,
};

/// The schema, one migration per entry (`user_version` counts them).
pub const MIGRATIONS: &[&str] = &["
CREATE TABLE phone_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    capabilities INTEGER,
    mms_auto_download_on_phone INTEGER NOT NULL DEFAULT 0,
    receive_mms INTEGER NOT NULL DEFAULT 1,
    auto_download_mms INTEGER NOT NULL DEFAULT 1,
    send_mms INTEGER NOT NULL DEFAULT 1,
    pc_consent INTEGER NOT NULL DEFAULT 1,
    message_notifications INTEGER NOT NULL DEFAULT 1,
    setup_completed_once INTEGER NOT NULL DEFAULT 0,
    last_update_ms INTEGER
);
CREATE TABLE sms (
    id INTEGER PRIMARY KEY,
    checksum INTEGER,
    thread_id INTEGER NOT NULL,
    timestamp_ms INTEGER NOT NULL,
    from_address TEXT NOT NULL,
    to_address TEXT NOT NULL,
    body TEXT NOT NULL,
    subject TEXT NOT NULL,
    mailbox INTEGER NOT NULL,
    phone_read INTEGER NOT NULL,
    pc_read INTEGER NOT NULL DEFAULT 0,
    subscription_id INTEGER NOT NULL,
    group_id INTEGER NOT NULL
);
CREATE INDEX sms_thread ON sms (thread_id);
CREATE TABLE mms (
    id INTEGER PRIMARY KEY,
    checksum INTEGER,
    thread_id INTEGER NOT NULL,
    timestamp_ms INTEGER NOT NULL,
    subject TEXT NOT NULL,
    subject_charset INTEGER NOT NULL,
    mailbox INTEGER NOT NULL,
    phone_read INTEGER NOT NULL,
    pc_read INTEGER NOT NULL DEFAULT 0,
    subscription_id INTEGER NOT NULL,
    from_address TEXT NOT NULL,
    to_addresses TEXT
);
CREATE INDEX mms_thread ON mms (thread_id);
CREATE TABLE mms_part (
    part_id INTEGER PRIMARY KEY,
    message_id INTEGER NOT NULL,
    sequence INTEGER NOT NULL,
    content_id TEXT NOT NULL,
    content_type TEXT NOT NULL,
    name TEXT NOT NULL,
    text TEXT NOT NULL,
    charset INTEGER NOT NULL,
    size INTEGER NOT NULL,
    bytes BLOB
);
CREATE INDEX mms_part_message ON mms_part (message_id);
CREATE TABLE rcs_chat (
    id INTEGER PRIMARY KEY,
    checksum INTEGER,
    thread_id INTEGER NOT NULL,
    timestamp_ms INTEGER NOT NULL,
    from_address TEXT NOT NULL,
    body TEXT NOT NULL,
    content_type TEXT NOT NULL,
    session_id TEXT NOT NULL,
    mailbox INTEGER NOT NULL,
    phone_read INTEGER NOT NULL,
    pc_read INTEGER NOT NULL DEFAULT 0,
    rcs_message_type INTEGER NOT NULL,
    subscription_id INTEGER NOT NULL,
    recipients TEXT,
    user_action_required INTEGER NOT NULL
);
CREATE INDEX rcs_chat_thread ON rcs_chat (thread_id);
CREATE TABLE rcs_file_transfer (
    id INTEGER PRIMARY KEY,
    checksum INTEGER,
    thread_id INTEGER NOT NULL,
    timestamp_ms INTEGER NOT NULL,
    from_address TEXT NOT NULL,
    content_type TEXT NOT NULL,
    session_id TEXT NOT NULL,
    mailbox INTEGER NOT NULL,
    phone_read INTEGER NOT NULL,
    pc_read INTEGER NOT NULL DEFAULT 0,
    rcs_message_type INTEGER NOT NULL,
    subscription_id INTEGER NOT NULL,
    recipients TEXT,
    user_action_required INTEGER NOT NULL,
    file_name TEXT NOT NULL,
    file_size INTEGER NOT NULL,
    bytes BLOB
);
CREATE INDEX rcs_file_transfer_thread ON rcs_file_transfer (thread_id);
CREATE TABLE conversation (
    thread_id INTEGER PRIMARY KEY,
    checksum INTEGER,
    recipients TEXT NOT NULL,
    has_rcs INTEGER NOT NULL
);
CREATE TABLE rcs_conversation (
    id INTEGER PRIMARY KEY,
    checksum INTEGER,
    fallback_id INTEGER NOT NULL,
    conversation_type INTEGER NOT NULL,
    recipients TEXT NOT NULL
);
CREATE INDEX rcs_conversation_fallback ON rcs_conversation (fallback_id);
CREATE TABLE subscription (
    id INTEGER PRIMARY KEY,
    checksum INTEGER,
    row TEXT NOT NULL
);
CREATE TABLE participant (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    thread_id INTEGER NOT NULL,
    address TEXT NOT NULL,
    normalized TEXT NOT NULL,
    UNIQUE (thread_id, normalized)
);
CREATE TABLE anchor (
    content_type TEXT PRIMARY KEY,
    current INTEGER NOT NULL DEFAULT -1,
    expected INTEGER NOT NULL DEFAULT -1
);
CREATE TABLE pending_send (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    temp_id TEXT NOT NULL UNIQUE,
    thread_id INTEGER NOT NULL,
    created_ms INTEGER NOT NULL,
    origin INTEGER NOT NULL,
    status INTEGER NOT NULL,
    failure_reason INTEGER,
    body TEXT NOT NULL,
    recipients TEXT NOT NULL,
    subscription_id INTEGER NOT NULL
);
CREATE TABLE pending_send_part (
    send_id INTEGER NOT NULL REFERENCES pending_send (id) ON DELETE CASCADE,
    idx INTEGER NOT NULL,
    name TEXT NOT NULL,
    content_type TEXT NOT NULL,
    bytes BLOB NOT NULL,
    PRIMARY KEY (send_id, idx)
);
CREATE TABLE conversation_attr (
    thread_id INTEGER PRIMARY KEY,
    muted INTEGER NOT NULL DEFAULT 0,
    hidden INTEGER NOT NULL DEFAULT 0,
    selected_subscription INTEGER
);
"];

fn to_json(list: &[String]) -> String {
    serde_json::to_string(list).expect("a string list serializes")
}

fn from_json(s: &str) -> rusqlite::Result<Vec<String>> {
    serde_json::from_str(s)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, e.into()))
}

fn opt_from_json(s: Option<String>) -> rusqlite::Result<Option<Vec<String>>> {
    s.map(|s| from_json(&s)).transpose()
}

// ------------------------------------------------------------------------------------------------
// Phone state and PC settings
// ------------------------------------------------------------------------------------------------

/// Creates the phone-state row when missing; `true` when it was created (the store is new).
pub fn ensure_phone_state(c: &Connection) -> rusqlite::Result<bool> {
    Ok(c.execute("INSERT OR IGNORE INTO phone_state (id) VALUES (1)", [])? == 1)
}

/// The phone-state row: what the phone reported and the PC settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhoneState {
    /// `MessagingCapabilities`; `None` = not yet reported.
    pub capabilities: Option<u32>,
    pub mms_auto_download_on_phone: bool,
    pub settings: PcSettings,
    pub setup_completed_once: bool,
    /// Unix ms of the last completed exchange.
    pub last_update_ms: Option<i64>,
}

/// The PC settings of the local state the protocol depends on (all default on).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcSettings {
    pub receive_mms: bool,
    pub auto_download_mms: bool,
    pub send_mms: bool,
    /// The PC-side consent ("messages" on this PC).
    pub pc_consent: bool,
    pub message_notifications: bool,
}

impl Default for PcSettings {
    fn default() -> Self {
        Self {
            receive_mms: true,
            auto_download_mms: true,
            send_mms: true,
            pc_consent: true,
            message_notifications: true,
        }
    }
}

pub fn phone_state(c: &Connection) -> rusqlite::Result<PhoneState> {
    c.query_row(
        "SELECT capabilities, mms_auto_download_on_phone, receive_mms, auto_download_mms, send_mms, pc_consent,
                message_notifications, setup_completed_once, last_update_ms FROM phone_state WHERE id = 1",
        [],
        |r| {
            Ok(PhoneState {
                capabilities: r.get::<_, Option<i64>>(0)?.map(|v| v as u32),
                mms_auto_download_on_phone: r.get(1)?,
                settings: PcSettings {
                    receive_mms: r.get(2)?,
                    auto_download_mms: r.get(3)?,
                    send_mms: r.get(4)?,
                    pc_consent: r.get(5)?,
                    message_notifications: r.get(6)?,
                },
                setup_completed_once: r.get(7)?,
                last_update_ms: r.get(8)?,
            })
        },
    )
}

pub fn set_capabilities(c: &Connection, caps: u32) -> rusqlite::Result<()> {
    c.execute("UPDATE phone_state SET capabilities = ?1 WHERE id = 1", [i64::from(caps)]).map(|_| ())
}

pub fn set_mms_auto_download_on_phone(c: &Connection, on: bool) -> rusqlite::Result<()> {
    c.execute("UPDATE phone_state SET mms_auto_download_on_phone = ?1 WHERE id = 1", [on]).map(|_| ())
}

pub fn set_pc_settings(c: &Connection, s: PcSettings) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE phone_state SET receive_mms = ?1, auto_download_mms = ?2, send_mms = ?3, pc_consent = ?4,
                message_notifications = ?5 WHERE id = 1",
        params![s.receive_mms, s.auto_download_mms, s.send_mms, s.pc_consent, s.message_notifications],
    )
    .map(|_| ())
}

pub fn set_setup_completed_once(c: &Connection) -> rusqlite::Result<()> {
    c.execute("UPDATE phone_state SET setup_completed_once = 1 WHERE id = 1", []).map(|_| ())
}

pub fn set_last_update(c: &Connection, ms: i64) -> rusqlite::Result<()> {
    c.execute("UPDATE phone_state SET last_update_ms = ?1 WHERE id = 1", [ms]).map(|_| ())
}

#[cfg(test)]
mod tests;
