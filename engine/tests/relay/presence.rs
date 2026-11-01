//! Presence and the inbound hub methods against a fake hub: the hand-off to the DCG layer, the
//! partner set and the `SendConnectedAsync` exchange, the `OnConnected` payload, the heartbeat
//! window, connection sharing, keys and region (inbound auto-assignment), and unknown inbound
//! traffic. Real time with short heartbeat windows.

use crate::support::common::{eventually, poll_once};
use crate::support::fake_hub;
use crate::support::relay_world as fixtures;
use crate::support::session_log;

use std::sync::Arc;
use std::time::Duration;

use fake_hub::{
    CompletionMode, on_connected, on_partner_connected, on_partner_disconnected, on_partner_heartbeat,
    on_receive_message, on_receive_session_based_message, trace_value,
};
use fixtures::{Harness, PHONE, RecordingPacketSink, drain_events, next_event, phone};
use phonelink_engine::codec::dcg_packet::{Ack, Fragment};
use phonelink_engine::codec::hub::{self, HubMessage, TraceContextPacket, method};
use phonelink_engine::seams::{HubControl, HubLink, HubRegionTimeout, RelayEvent, WakeHub};
use phonelink_engine::tap::TapLayer;
use phonelink_engine::types::{DcgClient, Environment, HubSendResult, HubState, PartnerDisconnectReason, TraceContext};
use rmpv::Value;
use tokio::time::Instant;

const SECOND: Duration = Duration::from_secs(1);

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn prod(id: &str) -> DcgClient {
    DcgClient::new(id, Environment::Prod)
}

fn connected(id: &str) -> impl Fn(&RelayEvent) -> bool + '_ {
    move |e| matches!(e, RelayEvent::PartnerConnected(p) if p.id == id)
}

fn disconnected(id: &str) -> impl Fn(&RelayEvent) -> bool + '_ {
    move |e| matches!(e, RelayEvent::PartnerDisconnected { partner, .. } if partner.id == id)
}

fn frame(value: Value) -> Vec<u8> {
    let mut out = Vec::new();
    hub::write_frame(&hub::to_bytes(&value), &mut out);
    out
}

/// Heartbeat windows of 150 ms (frequency 100 ms + grace 50 ms): removal after ≈ 450 ms.
fn short_heartbeats(c: &mut phonelink_engine::config::EngineConfig) {
    c.relay.heartbeat_frequency = ms(100);
    c.relay.heartbeat_grace = ms(50);
}

fn fragment_packet() -> hub::MultiplexPacket {
    Fragment {
        version: 1.0,
        session_id: "6f9619ff-8b86-d011-b42d-00c04fc964ff".into(),
        sequence_number: 3,
        message_id: 9,
        message_type_raw: Some(1),
        fragment_id: 1,
        fragment_count: 1,
        raw: vec![1, 2, 3],
    }
    .to_packet()
}

// ------------------------------------------------------------------------------------------------
// OnPartnerConnected and SendConnectedAsync
// ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn on_partner_connected_is_answered_with_the_golden_send_connected() {
    let mut h = Harness::new(|_| {}).await;
    let session = h.open_session().await;

    session.send(on_partner_connected(PHONE, Value::from("eastus")));
    let reply = session.wait_invocation(2 * SECOND, |i| i.target == method::SEND_CONNECTED).await;
    next_event(&mut h.events, SECOND, connected(PHONE)).await;
    assert!(h.relay.is_partner_connected(&phone()));

    let trace = reply.trace().expect("a trace context packet");
    let trace_id = trace.trace_id.expect("TraceId");
    let parent_id = trace.parent_id.expect("ParentId");

    assert_eq!(trace_id.len(), 32);
    assert!(trace_id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    assert_eq!(parent_id.len(), 16);
    assert!(parent_id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));

    let mut args = vec![0x93, 0x84];
    args.push(0xA7);
    args.extend_from_slice(b"TraceId");
    args.extend_from_slice(&[0xD9, 0x20]);
    args.extend_from_slice(trace_id.as_bytes());
    args.push(0xA8);
    args.extend_from_slice(b"ParentId");
    args.push(0xB0);
    args.extend_from_slice(parent_id.as_bytes());
    args.push(0xAA);
    args.extend_from_slice(b"TraceFlags");
    args.push(0x00);
    args.push(0xAA);
    args.extend_from_slice(b"TraceState");
    args.push(0x82);
    args.push(0xAC);
    args.extend_from_slice(b"ypScenarioId");
    args.push(0xB4);
    args.extend_from_slice(b"SendConnectedMessage");
    args.push(0xAB);
    args.extend_from_slice(b"ypTriggerId");
    args.push(0xB5);
    args.extend_from_slice(b"ConnectingWithPartner");
    args.push(0xA0 + PHONE.len() as u8);
    args.extend_from_slice(PHONE.as_bytes());
    args.push(0x80);

    assert_eq!(hub::to_bytes(&Value::Array(reply.arguments.clone())), args);

    let mut payload = vec![0x96, 0x01, 0x80, 0xA1, b'1', 0xB2];
    payload.extend_from_slice(b"SendConnectedAsync");
    payload.extend_from_slice(&args);
    payload.push(0x90);

    assert_eq!(reply.payload, payload, "[1, {{}}, \"1\", \"SendConnectedAsync\", [ctx, target, {{}}], []]");
}

