//! The item messages.

use super::{AnchorUpdate, Parsed, acks, correlation_vector, object, opt, req, sequence_numbers};
use crate::caps::PhoneSettings;
use crate::consts::{UNIX_MS_MAX, UNIX_MS_MIN, key, sync_action};
use phonelink_app::Malformed;
use phonelink_core::codec::valueset::ValueSet;
use phonelink_core::time::FILETIME_ZERO_UNIX_MS;
use std::collections::HashMap;
#[cfg(feature = "test-support")]
use std::collections::HashSet;

/// SMS and MMS `dates`: Unix ms; a value outside the representable range or not after
/// 1601-01-01T00:00:00Z is stored as 1601-01-01T00:00:00Z.
pub fn clamp_message_date(ms: i64) -> i64 {
    if !(UNIX_MS_MIN..=UNIX_MS_MAX).contains(&ms) || ms <= FILETIME_ZERO_UNIX_MS { FILETIME_ZERO_UNIX_MS } else { ms }
}

/// RCS `dates`: converted without clamping; out of range ⇒ malformed.
pub(super) fn rcs_date(ms: i64, k: &'static str) -> Parsed<i64> {
    if (UNIX_MS_MIN..=UNIX_MS_MAX).contains(&ms) {
        Ok(ms)
    } else {
        Err(Malformed::new("messaging RCS date out of range", k))
    }
}

pub(super) fn split_list(s: &str, sep: char) -> Vec<String> {
    s.split(sep).filter(|p| !p.is_empty()).map(str::to_string).collect()
}

/// The rows of one row-bearing sub-object: inserts/replaces, deletes, and every processed id.
#[derive(Debug, Clone, PartialEq)]
pub struct Rows<T> {
    pub upserts: Vec<T>,
    pub deletes: Vec<i64>,
    /// Every processed id (also deletes), in row order — each leaves its pending set.
    pub processed: Vec<i64>,
}

impl<T> Default for Rows<T> {
    fn default() -> Self {
        Self { upserts: Vec::new(), deletes: Vec::new(), processed: Vec::new() }
    }
}

/// The row count of a sub-object whose rows run over the shortest present array: `lens` are the
/// present arrays' lengths, `absent` the number of absent optional arrays (each counts as
/// `itemCount` long). Fewer rows than `itemCount` ⇒ malformed.
pub(super) fn row_count(item_count: i32, lens: &[usize], absent: usize, k: &'static str) -> Parsed<usize> {
    let mut n = lens.iter().map(|l| *l as i64).min().unwrap_or(i64::MAX);
    if absent > 0 {
        n = n.min(i64::from(item_count));
    }
    if n < i64::from(item_count) {
        return Err(Malformed::new("messaging rows fewer than itemCount", k));
    }
    Ok(usize::try_from(n).unwrap_or(0))
}

/// `conversations`, `rcsConversations` and `subscriptions`: `Delete` deletes, any other value inserts
/// or replaces.
pub(super) fn is_delete(actions: Option<&[i32]>, i: usize) -> bool {
    actions.is_some_and(|a| a[i] == sync_action::DELETE)
}

/// `smsMessages`, `mmsMessages`, `rcsChats` and `rcsFileTransfers`: `Delete` deletes, `Sync`,
/// `Create` and `Update` (or no `actions`) insert or replace, and any other value is malformed.
pub(super) fn message_row_is_delete(actions: Option<&[i32]>, i: usize) -> Parsed<bool> {
    match actions.map(|a| a[i]) {
        None | Some(sync_action::SYNC | sync_action::CREATE | sync_action::UPDATE) => Ok(false),
        Some(sync_action::DELETE) => Ok(true),
        Some(_) => Err(Malformed::new("messaging action outside SyncAction", key::ACTIONS)),
    }
}

/// One SMS row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmsRow {
    pub id: i64,
    pub checksum: i64,
    pub thread_id: i64,
    pub timestamp_ms: i64,
    pub from_address: String,
    pub to_address: String,
    pub body: String,
    pub subject: String,
    pub mailbox: i32,
    pub read: bool,
    pub subscription_id: i64,
    pub group_id: i64,
}

