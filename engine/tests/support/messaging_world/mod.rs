//! The messaging test world: the feature installed on the App-layer harness with in-memory edges, a
//! recording contact directory, and builders for the phone's messaging content.
//! `tests/main.rs` declares it as `crate::support::messaging_world`, next to the `crate::support::app_world` and
//! `crate::support::feature_world` it uses.
#![allow(dead_code)]

use crate::support::common::payload;
use phonelink_engine::config::features::ContactsConfig;
use phonelink_engine::config::features::MessagingConfig;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use phonelink_engine::app::InboundDisposition;
use phonelink_engine::codec::valueset::{Value, ValueSet};
use phonelink_engine::features::contacts::Contacts;
use phonelink_engine::features::messaging::edge::standins::{FakeFileTypes, FixedProximity};
use phonelink_engine::features::messaging::edge::testing::FakeImaging;
use phonelink_engine::features::messaging::{Messaging, MessagingEdges, MessagingEvent, ProximityState};
use phonelink_engine::features::{ContactDirectory, ContactMatch, StoreLocation};
use phonelink_engine::types::{DcgClient, DeviceInfoItem, TransportMessage};
use tokio::sync::broadcast;

use crate::support::app_world::{ASYNC_TRUST_CUSTOM_DATA, Harness, device_item, phone, phone_content, trace};
use crate::support::feature_world::{FeatureWorld, feature_world_at};

/// A contact directory with fixed names that records the `/contacts/v2` syncs it was asked for.
#[derive(Default)]
pub struct FakeContacts {
    pub names: Mutex<HashMap<String, String>>,
    pub syncs: Mutex<Vec<bool>>,
    pub lookups: Mutex<Vec<(String, Option<String>)>>,
}

impl FakeContacts {
    pub fn with(names: &[(&str, &str)]) -> Arc<Self> {
        let c = Self::default();
        *c.names.lock().unwrap() = names.iter().map(|(a, n)| (a.to_string(), n.to_string())).collect();
        Arc::new(c)
    }
}

impl ContactDirectory for FakeContacts {
    fn lookup(&self, _phone: &DcgClient, address: &str, region: Option<&str>) -> Option<ContactMatch> {
        self.lookups.lock().unwrap().push((address.to_string(), region.map(str::to_string)));
        // As the contacts lookup: an empty name is no name.
        self.names.lock().unwrap().get(address).map(|n| ContactMatch {
            contact_id: 1,
            display_name: (!n.is_empty()).then(|| n.clone()),
            photo_contact_id: None,
        })
    }

    fn sync_allowed(&self, _phone: &DcgClient) -> bool {
        true
    }

    fn request_sync(&self, user_initiated: bool) {
        self.syncs.lock().unwrap().push(user_initiated);
    }
}

pub struct World {
    pub h: Harness,
    pub w: FeatureWorld,
    pub m: Arc<Messaging>,
    pub imaging: Arc<FakeImaging>,
    pub proximity: Arc<FixedProximity>,
    pub contacts: Arc<FakeContacts>,
    pub events: broadcast::Receiver<MessagingEvent>,
}

/// The feature on a fresh harness with in-memory stores.
pub fn world() -> World {
    world_with(Harness::new(), StoreLocation::Memory, MessagingConfig::default())
}

/// The feature on `h` with stores at `stores`.
pub fn world_with(h: Harness, stores: StoreLocation, config: MessagingConfig) -> World {
    world_on(h, stores, config, device_item(Some(ASYNC_TRUST_CUSTOM_DATA)))
}

pub fn world_on(h: Harness, stores: StoreLocation, config: MessagingConfig, item: DeviceInfoItem) -> World {
    let w = feature_world_at(&h, stores, item);
    let imaging = FakeImaging::new();
    let proximity = FixedProximity::new(ProximityState::Connected);
    let contacts = FakeContacts::with(&[]);

    let edges =
        MessagingEdges { imaging: imaging.clone(), file_types: Arc::new(FakeFileTypes), proximity: proximity.clone() };
    let contacts_dyn: Arc<dyn ContactDirectory> = contacts.clone();
    let m = Messaging::install(&w.env, config, Some(contacts_dyn), edges).expect("messaging installs");

    let events = m.subscribe();
    World { h, w, m, imaging, proximity, contacts, events }
}