#[tokio::test]
async fn region_of_on_partner_connected_may_be_any_value() {
    let mut h = Harness::new(|_| {}).await;
    let session = h.open_session().await;

    let shapes = [
        ("as-string", Value::from("westus")),
        ("as-map", Value::Map(vec![(Value::from("RegionName"), Value::from("westus"))])),
        ("as-nil", Value::Nil),
        ("as-int", Value::from(7)),
    ];

    for (id, region) in shapes {
        session.send(on_partner_connected(id, region));
        next_event(&mut h.events, 2 * SECOND, connected(id)).await;
    }

    session.wait_invocations_of(method::SEND_CONNECTED, 4, 2 * SECOND).await;
}

#[tokio::test]
async fn on_partner_connected_accepts_a_nil_trace_context() {
    let mut h = Harness::new(|_| {}).await;
    let session = h.open_session().await;

    // `[sourceId, ctx = nil, region = nil]`: the trace context packet may be nil.
    session.invoke(method::ON_PARTNER_CONNECTED, vec![Value::from(PHONE), Value::Nil, Value::Nil]);
    let reply = session.wait_invocation(2 * SECOND, |i| i.target == method::SEND_CONNECTED).await;
    assert_eq!(reply.target_id(), Some(PHONE));
    next_event(&mut h.events, SECOND, connected(PHONE)).await;
    assert!(h.relay.is_partner_connected(&phone()));

    let undecodable = h.tap.snapshot().into_iter().filter(|e| e.layer == TapLayer::Undecodable).count();
    assert_eq!(undecodable, 0, "a nil trace context is not a binding failure");
}

#[tokio::test]
async fn flush_connected_awaits_the_completion() {
    let h = Harness::new(|_| {}).await;
    let session = h.open_session().await;

    assert_eq!(h.relay.flush_connected(&phone()).await, HubSendResult::Success);
    let sent = session.invocations();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].target, method::SEND_CONNECTED);
    assert_eq!(sent[0].target_id(), Some(PHONE));

    h.hub.set_completion_for(method::SEND_CONNECTED, CompletionMode::Error("nope".into()));
    assert_eq!(h.relay.send_connected(&phone()).await, HubSendResult::Failure);
}

// ------------------------------------------------------------------------------------------------
// OnConnected
// ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn on_connected_payload_wakes_region_waiters_and_is_replaced() {
    let mut h = Harness::new(|_| {}).await;
    let session = h.open_session().await;

    let mut waiter = std::pin::pin!(h.relay.hub_region(Environment::Prod, 3 * SECOND));
    assert!(poll_once(&mut waiter).await.is_pending(), "no region before OnConnected; the waiter waits");
    session.send(on_connected(Some("westeurope"), &["phone-a", "phone-b"]));
    assert_eq!(waiter.await, Ok(Some("westeurope".to_string())));

    let event = next_event(&mut h.events, SECOND, |e| matches!(e, RelayEvent::OnConnected { .. })).await;
    assert_eq!(event, RelayEvent::OnConnected { env: Environment::Prod, region: Some("westeurope".into()) });
    assert_eq!(h.relay.resolved_region(Environment::Prod).as_deref(), Some("westeurope"));
    assert_eq!(
        h.relay.on_connected_payload(Environment::Prod).unwrap().partners,
        Some(vec!["phone-a".to_string(), "phone-b".to_string()])
    );
    assert!(h.relay.partners(Environment::Prod).is_empty(), "OnConnected.Partners is informational only");

    session.send(on_connected(None, &[]));
    next_event(&mut h.events, SECOND, |e| matches!(e, RelayEvent::OnConnected { region: None, .. })).await;
    assert_eq!(h.relay.hub_region(Environment::Prod, SECOND).await, Ok(None));
}

