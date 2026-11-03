//! The call-history test world: the feature installed on the App-layer harness with in-memory stores
//! and a recording contact directory, the phone made call-history capable, and builders for the
//! phone's `callLogs` content. `tests/main.rs` declares it as `crate::support::calling_world`, next to
//! the `crate::support::app_world`, `crate::support::feature_world` and
//! `crate::support::messaging_world` it uses.
#![allow(dead_code)]

use std::sync::Arc;

use phonelink_engine::app::InboundDisposition;
use phonelink_engine::app::consts::{capability, permission, platform};
use phonelink_engine::codec::valueset::{Value, ValueSet};
use phonelink_engine::config::features::{BluetoothConfig, CallingConfig};
use phonelink_engine::features::bluetooth::Bluetooth;
use phonelink_engine::features::calling::Calling;
use phonelink_engine::features::calling::control::testing::FakeAg;
use phonelink_engine::types::TransportMessage;

use crate::support::app_world::{Harness, phone, phone_content, trace};
use crate::support::feature_world::{FeatureWorld, feature_world};
use crate::support::messaging_world::FakeContacts;

pub struct World {
    pub h: Harness,
    pub w: FeatureWorld,
    pub calling: Arc<Calling>,
    pub contacts: Arc<FakeContacts>,
    /// The phone's Bluetooth radio and Audio Gateway call control reaches.
    pub ag: Arc<FakeAg>,
}

/// A connected harness whose Android phone has both calling capabilities and contacts
/// capabilities, with call history installed; the consent and the call-log permission are as a new
/// phone has them (off).
pub fn world() -> World {
    world_with(CallingConfig::default())
}

pub fn world_with(config: CallingConfig) -> World {
    let h = Harness::new();
    let w = feature_world(&h);
    let contacts = FakeContacts::with(&[("+15550100", "Ada")]);
    let ag = FakeAg::new();
    let bluetooth = Bluetooth::install(&w.env, BluetoothConfig::default(), ag.clone()).expect("bluetooth install");
    let calling = Calling::install(&w.env, config, Some(contacts.clone()), bluetooth).expect("calling install");
    h.app.update_record_for(&phone(), |r| {
        r.platform = platform::ANDROID;
        r.capability_bits |= capability::CALLS | capability::CALL_LOGS;
        r.contacts_capabilities = Some(1);
    });
    World { h, w, calling, contacts, ag }
}

impl World {
    /// The consent on and the phone allowing its call log: the history is shown.
    pub fn grant(&self) {
        self.h.app.update_record_for(&phone(), |r| r.set_permission(permission::CALL_LOGS, true));
        self.calling.set_pc_consent(&phone(), true).unwrap();
    }
}

/// One call: `(id, number, type, start ms, last-updated ms)`.
pub type Call<'a> = (i64, &'a str, i32, i64, i64);

/// A `callLogs` metadata message listing `(id, last-updated ms)`.
pub fn metadata(listed: &[(i64, i64)], extra: ValueSet) -> TransportMessage {
    let mut vs = ValueSet::new()
        .with("metadata", true)
        .with("callLogsIds", Value::Int64Array(listed.iter().map(|c| c.0).collect()))
        .with("callLogsLastUpdatedTimes", Value::Int64Array(listed.iter().map(|c| c.1).collect()));
    for (k, v) in extra.iter() {
        vs.insert(k, v.clone());
    }
    phone_content("callLogs", vs)
}

/// The arrays of an item message carrying `calls` (duration 30 s, unread, no SIM id).
pub fn items_payload(calls: &[Call<'_>]) -> ValueSet {
    ValueSet::new()
        .with("itemCount", calls.len() as i32)
        .with("callLogsIds", Value::Int64Array(calls.iter().map(|c| c.0).collect()))
        .with("callLogsPhoneNumbers", Value::StringArray(calls.iter().map(|c| c.1.to_string()).collect()))
        .with("callLogsDuration", Value::Int64Array(calls.iter().map(|_| 30).collect()))
        .with("callLogsType", Value::Int32Array(calls.iter().map(|c| c.2).collect()))
        .with("callLogsDate", Value::Int64Array(calls.iter().map(|c| c.3).collect()))
        .with("callLogsRead", Value::BoolArray(calls.iter().map(|_| false).collect()))
        .with("callLogsPhoneAccountIds", Value::StringArray(calls.iter().map(|_| String::new()).collect()))
        .with("callLogsLastUpdatedTimes", Value::Int64Array(calls.iter().map(|c| c.4).collect()))
}

/// A `callLogs` item message carrying `calls` and `extra`.
pub fn items(calls: &[Call<'_>], extra: ValueSet) -> TransportMessage {
    let mut vs = items_payload(calls);
    for (k, v) in extra.iter() {
        vs.insert(k, v.clone());
    }
    phone_content("callLogs", vs)
}

/// Sends `msg` from the harness phone and returns the answer.
pub async fn send(h: &Harness, msg: TransportMessage) -> ValueSet {
    match h.app.handle_inbound(phone(), trace(), msg).await {
        InboundDisposition::Answered(v) => v,
        other => panic!("not answered: {other:?}"),
    }
}