/// The feature over the contacts feature's own directory, both installed on one environment as the
/// engine installs them (contacts first).
pub struct WithContacts {
    pub h: Harness,
    pub w: FeatureWorld,
    pub m: Arc<Messaging>,
    pub contacts: Arc<Contacts>,
}

pub fn world_with_contacts() -> WithContacts {
    let h = Harness::new();
    let w = feature_world_at(&h, StoreLocation::Memory, device_item(Some(ASYNC_TRUST_CUSTOM_DATA)));
    let contacts = Contacts::install(&w.env, ContactsConfig::default()).expect("contacts installs");

    let edges = MessagingEdges {
        imaging: FakeImaging::new(),
        file_types: Arc::new(FakeFileTypes),
        proximity: FixedProximity::new(ProximityState::Connected),
    };
    let m = Messaging::install(&w.env, MessagingConfig::default(), Some(contacts.directory()), edges)
        .expect("messaging installs");

    WithContacts { h, w, m, contacts }
}

/// 2026-10-20T00:00:00Z, the feature world's clock.
pub const NOW_MS: i64 = 1_792_454_400_000;

// ------------------------------------------------------------------------------------------------
// Pushing content
// ------------------------------------------------------------------------------------------------

/// A metadata message (`metadata` present) with `extra`.
pub fn metadata_msg(extra: ValueSet) -> TransportMessage {
    phone_content("messaging", ValueSet::new().with("metadata", true).with_all(extra))
}

/// An item message with `extra`.
pub fn item_msg(extra: ValueSet) -> TransportMessage {
    phone_content("messaging", extra)
}

/// Delivers `msg` and returns the answer.
pub async fn push(h: &Harness, msg: TransportMessage) -> ValueSet {
    match h.app.handle_inbound(phone(), trace(), msg).await {
        InboundDisposition::Answered(v) => v,
        other => panic!("not answered: {other:?}"),
    }
}

pub fn result_of(v: &ValueSet) -> Option<i32> {
    v.i32("result")
}

pub trait WithAll {
    fn with_all(self, other: ValueSet) -> Self;
}

impl WithAll for ValueSet {
    fn with_all(mut self, other: ValueSet) -> Self {
        for (k, v) in other.iter() {
            self.insert(k, v.clone());
        }
        self
    }
}

/// The `ValueSets` of the `/chat/v1` requests sent so far.
pub fn chat_requests(h: &Harness) -> Vec<ValueSet> {
    h.messenger.requests_on("/chat/v1").iter().map(payload).collect()
}

// ------------------------------------------------------------------------------------------------
// Content builders
// ------------------------------------------------------------------------------------------------

/// One SMS row.
#[derive(Debug, Clone)]
pub struct Sms {
    pub id: i64,
    pub thread: i64,
    pub date: i64,
    pub from: String,
    pub to: String,
    pub body: String,
    pub mailbox: i32,
    pub read: bool,
    pub checksum: i64,
    pub sub: i64,
}

impl Sms {
    /// An unread inbox SMS from `from`, sent now, on subscription 1.
    pub fn inbox(id: i64, thread: i64, from: &str, body: &str) -> Self {
        Self {
            id,
            thread,
            date: NOW_MS,
            from: from.into(),
            to: String::new(),
            body: body.into(),
            mailbox: 1,
            read: false,
            checksum: id * 10,
            sub: 1,
        }
    }

    pub fn sent(id: i64, thread: i64, to: &str, body: &str) -> Self {
        Self { mailbox: 2, read: true, from: String::new(), to: to.into(), ..Self::inbox(id, thread, "", body) }
    }
}