#[tokio::test]
async fn region_wait_times_out_without_on_connected() {
    let h = Harness::new(|_| {}).await;
    let _session = h.open_session().await;
    let started = Instant::now();
    assert_eq!(h.relay.hub_region(Environment::Prod, ms(200)).await, Err(HubRegionTimeout));
    assert!(started.elapsed() >= ms(190));
    assert_eq!(h.relay.hub_region(Environment::Beta, ms(50)).await, Err(HubRegionTimeout));
}

// ------------------------------------------------------------------------------------------------
// Heartbeats
// ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_partner_is_removed_after_three_missed_heartbeat_windows() {
    let mut h = Harness::new(short_heartbeats).await;
    let session = h.open_session().await;
    assert_eq!(session.upgrade.header("dcg-heartbeatfrequency"), Some("0.1"));

    let announced = Instant::now();
    session.send(on_partner_connected(PHONE, Value::Nil));
    next_event(&mut h.events, SECOND, connected(PHONE)).await;

    let removed = next_event(&mut h.events, 3 * SECOND, disconnected(PHONE)).await;
    let elapsed = announced.elapsed();

    assert_eq!(
        removed,
        RelayEvent::PartnerDisconnected { partner: phone(), reason: PartnerDisconnectReason::ExceededMissedHeartbeats }
    );
    assert!(elapsed >= ms(420) && elapsed < ms(1500), "removed after {elapsed:?}; three windows are 450 ms");
    assert!(!h.relay.is_partner_connected(&phone()));
}

#[tokio::test]
async fn heartbeats_matched_by_suffix_keep_the_partner() {
    let mut h = Harness::new(short_heartbeats).await;
    let session = h.open_session().await;
    let id = "0123456789abcdef-phone";
    session.send(on_partner_connected(id, Value::Nil));
    next_event(&mut h.events, SECOND, connected(id)).await;

    for _ in 0..10 {
        tokio::time::sleep(ms(100)).await;
        session.send(on_partner_heartbeat("cdef-phone", "conn-xyz"));
        // An unmatched suffix changes nothing.
        session.send(on_partner_heartbeat("not-a-suffix", "conn-xyz"));
    }

    assert!(h.relay.is_partner_connected(&prod(id)), "heartbeats re-arm the window");
    assert!(drain_events(&mut h.events).iter().all(|e| !disconnected(id)(e)));

    let last = Instant::now();
    let removed = next_event(&mut h.events, 3 * SECOND, disconnected(id)).await;
    assert!(last.elapsed() >= ms(400), "removed {:?} after the last heartbeat", last.elapsed());
    assert!(matches!(
        removed,
        RelayEvent::PartnerDisconnected { reason: PartnerDisconnectReason::ExceededMissedHeartbeats, .. }
    ));
}

#[tokio::test]
async fn messages_rearm_the_window_but_session_based_messages_do_not() {
    let mut h = Harness::new(short_heartbeats).await;
    h.relay.set_packet_sink(Arc::new(RecordingPacketSink::default()));
    let session = h.open_session().await;
    let ack = Ack::success("s", 1).to_packet();

    // OnReceiveMessage re-arms.
    session.send(on_partner_connected("messenger", Value::Nil));
    next_event(&mut h.events, SECOND, connected("messenger")).await;
    // A partner known only from session-based messages has no window at all.
    session.send(on_receive_session_based_message("session-only", &ack, "cs-1"));
    next_event(&mut h.events, SECOND, connected("session-only")).await;
    // OnPartnerConnected arms; session-based messages afterwards do not re-arm.
    session.send(on_partner_connected("sb-armed", Value::Nil));
    next_event(&mut h.events, SECOND, connected("sb-armed")).await;

    let armed = Instant::now();
    let mut sb_removed_at = None;
    for _ in 0..9 {
        tokio::time::sleep(ms(100)).await;
        session.send(on_receive_message("messenger", &ack));
        session.send(on_receive_session_based_message("sb-armed", &ack, "cs-2"));
        if sb_removed_at.is_none() && !h.relay.is_partner_connected(&prod("sb-armed")) {
            sb_removed_at = Some(armed.elapsed());
        }
    }

    assert!(h.relay.is_partner_connected(&prod("messenger")));
    assert!(h.relay.is_partner_connected(&prod("session-only")));
    let removed_after = sb_removed_at.expect("session-based messages did not keep sb-armed");
    assert!(removed_after >= ms(400), "sb-armed removed after {removed_after:?}");
}

