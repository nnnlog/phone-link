//! Inbound messaging content: the exchange model, metadata messages with their delta computation
//! and response, item messages with their processing and response, the sync anchors, participants,
//! and error handling.
//!
//! One [`Exchange`] is the App layer's content processor for one transaction key: a metadata
//! message starts it (its requested ids become the pending sets), item messages continue it, and it
//! completes when the pending sets of every type but subscriptions are empty. Each message is
//! parsed and validated first ([`super::parse`]) and applied in one store transaction; a malformed
//! message changes nothing and is answered by the shared rule for malformed content (`{result: 1}`,
//! exchange discarded).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use rusqlite::Connection;
use tracing::info;

use phonelink_app::{ContentProcessor, ContentProcessorFactory, InboundContext, PhoneRecord, Processed, Rejection};
use phonelink_core::codec::valueset::ValueSet;
use phonelink_core::types::DcgClient;
use phonelink_features::shared::db::StoreError;
use phonelink_features::shared::phone_number;

use super::caps::Predicates;
use super::consts::{CONTENT_TYPE, GOOGLE_RCS_SUFFIX, anchor, key, mailbox};
use super::notify;
use super::parse::{self, AnchorUpdate, Item, ItemRules, MetadataRules, MmsRow, SmsRow, SubscriptionRow};
use super::store::{self, Table};
use super::{Inner, MessagingEvent};
use phonelink_app::Malformed;

/// Creates an empty [`Exchange`] for every message that opens a transaction.
pub(super) struct Factory {
    inner: Arc<Inner>,
}

impl Factory {
    pub(super) fn new(inner: &Arc<Inner>) -> Self {
        Self { inner: Arc::clone(inner) }
    }
}

impl ContentProcessorFactory for Factory {
    fn create(
        &self,
        ctx: &InboundContext<'_>,
        _payload: &ValueSet,
    ) -> Result<Option<Box<dyn ContentProcessor>>, Rejection> {
        Ok(Some(Box::new(Exchange::new(Arc::clone(&self.inner), ctx.phone.clone()))))
    }
}

/// The pending ids of an exchange, one set per type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Pending {
    sms: HashSet<i64>,
    mms: HashSet<i64>,
    rcs_chats: HashSet<i64>,
    rcs_file_transfers: HashSet<i64>,
    conversations: HashSet<i64>,
    rcs_conversations: HashSet<i64>,
    subscriptions: HashSet<i64>,
}

impl Pending {
    /// Complete when every set but subscriptions is empty.
    fn is_complete(&self) -> bool {
        self.sms.is_empty()
            && self.mms.is_empty()
            && self.rcs_chats.is_empty()
            && self.rcs_file_transfers.is_empty()
            && self.conversations.is_empty()
            && self.rcs_conversations.is_empty()
    }
}

fn leave<T>(set: &mut HashSet<i64>, rows: &Option<parse::Rows<T>>) {
    if let Some(r) = rows {
        for id in &r.processed {
            set.remove(id);
        }
    }
}

/// The exchange's anchor set: one value per type of [`anchor::ALL`], `-1` = none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Anchors([i64; 7]);

impl Anchors {
    const NONE: Anchors = Anchors([-1; 7]);

    /// A received `sequenceNumbers` object replaces the set (RCS types untouched without RCS receive).
    fn apply(&mut self, u: &AnchorUpdate) {
        self.0[..4].copy_from_slice(&u.base);
        if let Some(rcs) = u.rcs {
            self.0[4..].copy_from_slice(&rcs);
        }
    }

    /// The types that are not "none", with their values.
    fn set(&self) -> Vec<(&'static str, i64)> {
        anchor::ALL.iter().zip(self.0).filter(|(_, v)| *v != -1).map(|(t, v)| (*t, v)).collect()
    }
}

/// Why processing failed.
enum Fail {
    Malformed(Malformed),
    Store(StoreError),
}

