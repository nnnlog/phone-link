//! Storing and deleting the phone's message, part and subscription rows.

use super::to_json;
use crate::parse::{
    ConversationRow, MmsPartRow, MmsRow, RcsChatRow, RcsConversationRow, RcsFileTransferRow, SmsRow, SubscriptionRow,
};
use rusqlite::{Connection, params};

/// Inserts or replaces an SMS row (the item upsert rules): a stored thread id other than `-1` and a
/// stored group id other than `0` are kept, and a row read on the PC stays read on the PC.
pub fn upsert_sms(c: &Connection, r: &SmsRow) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO sms (id, checksum, thread_id, timestamp_ms, from_address, to_address, body, subject, mailbox,
                          phone_read, pc_read, subscription_id, group_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, ?11, ?12)
         ON CONFLICT (id) DO UPDATE SET
            checksum = excluded.checksum,
            thread_id = CASE WHEN sms.thread_id != -1 THEN sms.thread_id ELSE excluded.thread_id END,
            timestamp_ms = excluded.timestamp_ms, from_address = excluded.from_address,
            to_address = excluded.to_address, body = excluded.body, subject = excluded.subject,
            mailbox = excluded.mailbox, phone_read = excluded.phone_read,
            subscription_id = excluded.subscription_id,
            group_id = CASE WHEN sms.group_id != 0 THEN sms.group_id ELSE excluded.group_id END",
        params![
            r.id,
            r.checksum,
            r.thread_id,
            r.timestamp_ms,
            r.from_address,
            r.to_address,
            r.body,
            r.subject,
            r.mailbox,
            r.read,
            r.subscription_id,
            r.group_id
        ],
    )
    .map(|_| ())
}

/// Inserts or replaces an MMS row (the item upsert rules): a row read on the PC stays read.
pub fn upsert_mms(c: &Connection, r: &MmsRow) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO mms (id, checksum, thread_id, timestamp_ms, subject, subject_charset, mailbox, phone_read,
                          pc_read, subscription_id, from_address, to_addresses)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, ?10, ?11)
         ON CONFLICT (id) DO UPDATE SET
            checksum = excluded.checksum, thread_id = excluded.thread_id, timestamp_ms = excluded.timestamp_ms,
            subject = excluded.subject, subject_charset = excluded.subject_charset, mailbox = excluded.mailbox,
            phone_read = excluded.phone_read, subscription_id = excluded.subscription_id,
            from_address = excluded.from_address, to_addresses = excluded.to_addresses",
        params![
            r.id,
            r.checksum,
            r.thread_id,
            r.timestamp_ms,
            r.subject,
            r.subject_charset,
            r.mailbox,
            r.read,
            r.subscription_id,
            r.from_address,
            r.to_addresses.as_deref().map(to_json)
        ],
    )
    .map(|_| ())
}

/// Inserts or replaces an MMS part (the item upsert rules): a part delivered without bytes keeps
/// the bytes stored earlier.
pub fn upsert_mms_part(c: &Connection, p: &MmsPartRow) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO mms_part (part_id, message_id, sequence, content_id, content_type, name, text, charset, size, bytes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (part_id) DO UPDATE SET
            message_id = excluded.message_id, sequence = excluded.sequence, content_id = excluded.content_id,
            content_type = excluded.content_type, name = excluded.name, text = excluded.text,
            charset = excluded.charset, size = excluded.size, bytes = coalesce(excluded.bytes, mms_part.bytes)",
        params![
            p.part_id,
            p.message_id,
            p.sequence,
            p.content_id,
            p.content_type,
            p.name,
            p.text,
            p.charset,
            p.size,
            p.bytes
        ],
    )
    .map(|_| ())
}