#[tokio::test]
async fn frequency_zero_runs_no_windows() {
    // One thread: the captured log holds every line of this relay.
    let log = session_log::capture_thread();
    let mut h = Harness::new(|c| c.relay.heartbeat_frequency = Duration::ZERO).await;
    let session = h.open_session().await;
    assert_eq!(session.upgrade.header("dcg-heartbeatfrequency"), Some("0"));

    session.send(on_partner_connected(PHONE, Value::Nil));
    next_event(&mut h.events, SECOND, connected(PHONE)).await;

    session.send(on_partner_heartbeat(PHONE, "conn"));
    assert!(
        eventually(|| log.logged("heartbeat ignored; HeartBeatFrequency is 0")).await,
        "the heartbeat finds no window to re-arm"
    );
    assert!(h.relay.is_partner_connected(&phone()));
}

// ------------------------------------------------------------------------------------------------
// Partner set changes
// ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn on_partner_disconnected_removes_with_notice_from_hub() {
    let mut h = Harness::new(|_| {}).await;
    let session = h.open_session().await;
    session.send(on_partner_connected(PHONE, Value::Nil));
    next_event(&mut h.events, SECOND, connected(PHONE)).await;

    session.send(on_partner_disconnected("someone-not-in-the-set"));
    session.send(on_partner_disconnected(PHONE));

    let removed = next_event(&mut h.events, SECOND, disconnected(PHONE)).await;
    assert_eq!(
        removed,
        RelayEvent::PartnerDisconnected { partner: phone(), reason: PartnerDisconnectReason::NoticeFromHub }
    );
    assert!(!h.relay.is_partner_connected(&phone()));
}

#[tokio::test]
async fn remove_partner_reports_the_callers_reason() {
    let mut h = Harness::new(|_| {}).await;
    let session = h.open_session().await;
    session.send(on_partner_connected(PHONE, Value::Nil));
    next_event(&mut h.events, SECOND, connected(PHONE)).await;
    HubLink::remove_partner(h.relay.as_ref(), &phone(), PartnerDisconnectReason::TooManyAckTimeouts);
    let removed = next_event(&mut h.events, SECOND, disconnected(PHONE)).await;
    assert_eq!(
        removed,
        RelayEvent::PartnerDisconnected { partner: phone(), reason: PartnerDisconnectReason::TooManyAckTimeouts }
    );

    // Removing a partner that is not in the set reports nothing.
    HubLink::remove_partner(h.relay.as_ref(), &phone(), PartnerDisconnectReason::TooManyAckTimeouts);
    assert!(drain_events(&mut h.events).is_empty(), "a removal reports while it removes");
}

#[tokio::test]
async fn wait_partner_connected_waits_for_the_partner_set() {
    let h = Harness::new(|_| {}).await;
    let session = h.open_session().await;
    let target = phone();

    let mut waiter = std::pin::pin!(h.relay.wait_partner_connected(&target, 3 * SECOND));
    assert!(poll_once(&mut waiter).await.is_pending(), "the waiter waits for the partner");
    session.send(on_partner_connected(PHONE, Value::Nil));
    assert!(waiter.await);

    assert!(!h.relay.wait_partner_connected(&prod("nobody"), ms(150)).await);
    assert!(h.relay.wait_partner_connected(&phone(), ms(10)).await, "already present");
}