impl From<Malformed> for Fail {
    fn from(m: Malformed) -> Self {
        Fail::Malformed(m)
    }
}

impl From<rusqlite::Error> for Fail {
    fn from(e: rusqlite::Error) -> Self {
        Fail::Store(e.into())
    }
}

impl From<StoreError> for Fail {
    fn from(e: StoreError) -> Self {
        Fail::Store(e)
    }
}

impl Fail {
    /// The rejection of a message (`{result: 1}`), which the App layer logs with its key or the
    /// store's error.
    fn rejection(self) -> Rejection {
        match self {
            Fail::Malformed(m) => Rejection::new(m.reason).at_key(m.key),
            Fail::Store(e) => Rejection::new("messaging store failure").because(e),
        }
    }
}

/// What a processed message changes besides the store, applied after the commit.
#[derive(Default)]
struct Effects {
    /// `(before, after)` when the message changed the `MessagingCapabilities` in force.
    capabilities: Option<(Predicates, Predicates)>,
    /// Thread ids (as the UI shows them) whose rows changed.
    threads: BTreeSet<i64>,
    /// Something changed that the thread list does not name (deletes, the metadata delta).
    everything: bool,
    acked: Vec<i64>,
    completed: bool,
    /// The ids a metadata message requested.
    requested_count: usize,
    notifications: Vec<notify::Candidate>,
}

/// One messaging exchange.
pub(super) struct Exchange {
    inner: Arc<Inner>,
    phone: DcgClient,
    pending: Pending,
    anchors: Anchors,
}

/// The state a message is processed against.
struct Context {
    record: PhoneRecord,
    before: Predicates,
    after: Predicates,
    settings: Option<super::caps::PhoneSettings>,
    state: store::PhoneState,
}

impl Exchange {
    fn new(inner: Arc<Inner>, phone: DcgClient) -> Self {
        Self { inner, phone, pending: Pending::default(), anchors: Anchors::NONE }
    }

    /// The capabilities before and after the message's own `settings`.
    fn context(&self, ctx: &InboundContext<'_>, payload: &ValueSet) -> Result<Context, Fail> {
        let Some(record) = ctx.app.phone_record() else {
            return Err(Fail::Store(StoreError::Corrupt("no record for the selected phone".into())));
        };

        let state = self.inner.db(&self.phone)?.read(store::phone_state)?;
        let contract = self.inner.contract_version(&record);
        let caps_before = self.inner.effective_caps(&self.phone, state.capabilities, &record);
        let settings = parse::settings(payload)?;
        let caps_after = settings.map_or(caps_before, |s| s.stored_capabilities());

        Ok(Context {
            before: Predicates::new(caps_before, contract),
            after: Predicates::new(caps_after, contract),
            record,
            settings,
            state,
        })
    }

    fn apply_settings(c: &Connection, cx: &Context, effects: &mut Effects) -> rusqlite::Result<()> {
        if let Some(s) = cx.settings {
            store::set_capabilities(c, s.stored_capabilities())?;
            store::set_mms_auto_download_on_phone(c, s.mms_auto_download_enabled)?;
            if cx.before.capabilities != cx.after.capabilities
                || cx.state.mms_auto_download_on_phone != s.mms_auto_download_enabled
            {
                effects.capabilities = Some((cx.before, cx.after));
            }
        }
        Ok(())
    }

    /// On completion, the anchor set replaces the stored `current` anchors and the
    /// time of the last messaging update is recorded.
    fn complete(&self, c: &Connection, anchors: &Anchors) -> rusqlite::Result<()> {
        store::complete_anchors(c, &anchors.set())?;
        store::set_last_update(c, self.inner.now_ms())
    }

    // --------------------------------------------------------------------------------------------
    // Metadata
    // --------------------------------------------------------------------------------------------