/// Inserts or replaces an RCS chat message (the item upsert rules): a row read on the PC stays
/// read.
pub fn upsert_rcs_chat(c: &Connection, r: &RcsChatRow) -> rusqlite::Result<()> {
    let m = &r.common;
    c.execute(
        "INSERT INTO rcs_chat (id, checksum, thread_id, timestamp_ms, from_address, body, content_type, session_id,
                               mailbox, phone_read, pc_read, rcs_message_type, subscription_id, recipients,
                               user_action_required)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, ?11, ?12, ?13, ?14)
         ON CONFLICT (id) DO UPDATE SET
            checksum = excluded.checksum, thread_id = excluded.thread_id, timestamp_ms = excluded.timestamp_ms,
            from_address = excluded.from_address, body = excluded.body, content_type = excluded.content_type,
            session_id = excluded.session_id, mailbox = excluded.mailbox, phone_read = excluded.phone_read,
            rcs_message_type = excluded.rcs_message_type, subscription_id = excluded.subscription_id,
            recipients = excluded.recipients, user_action_required = excluded.user_action_required",
        params![
            m.id,
            m.checksum,
            m.thread_id,
            m.timestamp_ms,
            m.from_address,
            r.body,
            m.content_type,
            m.session_id,
            m.mailbox,
            m.read,
            m.rcs_message_type,
            m.subscription_id,
            m.recipients.as_deref().map(to_json),
            m.user_action_required
        ],
    )
    .map(|_| ())
}

/// Inserts or replaces an RCS file transfer whole (the item upsert rules): its bytes (none when
/// delivered without) and its PC read state (unread again) included.
pub fn upsert_rcs_file_transfer(c: &Connection, r: &RcsFileTransferRow) -> rusqlite::Result<()> {
    let m = &r.common;
    c.execute(
        "INSERT INTO rcs_file_transfer (id, checksum, thread_id, timestamp_ms, from_address, content_type, session_id,
                                        mailbox, phone_read, pc_read, rcs_message_type, subscription_id, recipients,
                                        user_action_required, file_name, file_size, bytes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
         ON CONFLICT (id) DO UPDATE SET
            checksum = excluded.checksum, thread_id = excluded.thread_id, timestamp_ms = excluded.timestamp_ms,
            from_address = excluded.from_address, content_type = excluded.content_type,
            session_id = excluded.session_id, mailbox = excluded.mailbox, phone_read = excluded.phone_read,
            pc_read = 0, rcs_message_type = excluded.rcs_message_type, subscription_id = excluded.subscription_id,
            recipients = excluded.recipients, user_action_required = excluded.user_action_required,
            file_name = excluded.file_name, file_size = excluded.file_size, bytes = excluded.bytes",
        params![
            m.id,
            m.checksum,
            m.thread_id,
            m.timestamp_ms,
            m.from_address,
            m.content_type,
            m.session_id,
            m.mailbox,
            m.read,
            m.rcs_message_type,
            m.subscription_id,
            m.recipients.as_deref().map(to_json),
            m.user_action_required,
            r.file_name,
            r.file_size,
            r.bytes
        ],
    )
    .map(|_| ())
}

pub fn upsert_conversation(c: &Connection, r: &ConversationRow) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO conversation (thread_id, checksum, recipients, has_rcs) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (thread_id) DO UPDATE SET checksum = excluded.checksum, recipients = excluded.recipients,
            has_rcs = excluded.has_rcs",
        params![r.thread_id, r.checksum, to_json(&r.recipients), r.has_rcs],
    )
    .map(|_| ())
}

pub fn upsert_rcs_conversation(c: &Connection, r: &RcsConversationRow) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO rcs_conversation (id, checksum, fallback_id, conversation_type, recipients)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (id) DO UPDATE SET checksum = excluded.checksum, fallback_id = excluded.fallback_id,
            conversation_type = excluded.conversation_type, recipients = excluded.recipients",
        params![r.id, r.checksum, r.fallback_id, r.conversation_type, to_json(&r.recipients)],
    )
    .map(|_| ())
}

// The subscription row is stored as JSON: only a few fields are read and none is queried by.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct StoredSubscription {
    sim_slot_index: i64,
    country_iso: String,
    name: String,
    number: String,
    is_roaming: bool,
    is_mms_enabled: bool,
    allow_audio_attachments: bool,
    is_multipart_sms_enabled: bool,
    is_group_mms_enabled: bool,
    send_multipart_sms_as_separate: bool,
    max_message_size: i32,
    recipient_limit: i32,
    max_image_height: i32,
    max_image_width: i32,
    max_message_text_length: i32,
    sms_multipart_to_mms_text_threshold: i32,
    sms_to_mms_text_length_threshold: i32,
    max_subject_length: i32,
    is_default_data: bool,
    is_default_sms: bool,
    is_default: bool,
    is_default_voice: bool,
    is_rcs_supported: bool,
    max_rcs_message_size: i64,
    max_rcs_file_size: i64,
}

