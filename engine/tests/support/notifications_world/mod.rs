//! The notifications feature installed over the App-layer test harness (`support/app_world`)
//! and the `FeatureEnv` world (`support/feature_world`), plus builders for batches and item JSON.
//! `tests/main.rs` declares it as `crate::support::notifications_world`, next to the `crate::support::app_world` and
//! `crate::support::feature_world` it uses.
#![allow(dead_code)]

use crate::support::common::payload;
use phonelink_engine::config::features::NotificationsConfig;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tokio::sync::oneshot;

use phonelink_engine::app::consts::permission;
use phonelink_engine::app::{AppEvent, InboundDisposition};
use phonelink_engine::codec::valueset::{self, Value, ValueSet};
use phonelink_engine::features::notifications::{Notifications, NotificationsEvent, install};
use phonelink_engine::features::{MessagingAlerts, StoreLocation};
use phonelink_engine::seams::{PlatformRequest, PlatformResponse};
use phonelink_engine::types::{DcgClient, TransportMessage};

use crate::support::app_world::*;
use crate::support::feature_world::{FeatureWorld, feature_world_at};

/// The `ManualClock::fixed()` time (2026-10-20T00:00:00Z) in Unix ms: items posted now alert.
pub const NOW_MS: i64 = 1_792_454_400_000;

pub struct World {
    pub h: Harness,
    pub w: FeatureWorld,
    pub n: Arc<Notifications>,
}

/// Group-message consent of the messaging feature, settable.
#[derive(Default)]
pub struct Consent(pub Mutex<bool>);

impl MessagingAlerts for Consent {
    fn group_message_alerts_allowed(&self, _phone: &DcgClient) -> bool {
        *self.0.lock().unwrap()
    }
}

/// Options of a test world.
pub struct Setup {
    pub config: NotificationsConfig,
    pub messaging: Option<Arc<dyn MessagingAlerts>>,
    /// The device permission (record bit `0x40`) before install.
    pub device_permission: bool,
    pub stores: StoreLocation,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            config: NotificationsConfig::default(),
            messaging: None,
            device_permission: true,
            stores: StoreLocation::Memory,
        }
    }
}

/// A world whose phone already granted notification access (no full sync (b) at install).
pub fn world() -> World {
    world_with(Setup::default())
}

pub fn world_with(setup: Setup) -> World {
    let h = Harness::new();
    if setup.device_permission {
        h.app.update_phone_record(|r| r.set_permission(permission::NOTIFICATIONS_SYNC, true));
    }
    let w = feature_world_at(&h, setup.stores, device_item(Some(ASYNC_TRUST_CUSTOM_DATA)));
    let n = install(&w.env, setup.config, setup.messaging).expect("notifications install");
    World { h, w, n }
}

/// An item JSON text: `key`, `packageName` `com.example.chat`, `postTime`, plus `extra`'s keys.
pub fn item(key: &str, post_time: i64, extra: serde_json::Value) -> String {
    let mut v = json!({"key": key, "packageName": "com.example.chat", "postTime": post_time});
    for (k, x) in extra.as_object().expect("extra is an object") {
        v[k] = x.clone();
    }
    v.to_string()
}

/// An item that alerts: importance 4, posted now, with a title, a text and an app name.
pub fn alerting(key: &str) -> String {
    item(key, NOW_MS, json!({"importance": 4, "appName": "Chat", "title": "Ann", "text": "hi"}))
}

/// The content `ValueSet` of a batch.
pub fn batch_values(keys: &[&str], ops: &[i32], items: &[String]) -> ValueSet {
    ValueSet::new()
        .with("notificationKeys", keys.to_vec())
        .with("operations", ops.to_vec())
        .with("notifications", Value::StringArray(items.to_vec()))
}

/// A `/legacy/phonecontent` content message (`correlationVector` `cv-1`).
pub fn batch(keys: &[&str], ops: &[i32], items: &[String]) -> TransportMessage {
    phone_content("notifications", batch_values(keys, ops, items))
}

/// A `/legacy/phonecontent` message with exactly `values` (no `correlationVector` unless given).
pub fn raw_content(values: ValueSet) -> TransportMessage {
    message(&[("_requestId", "req-9"), ("_route", "/legacy/phonecontent")], valueset::encode(&values))
}