pub(super) fn sms_rows(o: &ValueSet, item_count: i32) -> Parsed<Rows<SmsRow>> {
    let ids = req::<&[i64]>(o, key::MESSAGE_IDS)?;
    let threads = req::<&[i64]>(o, key::THREAD_IDS)?;
    let dates = req::<&[i64]>(o, key::DATES)?;
    let from = req::<&[String]>(o, key::FROM_ADDRESSES)?;
    let bodies = req::<&[String]>(o, key::BODIES)?;
    let subjects = req::<&[String]>(o, key::SUBJECTS)?;
    let types = req::<&[i32]>(o, key::MESSAGE_TYPES)?;
    let read = req::<&[bool]>(o, key::READ_FLAGS)?;
    let to = opt::<&[String]>(o, key::TO_ADDRESSES)?;
    let checksums = opt::<&[i64]>(o, key::CHECKSUMS)?;
    let actions = opt::<&[i32]>(o, key::ACTIONS)?;
    let subs = opt::<&[i64]>(o, key::SUBSCRIPTION_IDS_ROW)?;
    let groups = opt::<&[i64]>(o, key::GROUP_IDS)?;

    let mut lens = vec![ids.len(), threads.len(), dates.len(), from.len(), bodies.len()];
    lens.extend([subjects.len(), types.len(), read.len()]);
    let optional = [to.map(<[_]>::len), checksums.map(<[_]>::len), actions.map(<[_]>::len)];
    let optional2 = [subs.map(<[_]>::len), groups.map(<[_]>::len)];
    let absent = optional.iter().chain(&optional2).filter(|l| l.is_none()).count();
    lens.extend(optional.iter().chain(&optional2).flatten());
    let n = row_count(item_count, &lens, absent, key::SMS_MESSAGES)?;

    let mut rows = Rows::default();
    for i in 0..n {
        rows.processed.push(ids[i]);
        if message_row_is_delete(actions, i)? {
            rows.deletes.push(ids[i]);
            continue;
        }

        rows.upserts.push(SmsRow {
            id: ids[i],
            checksum: checksums.map_or(0, |c| c[i]),
            thread_id: threads[i],
            timestamp_ms: clamp_message_date(dates[i]),
            from_address: from[i].clone(),
            to_address: to.map_or_else(String::new, |t| t[i].clone()),
            body: bodies[i].clone(),
            subject: subjects[i].clone(),
            mailbox: types[i],
            read: read[i],
            subscription_id: subs.map_or(-1, |s| s[i]),
            group_id: groups.map_or(-1, |g| g[i]),
        });
    }

    Ok(rows)
}

/// One MMS row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MmsRow {
    pub id: i64,
    pub checksum: i64,
    pub thread_id: i64,
    pub timestamp_ms: i64,
    pub subject: String,
    pub subject_charset: i32,
    pub mailbox: i32,
    pub read: bool,
    pub subscription_id: i64,
    pub from_address: String,
    /// `None`: the item has no `toAddresses`.
    pub to_addresses: Option<Vec<String>>,
}

pub(super) fn mms_rows(o: &ValueSet, item_count: i32, participants: bool) -> Parsed<Rows<MmsRow>> {
    let ids = req::<&[i64]>(o, key::MESSAGE_IDS)?;
    let threads = req::<&[i64]>(o, key::THREAD_IDS)?;
    let dates = req::<&[i64]>(o, key::DATES)?;
    let subjects = req::<&[String]>(o, key::SUBJECTS)?;
    let charsets = req::<&[i32]>(o, key::SUBJECT_CHARSETS)?;
    let boxes = req::<&[i32]>(o, key::MESSAGE_BOX_TYPES)?;
    let read = req::<&[bool]>(o, key::READ_FLAGS)?;
    let subs = req::<&[i64]>(o, key::SUBSCRIPTION_IDS_ROW)?;
    let from = opt::<&[String]>(o, key::FROM_ADDRESSES)?;
    let to = opt::<&[String]>(o, key::TO_ADDRESSES)?;
    let checksums = opt::<&[i64]>(o, key::CHECKSUMS)?;
    let actions = opt::<&[i32]>(o, key::ACTIONS)?;

    let mut lens = vec![ids.len(), threads.len(), dates.len(), subjects.len(), charsets.len()];
    lens.extend([boxes.len(), read.len(), subs.len()]);
    let optional = [from.map(<[_]>::len), to.map(<[_]>::len), checksums.map(<[_]>::len), actions.map(<[_]>::len)];
    let absent = optional.iter().filter(|l| l.is_none()).count();
    lens.extend(optional.iter().flatten());
    let n = row_count(item_count, &lens, absent, key::MMS_MESSAGES)?;

    let mut rows = Rows::default();
    for i in 0..n {
        rows.processed.push(ids[i]);
        if message_row_is_delete(actions, i)? {
            rows.deletes.push(ids[i]);
            continue;
        }

        rows.upserts.push(MmsRow {
            id: ids[i],
            checksum: checksums.map_or(0, |c| c[i]),
            thread_id: threads[i],
            timestamp_ms: clamp_message_date(dates[i]),
            subject: subjects[i].clone(),
            subject_charset: charsets[i],
            mailbox: boxes[i],
            read: read[i],
            subscription_id: subs[i],
            from_address: from.map_or_else(String::new, |f| f[i].clone()),
            to_addresses: to.map(|t| split_list(&t[i], ',')),
        });
    }

    if participants && to.is_none() && !rows.upserts.is_empty() {
        return Err(Malformed::new(
            "messaging MMS without toAddresses while participants are tracked",
            key::TO_ADDRESSES,
        ));
    }
    Ok(rows)
}