    fn process_metadata(&mut self, ctx: &InboundContext<'_>, payload: &ValueSet) -> Result<(ValueSet, Effects), Fail> {
        let cx = self.context(ctx, payload)?;
        let db = self.inner.db(&self.phone)?;
        let mut effects = Effects { everything: true, ..Effects::default() };

        let (answer, pending, anchors) = db.transaction(|tx| -> Result<_, Fail> {
            let forced = cx.after.participants() && store::participants_empty(tx)?;
            let rules = MetadataRules {
                rcs_receive_before: cx.before.rcs_receive(),
                rcs_receive_after: cx.after.rcs_receive(),
                forced_resync: forced,
            };

            let md = parse::metadata(payload, cx.settings, rules)?;
            Self::apply_settings(tx, &cx, &mut effects)?;

            if !cx.after.participants() {
                store::clear_participants(tx)?;
            }

            let rcs = cx.after.rcs_receive();
            let sms = sync_messages(tx, Table::Sms, &md.sms, forced)?;
            let mms = sync_messages(tx, Table::Mms, &md.mms, forced)?;
            let rcs_chats = sync_rcs(tx, Table::RcsChat, &md.rcs_chats, forced, rcs)?;
            let rcs_files = sync_rcs(tx, Table::RcsFileTransfer, &md.rcs_file_transfers, forced, rcs)?;
            let conversations =
                store::delta(tx, Table::Conversation, md.conversations.ids(), md.conversations.checksums.as_deref())?;
            let rcs_conversations = if rcs {
                checked_delta(tx, Table::RcsConversation, &md.rcs_conversations)?
            } else {
                store::delta(tx, Table::RcsConversation, &[], None)?
            };
            let subscriptions =
                store::delta(tx, Table::Subscription, md.subscriptions.ids(), md.subscriptions.checksums.as_deref())?;

            let mut anchors = Anchors::NONE;
            if let Some(u) = &md.sequence_numbers {
                anchors.apply(u);
            }

            if let Some(ids) = &md.acks {
                effects.acked = store::delete_pending_sends(tx, ids)?;
            }

            let total = sms.len()
                + mms.len()
                + rcs_chats.len()
                + rcs_files.len()
                + conversations.len()
                + rcs_conversations.len()
                + subscriptions.len();
            let mut answer = ValueSet::new()
                .with(key::CORRELATION_VECTOR, md.correlation_vector.clone())
                .with(key::CONTENT_TYPE, CONTENT_TYPE)
                .with(key::CONTRACT_VERSION, ctx.app.standard_keys().contract_version)
                .with(key::ITEM_COUNT, i32::try_from(total).unwrap_or(i32::MAX))
                .with(key::SMS_IDS, sms.clone())
                .with(key::MMS_IDS, mms.clone())
                .with(key::MMS_PART_IDS, Vec::<i64>::new())
                .with(key::CONVERSATION_IDS, conversations.clone())
                .with(key::SUBSCRIPTION_IDS, subscriptions.clone())
                .with(key::RESULT, 0i32);
            if rcs {
                answer.insert(key::RCS_CHAT_IDS, rcs_chats.clone());
                // The requested file-transfer ids go under the checksum-named key, as Phone Link on
                // Windows writes them.
                answer.insert(key::RCS_FILE_TRANSFER_CHECKSUMS, rcs_files.clone());
                answer.insert(key::RCS_CONVERSATION_IDS, rcs_conversations.clone());
            }

            let pending = Pending {
                sms: sms.into_iter().collect(),
                mms: mms.into_iter().collect(),
                rcs_chats: rcs_chats.into_iter().collect(),
                rcs_file_transfers: rcs_files.into_iter().collect(),
                conversations: conversations.into_iter().collect(),
                rcs_conversations: rcs_conversations.into_iter().collect(),
                subscriptions: subscriptions.into_iter().collect(),
            };

            effects.requested_count = total;
            // An empty request completes the exchange at once.
            if total == 0 {
                self.complete(tx, &anchors)?;
                effects.completed = true;
            }

            Ok((answer, pending, anchors))
        })?;

        self.pending = pending;
        self.anchors = anchors;
        Ok((answer, effects))
    }