// ------------------------------------------------------------------------------------------------
// Hand-off to the DCG layer and auto-assignment
// ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn relayed_packets_reach_the_packet_sink_in_arrival_order() {
    let mut h = Harness::new(|_| {}).await;
    let sink = Arc::new(RecordingPacketSink::default());
    h.relay.set_packet_sink(sink.clone());
    let session = h.open_session().await;

    let ack = Ack::success("6f9619ff-8b86-d011-b42d-00c04fc964ff", 1).to_packet();
    let fragment = fragment_packet();
    session.send_coalesced(&[on_receive_message(PHONE, &ack), on_receive_message(PHONE, &fragment)]);
    session.send(on_receive_session_based_message(PHONE, &ack, "cs-1"));
    let packets = sink.wait(3, 2 * SECOND).await;

    assert_eq!(packets[0].sender, phone());
    assert_eq!(packets[0].packet, ack);
    assert_eq!(packets[0].connection_session_id, None);
    assert_eq!(packets[1].packet, fragment);
    assert_eq!(packets[2].connection_session_id.as_deref(), Some("cs-1"));

    let trace: &TraceContext = &packets[0].trace;
    assert_eq!(trace.trace_id, "4bf92f3577b34da6a3ce929d0e0e4736", "TraceId kept");
    assert_eq!(trace.parent_id.as_deref(), Some("00f067aa0ba902b7"), "ParentId kept as parent");
    assert_ne!(trace.correlation_id, "00f067aa0ba902b7", "a new correlation id");
    assert_eq!(trace.trace_state, vec![("ypScenarioId".to_string(), "PhoneScenario".to_string())]);
    assert_ne!(packets[0].trace.correlation_id, packets[1].trace.correlation_id);

    // The phone entered the partner set once; it had a key from open(), so no auto-assignment.
    let entered: Vec<_> = drain_events(&mut h.events).into_iter().filter(|e| connected(PHONE)(e)).collect();
    assert_eq!(entered.len(), 1);
    assert!(h.relay.is_partner_connected(&phone()));
}

#[tokio::test]
async fn unknown_senders_are_auto_assigned_and_reported() {
    let mut h = Harness::new(|_| {}).await;
    let sink = Arc::new(RecordingPacketSink::default());
    h.relay.set_packet_sink(sink.clone());
    let session = h.open_session().await;
    let ack = Ack::success("s", 1).to_packet();

    // Announced first (enters the set), then its first packet maps it to this connection.
    session.send(on_partner_connected("announcer", Value::Nil));
    session.send(on_receive_message("announcer", &ack));
    session.send(on_receive_message("announcer", &ack));
    // A sender seen first through a packet: one report (it entered the set).
    session.send(on_receive_message("stranger", &ack));
    // Session-based keys create no partner connection of their own.
    session.send(on_receive_session_based_message("session-sender", &ack, "cs-7"));
    // Frames are handled in order: once a later packet of a known sender is in, the ones before
    // were handled, their reports included.
    session.send(on_receive_message("announcer", &ack));
    sink.wait(5, 2 * SECOND).await;

    let reports: Vec<String> = drain_events(&mut h.events)
        .into_iter()
        .filter_map(|e| match e {
            RelayEvent::PartnerConnected(p) => Some(p.id),
            _ => None,
        })
        .collect();
    assert_eq!(reports, vec!["announcer", "announcer", "stranger", "session-sender"]);
}

// ------------------------------------------------------------------------------------------------
// Trace contexts the receiver rejects
// ------------------------------------------------------------------------------------------------

/// A `HubRelayTraceContextPacket` value with this `TraceId` and `TraceState` (`None`: nil).
fn trace_packet(trace_id: Option<&str>, trace_state: Option<Vec<(String, String)>>) -> Value {
    TraceContextPacket {
        trace_id: trace_id.map(str::to_string),
        parent_id: Some("00f067aa0ba902b7".into()),
        trace_flags: 0,
        trace_state,
    }
    .to_value()
}

/// Every way a trace context is rejected.
fn rejected_trace_contexts() -> Vec<(&'static str, Value)> {
    let state = Some(vec![("ypScenarioId".to_string(), "PhoneScenario".to_string())]);
    let valid = "4bf92f3577b34da6a3ce929d0e0e4736";
    vec![
        ("a nil TraceId", trace_packet(None, state.clone())),
        ("an all-zero TraceId", trace_packet(Some(&"0".repeat(32)), state.clone())),
        ("a short TraceId", trace_packet(Some(&valid[..16]), state.clone())),
        ("a TraceId with a dash", trace_packet(Some("4bf92f35-7b34da6a3ce929d0e0e4736"), state.clone())),
        ("a nil TraceState", trace_packet(Some(valid), None)),
    ]
}