/// One MMS part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MmsPartRow {
    pub part_id: i64,
    pub message_id: i64,
    pub sequence: i32,
    pub content_id: String,
    pub content_type: String,
    pub name: String,
    pub text: String,
    pub charset: i32,
    pub size: i64,
    /// The carried bytes, kept only while "receive MMS" is on.
    pub bytes: Option<Vec<u8>>,
}

pub(super) fn mms_part_rows(o: &ValueSet, item_count: i32, receive_mms: bool) -> Parsed<Vec<MmsPartRow>> {
    let message_ids = req::<&[i64]>(o, key::MESSAGE_IDS)?;
    let part_ids = req::<&[i64]>(o, key::PART_IDS)?;
    let seqs = req::<&[i32]>(o, key::PART_SEQUENCE_NUMBERS)?;
    let content_ids = req::<&[String]>(o, key::CONTENT_IDS)?;
    let content_types = req::<&[String]>(o, key::CONTENT_TYPES)?;
    let texts = req::<&[String]>(o, key::TEXTS)?;
    let names = req::<&[String]>(o, key::NAMES)?;
    let charsets = req::<&[i32]>(o, key::CHARSETS)?;
    let sizes = opt::<&[i64]>(o, key::SIZES)?;
    let byte_sizes = opt::<&[i32]>(o, key::CONTENT_BYTE_SIZES)?;
    let bytes = opt::<&[u8]>(o, key::CONTENT_BYTES)?.unwrap_or(&[]);

    let mut lens = vec![part_ids.len(), message_ids.len(), seqs.len(), content_ids.len(), content_types.len()];
    lens.extend([texts.len(), names.len(), charsets.len()]);
    let optional = [sizes.map(<[_]>::len), byte_sizes.map(<[_]>::len)];
    let absent = optional.iter().filter(|l| l.is_none()).count();
    lens.extend(optional.iter().flatten());
    let n = row_count(item_count, &lens, absent, key::MMS_PARTS)?;

    let mut offset = 0usize;
    let mut rows = Vec::with_capacity(n);
    for i in 0..n {
        let mut part_bytes = None;
        if let Some(len) = byte_sizes.map(|b| b[i]).filter(|l| *l >= 0) {
            if receive_mms {
                part_bytes = Some(slice(bytes, &mut offset, len, key::CONTENT_BYTES)?);
            }
        }

        rows.push(MmsPartRow {
            part_id: part_ids[i],
            message_id: message_ids[i],
            sequence: seqs[i],
            content_id: content_ids[i].clone(),
            content_type: content_types[i].clone(),
            name: names[i].clone(),
            text: texts[i].clone(),
            charset: charsets[i],
            size: sizes.map_or(0, |s| s[i]),
            bytes: part_bytes,
        });
    }

    Ok(rows)
}

/// The next `len` bytes at `offset`; too few ⇒ malformed.
pub(super) fn slice(bytes: &[u8], offset: &mut usize, len: i32, k: &'static str) -> Parsed<Vec<u8>> {
    let len = usize::try_from(len).unwrap_or(0);
    let end = offset.checked_add(len).filter(|e| *e <= bytes.len());
    let Some(end) = end else {
        return Err(Malformed::new("messaging bytes shorter than the sizes that claim them", k));
    };

    let out = bytes[*offset..end].to_vec();
    *offset = end;
    Ok(out)
}

/// One RCS chat message, or the common part of an RCS file transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RcsRow {
    pub id: i64,
    pub checksum: i64,
    pub thread_id: i64,
    pub timestamp_ms: i64,
    pub from_address: String,
    pub content_type: String,
    pub session_id: String,
    pub mailbox: i32,
    pub read: bool,
    pub rcs_message_type: i32,
    pub subscription_id: i64,
    /// `None`: the item has no `recipients`.
    pub recipients: Option<Vec<String>>,
    pub user_action_required: bool,
}