    // --------------------------------------------------------------------------------------------
    // Items
    // --------------------------------------------------------------------------------------------

    fn process_item(&mut self, ctx: &InboundContext<'_>, payload: &ValueSet) -> Result<Effects, Fail> {
        let cx = self.context(ctx, payload)?;
        let rules = ItemRules {
            rcs_receive: cx.after.rcs_receive(),
            participants: cx.after.participants(),
            receive_mms: cx.state.settings.receive_mms,
        };
        let item = parse::item(payload, cx.settings, rules)?;

        let db = self.inner.db(&self.phone)?;
        let phone_country = phone_number::phone_country(&cx.record, self.inner.host.region_code().as_deref());
        let now_ms = self.inner.now_ms();
        let recency_ms = i64::try_from(self.inner.config.notification_recency.as_millis()).unwrap_or(i64::MAX);
        let mut effects = Effects::default();

        let (pending, anchors) = db.transaction(|tx| -> Result<_, Fail> {
            Self::apply_settings(tx, &cx, &mut effects)?;
            let mut pending = self.pending.clone();
            self.apply_item(tx, &item, &cx, phone_country.as_deref(), &mut pending, &mut effects)?;

            if let Some(ids) = &item.acks {
                effects.acked = store::delete_pending_sends(tx, ids)?;
            }

            let mut anchors = self.anchors;
            if let Some(u) = &item.sequence_numbers {
                anchors.apply(u);
            }

            if pending.is_complete() {
                self.complete(tx, &anchors)?;
                effects.completed = true;
            }

            let families = |t: &str| self.inner.edges.file_types.family(t.to_string());
            effects.notifications = notify::candidates(tx, &item, now_ms, recency_ms, cx.after, &families)?;
            Ok((pending, anchors))
        })?;

        self.pending = pending;
        self.anchors = anchors;
        Ok(effects)
    }

    /// Item processing, in order: subscriptions, conversations, SMS, MMS parts, MMS, then (RCS
    /// receive) RCS conversations, RCS chats, RCS file transfers.
    fn apply_item(
        &self,
        tx: &Connection,
        item: &Item,
        cx: &Context,
        phone_country: Option<&str>,
        pending: &mut Pending,
        effects: &mut Effects,
    ) -> Result<(), Fail> {
        let participants = cx.after.participants();

        if let Some(rows) = &item.subscriptions {
            store::delete_rows(tx, Table::Subscription, &rows.deletes)?;
            for r in &rows.upserts {
                store::upsert_subscription(tx, r)?;
            }
            effects.everything |= !rows.processed.is_empty();
        }
        leave(&mut pending.subscriptions, &item.subscriptions);

        if let Some(rows) = &item.conversations {
            store::delete_rows(tx, Table::Conversation, &rows.deletes)?;
            for r in &rows.upserts {
                store::upsert_conversation(tx, r)?;
                effects.threads.insert(r.thread_id);
            }
            effects.everything |= !rows.deletes.is_empty();
        }
        leave(&mut pending.conversations, &item.conversations);

        let subscriptions = store::subscriptions(tx)?;
        if let Some(rows) = &item.sms {
            store::delete_rows(tx, Table::Sms, &rows.deletes)?;
            for r in &rows.upserts {
                store::upsert_sms(tx, r)?;
                effects.threads.insert(r.thread_id);
            }
            if participants && !rows.upserts.is_empty() {
                sms_participants(tx, &rows.upserts, &subscriptions, phone_country)?;
            }
            effects.everything |= !rows.deletes.is_empty();
        }
        leave(&mut pending.sms, &item.sms);

        if let Some(parts) = &item.mms_parts {
            for p in parts {
                store::upsert_mms_part(tx, p)?;
                if let Some(m) = store::mms_by_id(tx, p.message_id)? {
                    effects.threads.insert(m.row.thread_id);
                }
            }
        }

        if let Some(rows) = &item.mms {
            store::delete_rows(tx, Table::Mms, &rows.deletes)?;
            for r in &rows.upserts {
                store::upsert_mms(tx, r)?;
                effects.threads.insert(r.thread_id);
            }
            if participants && !rows.upserts.is_empty() {
                mms_participants(tx, &rows.upserts, &subscriptions, phone_country)?;
            }
            effects.everything |= !rows.deletes.is_empty();
        }
        leave(&mut pending.mms, &item.mms);

        if cx.after.rcs_receive() {
            apply_rcs_items(tx, item, participants, pending, effects)?;
        }
        Ok(())
    }