/// Delivers `msg` and returns the answer.
pub async fn deliver(h: &Harness, msg: TransportMessage) -> ValueSet {
    match h.app.handle_inbound(phone(), trace(), msg).await {
        InboundDisposition::Answered(vs) => vs,
        other => panic!("not answered: {other:?}"),
    }
}

/// The trace state as `(key, value)` pairs.
pub fn trace_state(req: &PlatformRequest) -> Vec<(String, String)> {
    req.trace.trace_state.clone()
}

pub fn trace_names(req: &PlatformRequest) -> (String, String) {
    let get = |k: &str| req.trace.trace_state.iter().find(|(x, _)| x == k).map(|(_, v)| v.clone()).unwrap_or_default();
    (get("ypTriggerId"), get("ypScenarioId"))
}

/// The `/notifications` requests sent so far.
pub fn notification_requests(h: &Harness) -> Vec<PlatformRequest> {
    h.messenger.requests_on("/notifications")
}

pub fn permission_requests(h: &Harness) -> Vec<PlatformRequest> {
    h.messenger.requests_on("/permission")
}

pub fn settings_requests(h: &Harness) -> Vec<PlatformRequest> {
    h.messenger.requests_on("/settings")
}

/// A connect whose response stores `NotificationsCapabilities = caps`; later requests are answered `0`.
pub async fn store_capabilities(w: &World, caps: i32) {
    w.h.messenger.answer_all(response_with(
        ValueSet::new().with("result", 0i32).with("capabilities", ValueSet::new().with("notifications", caps)),
    ));
    w.h.app.send_connect(phonelink_engine::app::ConnectKind::Background, "Test").await;
    settle().await;
    w.h.messenger.answer_all(result_response(0));
    assert_eq!(w.n.status().unwrap().capabilities, caps);
}

/// A `permissions` push reporting `NotificationsSync` (`64`) granted (`state 0`) or denied (`1`).
pub fn device_permission_push(granted: bool) -> TransportMessage {
    phone_content(
        "permissions",
        ValueSet::new()
            .with("itemCount", 1i32)
            .with("permissionTypes", vec![64i32])
            .with("permissionStates", vec![if granted { 0i32 } else { 1 }]),
    )
}

/// `phoneCommand` type 7: the sensitive-notification permission granted or denied.
pub fn sensitive_command(granted: bool) -> TransportMessage {
    let scenario = if granted {
        "notifications_sensitive_permission_granted"
    } else {
        "notifications_sensitive_permission_denied"
    };
    phone_content(
        "phoneCommand",
        ValueSet::new().with("phoneCommandType", 7i32).with("notificationsSensitivePermissionScenario", scenario),
    )
}

/// The full syncs (operation 3) sent so far.
pub fn full_syncs(h: &Harness) -> Vec<PlatformRequest> {
    notification_requests(h).into_iter().filter(|r| payload(r).i32("operation") == Some(3)).collect()
}

/// Requests [`hold_requests`] keeps unanswered until [`release`].
pub type Held = Arc<Mutex<Vec<oneshot::Sender<PlatformResponse>>>>;

/// Holds every request `select` picks until it is released; answers the others `{ result: 0 }` at once.
pub fn hold_requests(h: &Harness, select: impl Fn(&PlatformRequest) -> bool + Send + 'static) -> Held {
    let held: Held = Arc::new(Mutex::new(Vec::new()));
    let slot = held.clone();

    h.messenger.set_responder(move |req| {
        if select(req) {
            let (tx, rx) = oneshot::channel();
            slot.lock().unwrap().push(tx);
            Reply::Later(rx)
        } else {
            Reply::Now(result_response(0))
        }
    });

    held
}

/// Answers the oldest held request `{ result: code }`.
pub fn release(held: &Held, code: i32) {
    let tx = held.lock().unwrap().remove(0);
    tx.send(result_response(code)).expect("the held request still waits");
}

/// Lets spawned tasks run (time is paused in these tests).
pub async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_millis(1)).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

/// Every event received so far.
pub fn drain(rx: &mut tokio::sync::broadcast::Receiver<NotificationsEvent>) -> Vec<NotificationsEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

pub fn app_events(rx: &mut tokio::sync::broadcast::Receiver<AppEvent>) -> Vec<AppEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

/// `{ result: Int32 code }` plus `extra`, as a phone answer.
pub fn answer(code: i32, extra: ValueSet) -> PlatformResponse {
    let mut vs = ValueSet::new().with("result", code);
    for (k, v) in extra.iter() {
        vs.insert(k, v.clone());
    }
    response_with(vs)
}