/// One RCS chat message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RcsChatRow {
    pub common: RcsRow,
    pub body: String,
}

/// One RCS file transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RcsFileTransferRow {
    pub common: RcsRow,
    pub file_name: String,
    pub file_size: i32,
    pub bytes: Option<Vec<u8>>,
}

/// The arrays `rcsChats` and `rcsFileTransfers` share.
pub(super) struct RcsArrays<'a> {
    ids: &'a [i64],
    threads: &'a [i64],
    dates: &'a [i64],
    from: &'a [String],
    content_types: &'a [String],
    session_ids: &'a [String],
    types: &'a [i32],
    read: &'a [bool],
    rcs_types: &'a [i32],
    subs: &'a [i64],
    actions: &'a [i32],
    recipients: Option<&'a [String]>,
    action_required: Option<&'a [bool]>,
    checksums: Option<&'a [i64]>,
}

impl<'a> RcsArrays<'a> {
    fn read(o: &'a ValueSet) -> Parsed<Self> {
        Ok(Self {
            ids: req::<&[i64]>(o, key::MESSAGE_IDS)?,
            threads: req::<&[i64]>(o, key::THREAD_IDS)?,
            dates: req::<&[i64]>(o, key::DATES)?,
            from: req::<&[String]>(o, key::FROM_ADDRESSES)?,
            content_types: req::<&[String]>(o, key::CONTENT_TYPES)?,
            session_ids: req::<&[String]>(o, key::SESSION_IDS)?,
            types: req::<&[i32]>(o, key::MESSAGE_TYPES)?,
            read: req::<&[bool]>(o, key::READ_FLAGS)?,
            rcs_types: req::<&[i32]>(o, key::RCS_MESSAGE_TYPES)?,
            subs: req::<&[i64]>(o, key::SUBSCRIPTION_IDS_ROW)?,
            actions: req::<&[i32]>(o, key::ACTIONS)?,
            recipients: opt::<&[String]>(o, key::ROW_RECIPIENTS)?,
            action_required: opt::<&[bool]>(o, key::IS_USER_ACTION_REQUIRED)?,
            checksums: opt::<&[i64]>(o, key::CHECKSUMS)?,
        })
    }

    /// The required arrays' lengths, the optional ones' and the count of absent optional arrays.
    fn lens(&self) -> (Vec<usize>, usize) {
        let mut lens = vec![self.ids.len(), self.threads.len(), self.dates.len(), self.from.len()];
        lens.extend([self.content_types.len(), self.session_ids.len(), self.types.len(), self.read.len()]);
        lens.extend([self.rcs_types.len(), self.subs.len(), self.actions.len()]);
        let optional =
            [self.recipients.map(<[_]>::len), self.action_required.map(<[_]>::len), self.checksums.map(<[_]>::len)];
        lens.extend(optional.iter().flatten());
        (lens, optional.iter().filter(|l| l.is_none()).count())
    }

    fn row(&self, i: usize) -> Parsed<RcsRow> {
        Ok(RcsRow {
            id: self.ids[i],
            checksum: self.checksums.map_or(0, |c| c[i]),
            thread_id: self.threads[i],
            timestamp_ms: rcs_date(self.dates[i], key::DATES)?,
            from_address: self.from[i].clone(),
            content_type: self.content_types[i].clone(),
            session_id: self.session_ids[i].clone(),
            mailbox: self.types[i],
            read: self.read[i],
            rcs_message_type: self.rcs_types[i],
            subscription_id: self.subs[i],
            recipients: self.recipients.map(|r| split_list(&r[i], ',')),
            user_action_required: self.action_required.is_some_and(|a| a[i]),
        })
    }
}

pub(super) fn rcs_chat_rows(o: &ValueSet, item_count: i32) -> Parsed<Rows<RcsChatRow>> {
    let a = RcsArrays::read(o)?;
    let bodies = req::<&[String]>(o, key::BODIES)?;

    let (mut lens, absent) = a.lens();
    lens.push(bodies.len());
    let n = row_count(item_count, &lens, absent, key::RCS_CHATS)?;

    let mut rows = Rows::default();
    for (i, body) in bodies.iter().enumerate().take(n) {
        rows.processed.push(a.ids[i]);
        if message_row_is_delete(Some(a.actions), i)? {
            rows.deletes.push(a.ids[i]);
            continue;
        }
        rows.upserts.push(RcsChatRow { common: a.row(i)?, body: body.clone() });
    }

    Ok(rows)
}