    /// Store-independent effects after a commit: capability change and sync trigger (b), events,
    /// the "message arrives in the open conversation" mark-as-read trigger, notifications, the
    /// up-to-date state.
    fn after(&self, ctx: &InboundContext<'_>, effects: Effects) {
        let inner = &self.inner;

        if let Some((before, after)) = effects.capabilities {
            inner.after_capabilities_change(&self.phone, before, after);
        }
        for id in &effects.acked {
            inner.emit(MessagingEvent::SendChanged { phone: self.phone.clone(), send_id: *id });
        }

        if effects.everything || !effects.threads.is_empty() {
            let threads = if effects.everything { Vec::new() } else { effects.threads.iter().copied().collect() };
            inner.emit(MessagingEvent::ContentChanged { phone: self.phone.clone(), threads });
        }

        if !effects.threads.is_empty() {
            inner.on_synced_threads(&self.phone, &effects.threads);
        }
        if !effects.notifications.is_empty() {
            notify::post(inner, &self.phone, effects.notifications, ctx.app.is_foreground());
        }

        if effects.completed {
            info!(sync_id = ?ctx.sync_id, "messaging sync finished");
            inner.refresh_sync_state(&self.phone);
        }
    }
}

/// SMS/MMS of a metadata message: a forced resync requests every id and deletes nothing (and,
/// without checksums, marks read every local row not listed unread); otherwise the checksum delta.
fn sync_messages(tx: &Connection, table: Table, lists: &parse::TypeLists, forced: bool) -> Result<Vec<i64>, Fail> {
    if forced {
        if lists.checksums.is_none() {
            if let Some(unread) = &lists.unread {
                store::mark_phone_read_except(tx, table, unread)?;
            }
        }
        return Ok(lists.ids().to_vec());
    }

    match lists.checksums.as_deref() {
        Some(checksums) => Ok(store::delta(tx, table, lists.ids(), Some(checksums))?),
        None => Err(Fail::Malformed(Malformed {
            reason: "messaging checksums missing outside a forced resync",
            key: key::SMS_CHECKSUMS,
        })),
    }
}

/// RCS chats and file transfers of a metadata message: without RCS receive every local row is
/// deleted and none requested; a forced resync requests every id; otherwise the checksum delta.
fn sync_rcs(
    tx: &Connection,
    table: Table,
    lists: &parse::TypeLists,
    forced: bool,
    rcs: bool,
) -> Result<Vec<i64>, Fail> {
    if !rcs {
        return Ok(store::delta(tx, table, &[], None)?);
    }

    if forced {
        return Ok(lists.ids().to_vec());
    }
    checked_delta(tx, table, lists)
}

/// The checksum delta for a type whose checksums are required when its ids are non-empty (validated).
fn checked_delta(tx: &Connection, table: Table, lists: &parse::TypeLists) -> Result<Vec<i64>, Fail> {
    if lists.ids().is_empty() {
        return Ok(store::delta(tx, table, &[], None)?);
    }
    match lists.checksums.as_deref() {
        Some(checksums) => Ok(store::delta(tx, table, lists.ids(), Some(checksums))?),
        None => Err(Fail::Malformed(Malformed { reason: "messaging checksums missing for non-empty ids", key: "ids" })),
    }
}