pub fn sms_object(rows: &[Sms]) -> ValueSet {
    ValueSet::new()
        .with("itemCount", rows.len() as i32)
        .with("messageIds", rows.iter().map(|r| r.id).collect::<Vec<i64>>())
        .with("threadIds", rows.iter().map(|r| r.thread).collect::<Vec<i64>>())
        .with("dates", rows.iter().map(|r| r.date).collect::<Vec<i64>>())
        .with("fromAddresses", rows.iter().map(|r| r.from.clone()).collect::<Vec<String>>())
        .with("toAddresses", rows.iter().map(|r| r.to.clone()).collect::<Vec<String>>())
        .with("bodies", rows.iter().map(|r| r.body.clone()).collect::<Vec<String>>())
        .with("subjects", rows.iter().map(|_| String::new()).collect::<Vec<String>>())
        .with("messageTypes", rows.iter().map(|r| r.mailbox).collect::<Vec<i32>>())
        .with("readFlags", rows.iter().map(|r| r.read).collect::<Vec<bool>>())
        .with("checksums", rows.iter().map(|r| r.checksum).collect::<Vec<i64>>())
        .with("subscriptionIds", rows.iter().map(|r| r.sub).collect::<Vec<i64>>())
}

/// `conversations` and `canonicalAddresses` for `(thread id, addresses)` rows.
pub fn conversations(rows: &[(i64, &[&str])]) -> ValueSet {
    let mut ids: Vec<String> = Vec::new();
    let mut addresses: Vec<String> = Vec::new();
    let mut recipient_ids = Vec::new();
    for (_, list) in rows {
        let mut own = Vec::new();
        for a in *list {
            let pos = match addresses.iter().position(|x| x == a) {
                Some(p) => p,
                None => {
                    addresses.push(a.to_string());
                    ids.push((addresses.len() + 100).to_string());
                    addresses.len() - 1
                }
            };
            own.push(ids[pos].clone());
        }
        recipient_ids.push(own.join(" "));
    }

    ValueSet::new()
        .with(
            "conversations",
            ValueSet::new()
                .with("itemCount", rows.len() as i32)
                .with("ids", rows.iter().map(|(t, _)| *t).collect::<Vec<i64>>())
                .with("recipientIds", recipient_ids)
                .with("checksums", rows.iter().map(|(t, _)| t * 100).collect::<Vec<i64>>()),
        )
        .with("canonicalAddresses", ValueSet::new().with("ids", ids).with("addresses", addresses))
}

/// One SIM subscription.
#[derive(Debug, Clone)]
pub struct Sub {
    pub id: i64,
    pub country: String,
    pub number: String,
    pub mms: bool,
    pub group_mms: bool,
    pub default_sms: bool,
    pub max_size: i32,
    pub max_w: i32,
    pub max_h: i32,
    pub max_text: i32,
    pub recipient_limit: i32,
    pub rcs: bool,
}

impl Sub {
    pub fn new(id: i64, country: &str) -> Self {
        Self {
            id,
            country: country.into(),
            number: String::new(),
            mms: true,
            group_mms: true,
            default_sms: true,
            max_size: 0,
            max_w: 0,
            max_h: 0,
            max_text: 0,
            recipient_limit: 0,
            rcs: false,
        }
    }
}

pub fn subscriptions_object(rows: &[Sub]) -> ValueSet {
    let n = rows.len();
    let bools = |f: fn(&Sub) -> bool| rows.iter().map(f).collect::<Vec<bool>>();
    let ints = |f: fn(&Sub) -> i32| rows.iter().map(f).collect::<Vec<i32>>();

    ValueSet::new()
        .with("itemCount", n as i32)
        .with("ids", rows.iter().map(|s| s.id).collect::<Vec<i64>>())
        .with("simSlotIndices", (0..n as i64).collect::<Vec<i64>>())
        .with("countryIsos", rows.iter().map(|s| s.country.clone()).collect::<Vec<String>>())
        .with("names", rows.iter().map(|_| "SIM".to_string()).collect::<Vec<String>>())
        .with("numbers", rows.iter().map(|s| s.number.clone()).collect::<Vec<String>>())
        .with("isRoamings", vec![false; n])
        .with("isMmsEnableds", bools(|s| s.mms))
        .with("allowAudioAttachments", vec![true; n])
        .with("isMultipartSmsEnableds", vec![true; n])
        .with("isGroupMmsEnableds", bools(|s| s.group_mms))
        .with("shouldSendMultipartSmsAsSeparateMessages", vec![false; n])
        .with("maxMessageSizes", ints(|s| s.max_size))
        .with("recipientLimits", ints(|s| s.recipient_limit))
        .with("maxImageHeights", ints(|s| s.max_h))
        .with("maxImageWidths", ints(|s| s.max_w))
        .with("smsMultipartToMmsTextThresholds", vec![0i32; n])
        .with("smsToMmsTextLengthThresholds", vec![0i32; n])
        .with("maxMessageTextLengths", ints(|s| s.max_text))
        .with("maxSubjectLengths", vec![40i32; n])
        .with("isDefaultDataSubscriptions", vec![true; n])
        .with("isDefaultSmsSubscriptions", bools(|s| s.default_sms))
        .with("isDefaultSubscriptions", vec![true; n])
        .with("isDefaultVoiceSubscriptions", vec![true; n])
        .with("isRcsSupported", bools(|s| s.rcs))
}