#[tokio::test]
async fn a_rejected_trace_context_drops_the_packet_and_leaves_the_presence_untouched() {
    let mut h = Harness::new(|_| {}).await;
    let sink = Arc::new(RecordingPacketSink::default());
    h.relay.set_packet_sink(sink.clone());
    let session = h.open_session().await;
    let ack = Ack::success("s", 1).to_packet();
    let cases = rejected_trace_contexts();

    for (_, ctx) in &cases {
        session.invoke(method::ON_RECEIVE_MESSAGE, vec![Value::from("stranger"), ctx.clone(), ack.to_value()]);
        session.invoke(
            method::ON_RECEIVE_SESSION_BASED_MESSAGE,
            vec![Value::from("stranger"), ctx.clone(), ack.to_value(), Value::from("cs-1")],
        );
    }

    // Frames are handled in order: once a valid packet arrives, the rejected ones were handled.
    session.send(on_receive_message(PHONE, &ack));
    let packets = sink.wait(1, 2 * SECOND).await;
    assert_eq!(packets[0].sender, phone(), "no rejected packet reached the DCG layer");
    assert_eq!(sink.packets().len(), 1);
    assert!(!h.relay.is_partner_connected(&prod("stranger")), "the sender was not added");

    let events = drain_events(&mut h.events);
    assert!(events.iter().all(|e| !connected("stranger")(e)), "no partner event, no auto-assignment: {events:?}");

    let undecodable = h.tap.snapshot().into_iter().filter(|e| e.layer == TapLayer::Undecodable).count();
    assert_eq!(undecodable, 2 * cases.len(), "each drop is reported");
    assert_eq!(h.relay.hub_state(Environment::Prod), HubState::Connected);
}

#[tokio::test]
async fn a_message_with_a_rejected_trace_context_does_not_rearm_the_window() {
    let mut h = Harness::new(short_heartbeats).await;
    h.relay.set_packet_sink(Arc::new(RecordingPacketSink::default()));
    let session = h.open_session().await;
    let ack = Ack::success("s", 1).to_packet();

    let announced = Instant::now();
    session.send(on_partner_connected(PHONE, Value::Nil));
    next_event(&mut h.events, SECOND, connected(PHONE)).await;

    let rejected = trace_packet(Some(&"0".repeat(32)), Some(vec![]));
    let sender = session.clone();
    let messages = tokio::spawn(async move {
        for _ in 0..12 {
            tokio::time::sleep(ms(100)).await;
            sender.invoke(method::ON_RECEIVE_MESSAGE, vec![Value::from(PHONE), rejected.clone(), ack.to_value()]);
        }
    });

    let removed = next_event(&mut h.events, 3 * SECOND, disconnected(PHONE)).await;
    let elapsed = announced.elapsed();
    messages.abort();

    assert_eq!(
        removed,
        RelayEvent::PartnerDisconnected { partner: phone(), reason: PartnerDisconnectReason::ExceededMissedHeartbeats }
    );
    assert!(elapsed < ms(1100), "the rejected messages kept the partner for {elapsed:?}");
}

#[tokio::test]
async fn a_nil_trace_context_marks_the_sender_connected_but_drops_the_packet() {
    let mut h = Harness::new(|_| {}).await;
    let sink = Arc::new(RecordingPacketSink::default());
    h.relay.set_packet_sink(sink.clone());
    let session = h.open_session().await;
    let ack = Ack::success("s", 1).to_packet();

    session.invoke(method::ON_RECEIVE_MESSAGE, vec![Value::from("stranger"), Value::Nil, ack.to_value()]);
    next_event(&mut h.events, SECOND, connected("stranger")).await;
    assert!(h.relay.is_partner_connected(&prod("stranger")), "the presence takes a nil trace context");

    session.send(on_receive_message(PHONE, &ack));
    let packets = sink.wait(1, 2 * SECOND).await;
    assert_eq!(packets[0].sender, phone(), "the data path dropped the packet with the nil trace context");

    // With a trace context the next packet is handed over, and only now is the sender auto-assigned.
    session.send(on_receive_message("stranger", &ack));
    let packets = sink.wait(2, 2 * SECOND).await;
    assert_eq!(packets[1].sender, prod("stranger"));
    next_event(&mut h.events, SECOND, connected("stranger")).await;
}