/// RCS item processing, in order: RCS conversations, RCS chats, RCS file transfers.
fn apply_rcs_items(
    tx: &Connection,
    item: &Item,
    participants: bool,
    pending: &mut Pending,
    effects: &mut Effects,
) -> Result<(), Fail> {
    if let Some(rows) = &item.rcs_conversations {
        store::delete_rows(tx, Table::RcsConversation, &rows.deletes)?;
        for r in &rows.upserts {
            store::upsert_rcs_conversation(tx, r)?;
            if participants {
                store::set_rcs_participants(tx, r.id, &r.recipients)?;
            }
            effects.threads.insert(r.fallback_id);
        }
        effects.everything |= !rows.deletes.is_empty();
    }
    leave(&mut pending.rcs_conversations, &item.rcs_conversations);

    if let Some(rows) = &item.rcs_chats {
        store::delete_rows(tx, Table::RcsChat, &rows.deletes)?;
        for r in &rows.upserts {
            store::upsert_rcs_chat(tx, r)?;
            effects.threads.insert(shown_thread(tx, r.common.thread_id)?);
        }
        effects.everything |= !rows.deletes.is_empty();
    }
    leave(&mut pending.rcs_chats, &item.rcs_chats);

    if let Some(rows) = &item.rcs_file_transfers {
        store::delete_rows(tx, Table::RcsFileTransfer, &rows.deletes)?;
        for r in &rows.upserts {
            store::upsert_rcs_file_transfer(tx, r)?;
            effects.threads.insert(shown_thread(tx, r.common.thread_id)?);
        }
        effects.everything |= !rows.deletes.is_empty();
    }
    leave(&mut pending.rcs_file_transfers, &item.rcs_file_transfers);

    Ok(())
}

/// The thread id the UI shows an RCS message under (its RCS conversation's fallback id).
fn shown_thread(tx: &Connection, rcs_conversation_id: i64) -> rusqlite::Result<i64> {
    Ok(store::unified_thread_of_rcs(tx, rcs_conversation_id)?.unwrap_or(rcs_conversation_id))
}

// ------------------------------------------------------------------------------------------------
// Participants
// ------------------------------------------------------------------------------------------------

/// Not empty and not a Google RCS address.
fn valid_participant(address: &str) -> bool {
    !address.is_empty() && !address.to_ascii_lowercase().ends_with(GOOGLE_RCS_SUFFIX)
}

/// The E.164 form of `address` with `region`; an address that does not parse is kept unchanged.
fn normalize(address: &str, region: Option<&str>) -> String {
    phone_number::to_e164(address, region).unwrap_or_else(|| address.to_string())
}

/// The message's subscription, else the default-for-SMS one (lowest id first).
fn subscription_for(subscriptions: &[SubscriptionRow], id: i64) -> Option<&SubscriptionRow> {
    subscriptions.iter().find(|s| s.id == id).or_else(|| subscriptions.iter().find(|s| s.is_default_sms))
}

/// The normalisation region of a message: its subscription's `countryIsos` entry when non-empty,
/// else the phone's country.
fn message_region<'a>(sub: Option<&'a SubscriptionRow>, phone_country: Option<&'a str>) -> Option<&'a str> {
    match sub {
        Some(s) if !s.country_iso.is_empty() => Some(s.country_iso.as_str()),
        _ => phone_country,
    }
}

/// Newest first; on equal timestamps the later row first.
fn newest_first(timestamps: &[i64]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..timestamps.len()).collect();
    order.sort_by(|a, b| timestamps[*b].cmp(&timestamps[*a]).then(b.cmp(a)));
    order
}