/// One MMS row.
#[derive(Debug, Clone)]
pub struct Mms {
    pub id: i64,
    pub thread: i64,
    pub date: i64,
    pub from: String,
    pub to: String,
    pub mailbox: i32,
    pub read: bool,
    pub sub: i64,
}

pub fn mms_object(rows: &[Mms], with_to: bool) -> ValueSet {
    let mut v = ValueSet::new()
        .with("itemCount", rows.len() as i32)
        .with("messageIds", rows.iter().map(|r| r.id).collect::<Vec<i64>>())
        .with("threadIds", rows.iter().map(|r| r.thread).collect::<Vec<i64>>())
        .with("dates", rows.iter().map(|r| r.date).collect::<Vec<i64>>())
        .with("subjects", rows.iter().map(|_| String::new()).collect::<Vec<String>>())
        .with("subjectCharsets", vec![106i32; rows.len()])
        .with("messageBoxTypes", rows.iter().map(|r| r.mailbox).collect::<Vec<i32>>())
        .with("readFlags", rows.iter().map(|r| r.read).collect::<Vec<bool>>())
        .with("subscriptionIds", rows.iter().map(|r| r.sub).collect::<Vec<i64>>())
        .with("fromAddresses", rows.iter().map(|r| r.from.clone()).collect::<Vec<String>>());

    if with_to {
        v.insert("toAddresses", rows.iter().map(|r| r.to.clone()).collect::<Vec<String>>());
    }
    v
}

/// One MMS part; `bytes: None` sends `contentByteSizes` `-1`.
#[derive(Debug, Clone)]
pub struct Part {
    pub id: i64,
    pub message: i64,
    pub content_type: String,
    pub text: String,
    pub bytes: Option<Vec<u8>>,
}

pub fn parts_object(parts: &[Part]) -> ValueSet {
    let mut blob = Vec::new();
    let mut sizes = Vec::new();
    for p in parts {
        match &p.bytes {
            Some(b) => {
                sizes.push(b.len() as i32);
                blob.extend(b);
            }
            None => sizes.push(-1),
        }
    }

    let n = parts.len();
    ValueSet::new()
        .with("itemCount", n as i32)
        .with("messageIds", parts.iter().map(|p| p.message).collect::<Vec<i64>>())
        .with("partIds", parts.iter().map(|p| p.id).collect::<Vec<i64>>())
        .with("sequenceNumbers", (0..n as i32).collect::<Vec<i32>>())
        .with("contentIds", vec![String::new(); n])
        .with("contentTypes", parts.iter().map(|p| p.content_type.clone()).collect::<Vec<String>>())
        .with("texts", parts.iter().map(|p| p.text.clone()).collect::<Vec<String>>())
        .with("names", vec![String::new(); n])
        .with("charsets", vec![106i32; n])
        .with("sizes", parts.iter().map(|p| p.bytes.as_ref().map_or(1000, |b| b.len() as i64)).collect::<Vec<i64>>())
        .with("contentByteSizes", sizes)
        .with("contentBytes", Value::UInt8Array(blob))
}