#[tokio::test]
async fn on_partner_connected_with_a_rejected_trace_context_is_answered_but_not_added() {
    let mut h = Harness::new(|_| {}).await;
    let session = h.open_session().await;
    let cases = rejected_trace_contexts();

    for (_, ctx) in &cases {
        session.invoke(method::ON_PARTNER_CONNECTED, vec![Value::from(PHONE), ctx.clone(), Value::Nil]);
    }

    let replies = session.wait_invocations_of(method::SEND_CONNECTED, cases.len(), 2 * SECOND).await;
    assert!(replies.iter().all(|r| r.target_id() == Some(PHONE)), "every announcement is answered");

    // Frames are handled in order: once a later announcement is in, the rejected ones were handled.
    session.send(on_partner_connected("marker", Value::Nil));
    next_event(&mut h.events, SECOND, connected("marker")).await;
    assert!(!h.relay.is_partner_connected(&phone()), "but the partner is not added");
    assert!(drain_events(&mut h.events).iter().all(|e| !connected(PHONE)(e)));

    session.send(on_partner_connected(PHONE, Value::Nil));
    next_event(&mut h.events, SECOND, connected(PHONE)).await;
}

// ------------------------------------------------------------------------------------------------
// Unknown or malformed inbound traffic
// ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn unknown_and_malformed_inbound_traffic_keeps_the_connection() {
    let mut h = Harness::new(|_| {}).await;
    let sink = Arc::new(RecordingPacketSink::default());
    h.relay.set_packet_sink(sink.clone());
    h.hub.set_completion_for(method::SEND_MESSAGE, CompletionMode::Never);
    let session = h.open_session().await;

    // A send whose completion will come back with an unknown result kind.
    let relay = h.relay.clone();
    let pending = tokio::spawn(async move {
        HubLink::send_packet(
            relay.as_ref(),
            &phone(),
            &TraceContext::new_root(Vec::new()),
            Ack::success("s", 1).to_packet(),
            None,
        )
        .await
    });
    session.wait_invocation(SECOND, |i| i.target == method::SEND_MESSAGE).await;

    // Unknown message type.
    session.send_raw(frame(Value::Array(vec![Value::from(42), Value::from("x")])));
    // Not MessagePack at all (0xC1 is never used).
    session.send_raw({
        let mut out = Vec::new();
        hub::write_frame(&[0xC1], &mut out);
        out
    });
    // A message whose type is not an integer.
    session.send_raw(frame(Value::Array(vec![Value::Nil])));
    // Wrong argument count.
    session.invoke(method::ON_PARTNER_CONNECTED, vec![Value::from(PHONE)]);
    // An argument that fails to bind.
    session.invoke(method::ON_RECEIVE_MESSAGE, vec![Value::from(PHONE), trace_value(), Value::from("not a map")]);
    // An unregistered hub method.
    session.invoke("OnSomethingNew", vec![Value::from(1)]);
    // An unknown completion result kind for the pending send.
    session.send_raw(frame(Value::Array(vec![Value::from(3), Value::Map(vec![]), Value::from("1"), Value::from(9)])));
    // Stream messages the client never uses.
    session.send(HubMessage::CancelInvocation { headers: vec![], invocation_id: "x".into() });
    session.send(HubMessage::Sequence { sequence_id: 4 });
    // Then a valid announcement still works on the same session.
    session.send(on_partner_connected(PHONE, Value::Nil));
    session.wait_invocation(2 * SECOND, |i| i.target == method::SEND_CONNECTED).await;
    next_event(&mut h.events, SECOND, connected(PHONE)).await;

    assert_eq!(
        tokio::time::timeout(SECOND, pending).await.expect("the pending send completes").unwrap(),
        HubSendResult::Failure,
        "an unknown completion kind fails its invocation instead of leaving it pending"
    );

    // A length prefix over 2 GB desynchronizes only the rest of that WebSocket message.
    session.send_raw(vec![0xFF, 0xFF, 0xFF, 0xFF, 0x7F, 0x00]);
    session.send(on_partner_disconnected(PHONE));
    next_event(&mut h.events, SECOND, disconnected(PHONE)).await;

    assert!(sink.packets().is_empty(), "the binding failure dropped that OnReceiveMessage");
    assert!(!session.is_closed());
    assert_eq!(h.relay.hub_state(Environment::Prod), HubState::Connected);
    assert_eq!(h.hub.sessions().len(), 1, "no reconnect happened");
    let undecodable = h.tap.snapshot().into_iter().filter(|e| e.layer == TapLayer::Undecodable).count();
    assert!(undecodable >= 7, "each case is reported to the tap ({undecodable})");
}