pub(super) fn rcs_file_transfer_rows(
    o: &ValueSet,
    item_count: i32,
    receive_mms: bool,
) -> Parsed<Rows<RcsFileTransferRow>> {
    let a = RcsArrays::read(o)?;
    let names = req::<&[String]>(o, key::FILE_NAMES)?;
    let sizes = req::<&[i32]>(o, key::FILE_SIZES)?;
    let byte_counts = req::<&[i32]>(o, key::FILE_BYTE_COUNTS)?;
    let bytes = req::<&[u8]>(o, key::FILE_BYTES)?;

    let (mut lens, absent) = a.lens();
    lens.extend([names.len(), sizes.len(), byte_counts.len()]);
    let n = row_count(item_count, &lens, absent, key::RCS_FILE_TRANSFERS)?;

    let mut offset = 0usize;
    let mut rows = Rows::default();
    for i in 0..n {
        rows.processed.push(a.ids[i]);
        if message_row_is_delete(Some(a.actions), i)? {
            // A `Delete` row never consumes bytes.
            rows.deletes.push(a.ids[i]);
            continue;
        }

        let common = a.row(i)?;
        let mut file_bytes = None;
        if byte_counts[i] >= 0 && receive_mms {
            file_bytes = Some(slice(bytes, &mut offset, byte_counts[i], key::FILE_BYTES)?);
        }

        rows.upserts.push(RcsFileTransferRow {
            common,
            file_name: names[i].clone(),
            file_size: sizes[i],
            bytes: file_bytes,
        });
    }

    Ok(rows)
}

/// `canonicalAddresses`: canonical-address id → address.
pub(super) fn canonical_addresses(item: &ValueSet) -> Parsed<HashMap<String, String>> {
    let Some(c) = object(item, key::CANONICAL_ADDRESSES) else {
        return Err(Malformed::new("messaging canonicalAddresses missing", key::CANONICAL_ADDRESSES));
    };

    let addresses = req::<&[String]>(c, key::ADDRESSES)?;
    let ids = req::<&[String]>(c, key::IDS)?;
    if addresses.len() < ids.len() {
        return Err(Malformed::new("messaging canonicalAddresses addresses shorter than ids", key::ADDRESSES));
    }

    let mut map = HashMap::with_capacity(ids.len());
    for (id, address) in ids.iter().zip(addresses) {
        if map.insert(id.clone(), address.clone()).is_some() {
            return Err(Malformed::new("messaging duplicate canonical-address id", key::IDS));
        }
    }

    Ok(map)
}

/// Resolves a space-separated list of canonical-address ids; an unknown id is skipped (logged).
pub(super) fn resolve(recipient_ids: &str, map: &HashMap<String, String>) -> Vec<String> {
    let mut out = Vec::new();
    for id in recipient_ids.split(' ').filter(|p| !p.is_empty()) {
        match map.get(id) {
            Some(a) => out.push(a.clone()),
            None => tracing::debug!("conversation recipient id without a canonical address skipped"),
        }
    }
    out
}

/// Every present array of an `ids`-iterated sub-object must be at least as long as `ids`.
pub(super) fn at_least(len: Option<usize>, ids: usize, k: &'static str) -> Parsed<()> {
    match len {
        Some(l) if l < ids => Err(Malformed::new("messaging array shorter than ids", k)),
        _ => Ok(()),
    }
}

/// One conversation (thread).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationRow {
    pub thread_id: i64,
    pub checksum: i64,
    pub recipients: Vec<String>,
    pub has_rcs: bool,
}