/// RCS conversations (+ `canonicalAddresses`) for `(id, fallback, type, addresses)` rows.
pub fn rcs_conversations(rows: &[(i64, i64, i32, &[&str])]) -> ValueSet {
    let mut ids: Vec<String> = Vec::new();
    let mut addresses: Vec<String> = Vec::new();
    let mut recipient_ids = Vec::new();
    for (_, _, _, list) in rows {
        let mut own = Vec::new();
        for a in *list {
            addresses.push(a.to_string());
            ids.push((addresses.len() + 500).to_string());
            own.push(ids.last().unwrap().clone());
        }
        recipient_ids.push(own.join(" "));
    }

    ValueSet::new()
        .with(
            "rcsConversations",
            ValueSet::new()
                .with("itemCount", rows.len() as i32)
                .with("ids", rows.iter().map(|r| r.0).collect::<Vec<i64>>())
                .with("fallbackIds", rows.iter().map(|r| r.1).collect::<Vec<i64>>())
                .with("rcsConversationTypes", rows.iter().map(|r| r.2).collect::<Vec<i32>>())
                .with("recipientIds", recipient_ids)
                .with("checksums", rows.iter().map(|r| r.0 * 7).collect::<Vec<i64>>()),
        )
        .with("canonicalAddresses", ValueSet::new().with("ids", ids).with("addresses", addresses))
}

/// One RCS chat row (inbox, unread, now).
pub fn rcs_chats_object(rows: &[(i64, i64, &str, &str)]) -> ValueSet {
    let n = rows.len();
    ValueSet::new()
        .with("itemCount", n as i32)
        .with("messageIds", rows.iter().map(|r| r.0).collect::<Vec<i64>>())
        .with("threadIds", rows.iter().map(|r| r.1).collect::<Vec<i64>>())
        .with("dates", vec![NOW_MS; n])
        .with("fromAddresses", rows.iter().map(|r| r.2.to_string()).collect::<Vec<String>>())
        .with("bodies", rows.iter().map(|r| r.3.to_string()).collect::<Vec<String>>())
        .with("contentTypes", vec!["text/plain".to_string(); n])
        .with("sessionIds", vec!["s".to_string(); n])
        .with("messageTypes", vec![1i32; n])
        .with("readFlags", vec![false; n])
        .with("rcsMessageTypes", vec![0i32; n])
        .with("subscriptionIds", vec![1i64; n])
        .with("actions", vec![0i32; n])
}

/// RCS file transfers `(id, thread, content type, bytes)` (inbox, unread, now).
pub fn rcs_files_object(rows: &[(i64, i64, &str, Option<Vec<u8>>)], actions: &[i32]) -> ValueSet {
    let n = rows.len();

    let mut blob = Vec::new();
    let mut counts = Vec::new();
    for r in rows {
        match &r.3 {
            Some(b) => {
                counts.push(b.len() as i32);
                blob.extend(b);
            }
            None => counts.push(-1),
        }
    }

    ValueSet::new()
        .with("itemCount", n as i32)
        .with("messageIds", rows.iter().map(|r| r.0).collect::<Vec<i64>>())
        .with("threadIds", rows.iter().map(|r| r.1).collect::<Vec<i64>>())
        .with("dates", vec![NOW_MS; n])
        .with("fromAddresses", vec!["+15550100".to_string(); n])
        .with("contentTypes", rows.iter().map(|r| r.2.to_string()).collect::<Vec<String>>())
        .with("sessionIds", vec!["s".to_string(); n])
        .with("messageTypes", vec![1i32; n])
        .with("readFlags", vec![false; n])
        .with("rcsMessageTypes", vec![0i32; n])
        .with("subscriptionIds", vec![1i64; n])
        .with("actions", actions.to_vec())
        .with("fileNames", rows.iter().map(|r| format!("f{}", r.0)).collect::<Vec<String>>())
        .with("fileSizes", vec![10i32; n])
        .with("fileByteCounts", counts)
        .with("fileBytes", Value::UInt8Array(blob))
}

/// Drains the events received so far.
pub fn drain(rx: &mut broadcast::Receiver<MessagingEvent>) -> Vec<MessagingEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}