pub fn upsert_subscription(c: &Connection, r: &SubscriptionRow) -> rusqlite::Result<()> {
    let stored = StoredSubscription {
        sim_slot_index: r.sim_slot_index,
        country_iso: r.country_iso.clone(),
        name: r.name.clone(),
        number: r.number.clone(),
        is_roaming: r.is_roaming,
        is_mms_enabled: r.is_mms_enabled,
        allow_audio_attachments: r.allow_audio_attachments,
        is_multipart_sms_enabled: r.is_multipart_sms_enabled,
        is_group_mms_enabled: r.is_group_mms_enabled,
        send_multipart_sms_as_separate: r.send_multipart_sms_as_separate,
        max_message_size: r.max_message_size,
        recipient_limit: r.recipient_limit,
        max_image_height: r.max_image_height,
        max_image_width: r.max_image_width,
        max_message_text_length: r.max_message_text_length,
        sms_multipart_to_mms_text_threshold: r.sms_multipart_to_mms_text_threshold,
        sms_to_mms_text_length_threshold: r.sms_to_mms_text_length_threshold,
        max_subject_length: r.max_subject_length,
        is_default_data: r.is_default_data,
        is_default_sms: r.is_default_sms,
        is_default: r.is_default,
        is_default_voice: r.is_default_voice,
        is_rcs_supported: r.is_rcs_supported,
        max_rcs_message_size: r.max_rcs_message_size,
        max_rcs_file_size: r.max_rcs_file_size,
    };

    let json = serde_json::to_string(&stored).expect("a subscription serializes");
    c.execute(
        "INSERT INTO subscription (id, checksum, row) VALUES (?1, ?2, ?3)
         ON CONFLICT (id) DO UPDATE SET checksum = excluded.checksum, row = excluded.row",
        params![r.id, r.checksum, json],
    )
    .map(|_| ())
}

/// Every stored subscription, by id.
pub fn subscriptions(c: &Connection) -> rusqlite::Result<Vec<SubscriptionRow>> {
    let mut stmt = c.prepare("SELECT id, checksum, row FROM subscription ORDER BY id")?;
    let rows =
        stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, String>(2)?)))?;

    let mut out = Vec::new();
    for row in rows {
        let (id, checksum, json) = row?;
        let s: StoredSubscription = serde_json::from_str(&json)
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, e.into()))?;

        out.push(SubscriptionRow {
            id,
            checksum: checksum.unwrap_or(0),
            sim_slot_index: s.sim_slot_index,
            country_iso: s.country_iso,
            name: s.name,
            number: s.number,
            is_roaming: s.is_roaming,
            is_mms_enabled: s.is_mms_enabled,
            allow_audio_attachments: s.allow_audio_attachments,
            is_multipart_sms_enabled: s.is_multipart_sms_enabled,
            is_group_mms_enabled: s.is_group_mms_enabled,
            send_multipart_sms_as_separate: s.send_multipart_sms_as_separate,
            max_message_size: s.max_message_size,
            recipient_limit: s.recipient_limit,
            max_image_height: s.max_image_height,
            max_image_width: s.max_image_width,
            max_message_text_length: s.max_message_text_length,
            sms_multipart_to_mms_text_threshold: s.sms_multipart_to_mms_text_threshold,
            sms_to_mms_text_length_threshold: s.sms_to_mms_text_length_threshold,
            max_subject_length: s.max_subject_length,
            is_default_data: s.is_default_data,
            is_default_sms: s.is_default_sms,
            is_default: s.is_default,
            is_default_voice: s.is_default_voice,
            is_rcs_supported: s.is_rcs_supported,
            max_rcs_message_size: s.max_rcs_message_size,
            max_rcs_file_size: s.max_rcs_file_size,
        });
    }
    Ok(out)
}