pub(super) fn conversation_rows(o: &ValueSet, item_count: i32, item: &ValueSet) -> Parsed<Rows<ConversationRow>> {
    let map = canonical_addresses(item)?;
    let recipient_ids = req::<&[String]>(o, key::RECIPIENT_IDS)?;
    let ids = req::<&[i64]>(o, key::IDS)?;
    let has_rcs = opt::<&[bool]>(o, key::HAS_RCS)?;
    let checksums = opt::<&[i64]>(o, key::CHECKSUMS)?;
    let actions = opt::<&[i32]>(o, key::ACTIONS)?;

    at_least(Some(recipient_ids.len()), ids.len(), key::RECIPIENT_IDS)?;
    at_least(has_rcs.map(<[_]>::len), ids.len(), key::HAS_RCS)?;
    at_least(checksums.map(<[_]>::len), ids.len(), key::CHECKSUMS)?;
    at_least(actions.map(<[_]>::len), ids.len(), key::ACTIONS)?;
    if (ids.len() as i64) < i64::from(item_count) {
        return Err(Malformed::new("messaging rows fewer than itemCount", key::CONVERSATIONS));
    }

    let mut rows = Rows::default();
    for (i, id) in ids.iter().enumerate() {
        rows.processed.push(*id);
        if is_delete(actions, i) {
            rows.deletes.push(*id);
            continue;
        }

        rows.upserts.push(ConversationRow {
            thread_id: *id,
            checksum: checksums.map_or(0, |c| c[i]),
            recipients: resolve(&recipient_ids[i], &map),
            has_rcs: has_rcs.is_some_and(|h| h[i]),
        });
    }

    Ok(rows)
}

/// One RCS conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RcsConversationRow {
    pub id: i64,
    pub checksum: i64,
    /// The SMS/MMS thread it merges with (absent ⇒ its own id).
    pub fallback_id: i64,
    pub conversation_type: i32,
    pub recipients: Vec<String>,
}

pub(super) fn rcs_conversation_rows(
    o: &ValueSet,
    item_count: i32,
    item: &ValueSet,
) -> Parsed<Rows<RcsConversationRow>> {
    let map = canonical_addresses(item)?;
    let recipient_ids = req::<&[String]>(o, key::RECIPIENT_IDS)?;
    let ids = req::<&[i64]>(o, key::IDS)?;
    let fallback = opt::<&[i64]>(o, key::FALLBACK_IDS)?;
    let checksums = opt::<&[i64]>(o, key::CHECKSUMS)?;
    let actions = opt::<&[i32]>(o, key::ACTIONS)?;
    let types = opt::<&[i32]>(o, key::RCS_CONVERSATION_TYPES)?;

    at_least(Some(recipient_ids.len()), ids.len(), key::RECIPIENT_IDS)?;
    at_least(fallback.map(<[_]>::len), ids.len(), key::FALLBACK_IDS)?;
    at_least(checksums.map(<[_]>::len), ids.len(), key::CHECKSUMS)?;
    at_least(actions.map(<[_]>::len), ids.len(), key::ACTIONS)?;
    at_least(types.map(<[_]>::len), ids.len(), key::RCS_CONVERSATION_TYPES)?;
    if (ids.len() as i64) < i64::from(item_count) {
        return Err(Malformed::new("messaging rows fewer than itemCount", key::RCS_CONVERSATIONS));
    }

    let mut rows = Rows::default();
    for (i, id) in ids.iter().enumerate() {
        rows.processed.push(*id);
        if is_delete(actions, i) {
            rows.deletes.push(*id);
            continue;
        }

        rows.upserts.push(RcsConversationRow {
            id: *id,
            checksum: checksums.map_or(0, |c| c[i]),
            fallback_id: fallback.map_or(*id, |f| f[i]),
            conversation_type: types.map_or(0, |t| t[i]),
            recipients: resolve(&recipient_ids[i], &map),
        });
    }

    Ok(rows)
}

/// One SIM subscription.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SubscriptionRow {
    pub id: i64,
    pub checksum: i64,
    pub sim_slot_index: i64,
    pub country_iso: String,
    pub name: String,
    pub number: String,
    pub is_roaming: bool,
    pub is_mms_enabled: bool,
    pub allow_audio_attachments: bool,
    pub is_multipart_sms_enabled: bool,
    pub is_group_mms_enabled: bool,
    pub send_multipart_sms_as_separate: bool,
    pub max_message_size: i32,
    pub recipient_limit: i32,
    pub max_image_height: i32,
    pub max_image_width: i32,
    pub max_message_text_length: i32,
    pub sms_multipart_to_mms_text_threshold: i32,
    pub sms_to_mms_text_length_threshold: i32,
    pub max_subject_length: i32,
    pub is_default_data: bool,
    pub is_default_sms: bool,
    pub is_default: bool,
    pub is_default_voice: bool,
    pub is_rcs_supported: bool,
    pub max_rcs_message_size: i64,
    pub max_rcs_file_size: i64,
}