/// Participants from inserted SMS rows: inbox ⇒ the sender, otherwise the recipient, de-duplicated
/// per thread by normalised address.
fn sms_participants(
    tx: &Connection,
    rows: &[SmsRow],
    subscriptions: &[SubscriptionRow],
    phone_country: Option<&str>,
) -> rusqlite::Result<()> {
    let order = newest_first(&rows.iter().map(|r| r.timestamp_ms).collect::<Vec<_>>());
    let mut seen: HashMap<i64, HashSet<String>> = HashMap::new();
    let mut out = Vec::new();

    for i in order {
        let r = &rows[i];
        let address = if r.mailbox == mailbox::INBOX { &r.from_address } else { &r.to_address };
        if !valid_participant(address) {
            continue;
        }

        let region = message_region(subscription_for(subscriptions, r.subscription_id), phone_country);
        let normalized = normalize(address, region);
        if seen.entry(r.thread_id).or_default().insert(normalized.clone()) {
            out.push((r.thread_id, address.clone(), normalized));
        }
    }

    for (thread, address, normalized) in out {
        store::upsert_participant(tx, thread, &address, &normalized)?;
    }

    Ok(())
}

/// Participants from inserted MMS rows: for the newest inserted MMS of each thread, the sender of an
/// inbox row plus every `toAddresses` entry except the subscription's own number, ordered as the
/// conversation lists them.
fn mms_participants(
    tx: &Connection,
    rows: &[MmsRow],
    subscriptions: &[SubscriptionRow],
    phone_country: Option<&str>,
) -> rusqlite::Result<()> {
    let order = newest_first(&rows.iter().map(|r| r.timestamp_ms).collect::<Vec<_>>());
    let mut done = HashSet::new();

    for i in order {
        let r = &rows[i];
        if !done.insert(r.thread_id) {
            continue;
        }

        let recipients = store::conversation(tx, r.thread_id)?.map(|c| c.recipients).unwrap_or_default();
        let sub = subscription_for(subscriptions, r.subscription_id);
        let region = message_region(sub, phone_country);

        let mut list: Vec<(String, String)> = Vec::new();
        if r.mailbox == mailbox::INBOX && valid_participant(&r.from_address) {
            list.push((r.from_address.clone(), normalize(&r.from_address, region)));
        }

        let own = normalize(sub.map_or("", |s| s.number.as_str()), region);
        for address in r.to_addresses.as_deref().unwrap_or_default() {
            let normalized = normalize(address, region);
            if valid_participant(&normalized) && !own.eq_ignore_ascii_case(&normalized) {
                list.push((address.clone(), normalized));
            }
        }

        let position = |n: &str| recipients.iter().position(|a| a == n).map_or(-1, |p| p as i64);
        list.sort_by_key(|(_, n)| position(n));
        for (address, normalized) in list {
            store::upsert_participant(tx, r.thread_id, &address, &normalized)?;
        }
    }

    Ok(())
}

#[async_trait]
impl ContentProcessor for Exchange {
    async fn metadata(&mut self, ctx: &InboundContext<'_>, payload: &ValueSet) -> Processed {
        match self.process_metadata(ctx, payload) {
            Ok((answer, effects)) => {
                let completed = effects.completed;
                if !completed {
                    info!(sync_id = ?ctx.sync_id, requested_count = effects.requested_count, "messaging sync started");
                }
                self.after(ctx, effects);
                Processed::Answer { answer, completed }
            }
            Err(fail) => Processed::Rejected(fail.rejection()),
        }
    }

    async fn content(&mut self, ctx: &InboundContext<'_>, payload: &ValueSet) -> Processed {
        match self.process_item(ctx, payload) {
            Ok(effects) => {
                let completed = effects.completed;
                self.after(ctx, effects);
                Processed::Answer { answer: phonelink_app::simple_answer(0), completed }
            }
            Err(fail) => Processed::Rejected(fail.rejection()),
        }
    }
}

#[cfg(test)]
mod tests;
