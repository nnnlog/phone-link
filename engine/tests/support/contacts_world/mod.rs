//! Helpers of the contacts tests: an App-layer harness with contacts installed over the feature
//! world, and builders for `contactsV2` messages. `tests/main.rs` declares it as
//! `crate::support::contacts_world`, next to the `crate::support::app_world` and `crate::support::feature_world` it uses.
#![allow(dead_code)]

use phonelink_engine::config::features::ContactsConfig;
use std::sync::Arc;

use phonelink_engine::app::InboundDisposition;
use phonelink_engine::codec::valueset::{self, Value, ValueSet};
use phonelink_engine::features::contacts::{Contacts, ContactsEvent, SyncState};
use phonelink_engine::features::{FeatureEnv, StoreLocation};
use phonelink_engine::types::{DeviceInfoItem, TransportMessage};

use crate::support::app_world::{ASYNC_TRUST_CUSTOM_DATA, Harness, device_item, message, phone, phone_content, trace};
use crate::support::feature_world::{FeatureWorld, feature_world, feature_world_at};

pub struct World {
    pub h: Harness,
    pub w: FeatureWorld,
    pub contacts: Arc<Contacts>,
}

/// A connected harness (Android phone with the default capability bits, so contacts-capable) with
/// contacts installed over in-memory stores.
pub fn world() -> World {
    world_with(ContactsConfig::default())
}

pub fn world_with(config: ContactsConfig) -> World {
    let h = Harness::new();
    let w = feature_world(&h);
    let contacts = Contacts::install(&w.env, config).expect("contacts install");
    World { h, w, contacts }
}

/// As [`world`] for a phone whose device-list `customData` is `custom_data`.
pub fn world_with_custom_data(custom_data: &str) -> World {
    let h = Harness::with_custom_data(Some(custom_data));
    let w = feature_world_at(&h, StoreLocation::Memory, device_item(Some(custom_data)));
    let contacts = Contacts::install(&w.env, ContactsConfig::default()).expect("contacts install");
    World { h, w, contacts }
}

/// As [`world`] over on-disk stores at `dir`.
pub fn world_at(dir: &std::path::Path) -> World {
    let h = Harness::new();
    let w =
        feature_world_at(&h, StoreLocation::Directory(dir.to_path_buf()), device_item(Some(ASYNC_TRUST_CUSTOM_DATA)));
    let contacts = Contacts::install(&w.env, ContactsConfig::default()).expect("contacts install");
    World { h, w, contacts }
}

/// Contacts installed on `env` (for a world whose environment a test edited).
pub fn install_on(env: &FeatureEnv) -> Arc<Contacts> {
    Contacts::install(env, ContactsConfig::default()).expect("contacts install")
}

pub fn device(custom_data: &str) -> DeviceInfoItem {
    device_item(Some(custom_data))
}

/// Concatenates `extra` into `base` (later keys win).
pub fn with(mut base: ValueSet, extra: ValueSet) -> ValueSet {
    for (k, v) in extra.iter() {
        base.insert(k, v.clone());
    }
    base
}

/// A `contactsV2` metadata message (`correlationVector: cv-1` unless `extra` sets one).
pub fn metadata(extra: ValueSet) -> TransportMessage {
    phone_content("contactsV2", with(ValueSet::new().with("metadata", true), extra))
}

/// A `contactsV2` item message (`correlationVector: cv-1` unless `extra` sets one).
pub fn item(extra: ValueSet) -> TransportMessage {
    phone_content("contactsV2", extra)
}

/// A `/legacy/phonecontent` request whose payload is exactly `vs` (no correlationVector added).
pub fn raw(vs: ValueSet) -> TransportMessage {
    message(&[("_requestId", "req-1"), ("_route", "/legacy/phonecontent")], valueset::encode(&vs))
}

/// The six sequence numbers, all `Int64`.
pub fn seq(values: [i64; 6]) -> ValueSet {
    let names = ["contact", "phonenumber", "emailaddress", "postaladdress", "contactdate", "contacturl"];
    let mut vs = ValueSet::new();
    for (k, v) in names.iter().zip(values) {
        vs.insert(*k, v);
    }
    vs
}

/// A `contacts` batch: contact `id` named `Contact <id>`, checksum `id * 10`, no thumbnail.
pub fn contacts_batch(ids: &[i64]) -> ValueSet {
    ValueSet::new()
        .with("itemCount", ids.len() as i32)
        .with("contactIds", ids.to_vec())
        .with("lastUpdatedTimes", ids.iter().map(|_| 1_700_000_000_000i64).collect::<Vec<_>>())
        .with("checksums", ids.iter().map(|id| id * 10).collect::<Vec<_>>())
        .with("displayNames", ids.iter().map(|id| format!("Contact {id}")).collect::<Vec<_>>())
}

/// A `phoneNumbers` batch of `(id, contact id, number, display number)`, checksum `id * 10`.
pub fn phone_batch(entries: &[(i64, i64, &str, &str)]) -> ValueSet {
    ValueSet::new()
        .with("itemCount", entries.len() as i32)
        .with("phoneNumberIds", entries.iter().map(|e| e.0).collect::<Vec<_>>())
        .with("contactIds", entries.iter().map(|e| e.1).collect::<Vec<_>>())
        .with("checksums", entries.iter().map(|e| e.0 * 10).collect::<Vec<_>>())
        .with("phoneNumbers", entries.iter().map(|e| e.2.to_string()).collect::<Vec<_>>())
        .with("displayNumbers", entries.iter().map(|e| e.3.to_string()).collect::<Vec<_>>())
        .with("types", entries.iter().map(|_| 2i32).collect::<Vec<_>>())
}

/// An `emailAddresses` batch of `(id, contact id, address)`, checksum `id * 10`.
pub fn email_batch(entries: &[(i64, i64, &str)]) -> ValueSet {
    ValueSet::new()
        .with("itemCount", entries.len() as i32)
        .with("contactEmailAddressIds", entries.iter().map(|e| e.0).collect::<Vec<_>>())
        .with("contactIds", entries.iter().map(|e| e.1).collect::<Vec<_>>())
        .with("checksums", entries.iter().map(|e| e.0 * 10).collect::<Vec<_>>())
        .with("emailAddresses", entries.iter().map(|e| e.2.to_string()).collect::<Vec<_>>())
}

/// Sends `msg` from the harness phone and returns the answer.
pub async fn send(h: &Harness, msg: TransportMessage) -> ValueSet {
    match h.app.handle_inbound(phone(), trace(), msg).await {
        InboundDisposition::Answered(v) => v,
        other => panic!("not answered: {other:?}"),
    }
}

pub fn result(code: i32) -> ValueSet {
    ValueSet::new().with("result", code)
}

/// The stored records of the harness phone: `(contact id, display name)` by id.
pub fn contact_names(world: &World) -> Vec<(i64, String)> {
    let mut v: Vec<_> =
        world.contacts.contacts(&phone()).unwrap().into_iter().map(|c| (c.id, c.display_name)).collect();
    v.sort();
    v
}

pub fn sync_state(world: &World) -> SyncState {
    world.contacts.sync_state(&phone()).unwrap()
}

pub fn int64s(v: &[i64]) -> Value {
    Value::Int64Array(v.to_vec())
}

/// Drains `rx`.
pub fn drain(rx: &mut tokio::sync::broadcast::Receiver<ContactsEvent>) -> Vec<ContactsEvent> {
    let mut v = Vec::new();
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}