pub(super) fn subscription_rows(o: &ValueSet, item_count: i32) -> Parsed<Rows<SubscriptionRow>> {
    let ids = req::<&[i64]>(o, key::IDS)?;
    let slots = req::<&[i64]>(o, key::SIM_SLOT_INDICES)?;
    let isos = req::<&[String]>(o, key::COUNTRY_ISOS)?;
    let names = req::<&[String]>(o, key::NAMES)?;
    let roaming = req::<&[bool]>(o, key::IS_ROAMINGS)?;
    let numbers = req::<&[String]>(o, key::NUMBERS)?;
    let mms = req::<&[bool]>(o, key::IS_MMS_ENABLEDS)?;
    let audio = req::<&[bool]>(o, key::ALLOW_AUDIO_ATTACHMENTS)?;
    let multipart = req::<&[bool]>(o, key::IS_MULTIPART_SMS_ENABLEDS)?;
    let group_mms = req::<&[bool]>(o, key::IS_GROUP_MMS_ENABLEDS)?;
    let separate = req::<&[bool]>(o, key::SEND_MULTIPART_SMS_AS_SEPARATE)?;
    let max_size = req::<&[i32]>(o, key::MAX_MESSAGE_SIZES)?;
    let recipient_limits = req::<&[i32]>(o, key::RECIPIENT_LIMITS)?;
    let max_h = req::<&[i32]>(o, key::MAX_IMAGE_HEIGHTS)?;
    let max_w = req::<&[i32]>(o, key::MAX_IMAGE_WIDTHS)?;
    let mp_threshold = req::<&[i32]>(o, key::SMS_MULTIPART_TO_MMS_TEXT_THRESHOLDS)?;
    let len_threshold = req::<&[i32]>(o, key::SMS_TO_MMS_TEXT_LENGTH_THRESHOLDS)?;
    let max_text = req::<&[i32]>(o, key::MAX_MESSAGE_TEXT_LENGTHS)?;
    let max_subject = req::<&[i32]>(o, key::MAX_SUBJECT_LENGTHS)?;
    let def_data = req::<&[bool]>(o, key::IS_DEFAULT_DATA_SUBSCRIPTIONS)?;
    let def_sms = req::<&[bool]>(o, key::IS_DEFAULT_SMS_SUBSCRIPTIONS)?;
    let def = req::<&[bool]>(o, key::IS_DEFAULT_SUBSCRIPTIONS)?;
    let def_voice = req::<&[bool]>(o, key::IS_DEFAULT_VOICE_SUBSCRIPTIONS)?;
    let rcs = opt::<&[bool]>(o, key::IS_RCS_SUPPORTED)?;
    let rcs_msg = opt::<&[i64]>(o, key::MAX_RCS_MESSAGE_SIZE)?;
    let rcs_file = opt::<&[i64]>(o, key::MAX_RCS_FILE_SIZE)?;
    let actions = opt::<&[i32]>(o, key::ACTIONS)?;
    let checksums = opt::<&[i64]>(o, key::CHECKSUMS)?;

    let mut lens = vec![ids.len(), slots.len(), isos.len(), names.len(), roaming.len(), numbers.len(), mms.len()];
    lens.extend([audio.len(), multipart.len(), group_mms.len(), separate.len(), max_size.len()]);
    lens.extend([recipient_limits.len(), max_h.len(), max_w.len(), mp_threshold.len(), len_threshold.len()]);
    lens.extend([max_text.len(), max_subject.len(), def_data.len(), def_sms.len(), def.len(), def_voice.len()]);
    let optional = [
        rcs.map(<[_]>::len),
        rcs_msg.map(<[_]>::len),
        rcs_file.map(<[_]>::len),
        actions.map(<[_]>::len),
        checksums.map(<[_]>::len),
    ];
    let absent = optional.iter().filter(|l| l.is_none()).count();
    lens.extend(optional.iter().flatten());
    let n = row_count(item_count, &lens, absent, key::SUBSCRIPTIONS)?;

    let mut rows = Rows::default();
    for i in 0..n {
        rows.processed.push(ids[i]);
        if is_delete(actions, i) {
            rows.deletes.push(ids[i]);
            continue;
        }

        rows.upserts.push(SubscriptionRow {
            id: ids[i],
            checksum: checksums.map_or(0, |c| c[i]),
            sim_slot_index: slots[i],
            country_iso: isos[i].clone(),
            name: names[i].clone(),
            number: numbers[i].clone(),
            is_roaming: roaming[i],
            is_mms_enabled: mms[i],
            allow_audio_attachments: audio[i],
            is_multipart_sms_enabled: multipart[i],
            is_group_mms_enabled: group_mms[i],
            send_multipart_sms_as_separate: separate[i],
            max_message_size: max_size[i],
            recipient_limit: recipient_limits[i],
            max_image_height: max_h[i],
            max_image_width: max_w[i],
            max_message_text_length: max_text[i],
            sms_multipart_to_mms_text_threshold: mp_threshold[i],
            sms_to_mms_text_length_threshold: len_threshold[i],
            max_subject_length: max_subject[i],
            is_default_data: def_data[i],
            is_default_sms: def_sms[i],
            is_default: def[i],
            is_default_voice: def_voice[i],
            is_rcs_supported: rcs.is_some_and(|r| r[i]),
            max_rcs_message_size: rcs_msg.map_or(0, |r| r[i]),
            max_rcs_file_size: rcs_file.map_or(0, |r| r[i]),
        });
    }

    Ok(rows)
}

/// What item parsing depends on (all evaluated after the item's own `settings`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemRules {
    pub rcs_receive: bool,
    pub participants: bool,
    /// The PC setting "receive MMS": whether part and file bytes are sliced and kept.
    pub receive_mms: bool,
}

/// A validated item message.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Item {
    pub settings: Option<PhoneSettings>,
    pub subscriptions: Option<Rows<SubscriptionRow>>,
    pub conversations: Option<Rows<ConversationRow>>,
    pub sms: Option<Rows<SmsRow>>,
    pub mms_parts: Option<Vec<MmsPartRow>>,
    pub mms: Option<Rows<MmsRow>>,
    pub rcs_conversations: Option<Rows<RcsConversationRow>>,
    pub rcs_chats: Option<Rows<RcsChatRow>>,
    pub rcs_file_transfers: Option<Rows<RcsFileTransferRow>>,
    pub acks: Option<Vec<i64>>,
    pub sequence_numbers: Option<AnchorUpdate>,
}

/// A row-bearing sub-object with a non-zero `itemCount` (`itemCount` 0 ⇒ the sub-object is skipped).
pub(super) fn rows_object<'a>(item: &'a ValueSet, k: &'static str) -> Parsed<Option<(&'a ValueSet, i32)>> {
    let Some(o) = object(item, k) else { return Ok(None) };
    let count = req::<i32>(o, key::ITEM_COUNT)?;
    Ok((count != 0).then_some((o, count)))
}

/// Validates an item message under `rules`; `settings` is the item's already-parsed `settings`.
pub fn item(payload: &ValueSet, settings: Option<PhoneSettings>, rules: ItemRules) -> Parsed<Item> {
    correlation_vector(payload)?;
    let mut out = Item { settings, ..Item::default() };

    if let Some((o, n)) = rows_object(payload, key::SUBSCRIPTIONS)? {
        out.subscriptions = Some(subscription_rows(o, n)?);
    }
    if let Some((o, n)) = rows_object(payload, key::CONVERSATIONS)? {
        out.conversations = Some(conversation_rows(o, n, payload)?);
    }
    if let Some((o, n)) = rows_object(payload, key::SMS_MESSAGES)? {
        out.sms = Some(sms_rows(o, n)?);
    }
    if let Some((o, n)) = rows_object(payload, key::MMS_PARTS)? {
        out.mms_parts = Some(mms_part_rows(o, n, rules.receive_mms)?);
    }
    if let Some((o, n)) = rows_object(payload, key::MMS_MESSAGES)? {
        out.mms = Some(mms_rows(o, n, rules.participants)?);
    }
    if rules.rcs_receive {
        if let Some((o, n)) = rows_object(payload, key::RCS_CONVERSATIONS)? {
            out.rcs_conversations = Some(rcs_conversation_rows(o, n, payload)?);
        }
        if let Some((o, n)) = rows_object(payload, key::RCS_CHATS)? {
            out.rcs_chats = Some(rcs_chat_rows(o, n)?);
        }
        if let Some((o, n)) = rows_object(payload, key::RCS_FILE_TRANSFERS)? {
            out.rcs_file_transfers = Some(rcs_file_transfer_rows(o, n, rules.receive_mms)?);
        }
    }

    out.acks = acks(payload)?;
    out.sequence_numbers = sequence_numbers(payload, rules.rcs_receive)?;
    Ok(out)
}

#[cfg(feature = "test-support")]
/// The ids of `rows` that a set of pending ids loses.
pub fn processed_ids<T>(rows: &Option<Rows<T>>) -> HashSet<i64> {
    rows.as_ref().map(|r| r.processed.iter().copied().collect()).unwrap_or_default()
}
