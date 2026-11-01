//! End to end: `DcgTransport` ↔ `RelayManager` ↔ fake hub ↔ a scripted peer that acknowledges
//! fragments, across the relay's hand-off to the DCG layer. Real time.

use crate::support::fake_hub;
use crate::support::relay_world as fixtures;

use std::sync::Arc;
use std::time::Duration;

use fake_hub::{
    FakeHub, HubSession, message_fragments, on_partner_connected, on_receive_message, on_receive_session_based_message,
    reassemble,
};
use fixtures::{Harness, PHONE, RecordingMessageSink, TrustSet, next_event, phone};
use phonelink_engine::codec::dcg_packet::{Ack, Fragment, ParsedPacket};
use phonelink_engine::codec::envelope;
use phonelink_engine::codec::hub::method;
use phonelink_engine::config::EngineConfig;
use phonelink_engine::dcg::{DcgIdentifiers, DcgTransport};
use phonelink_engine::seams::{MessageTransport, RelayEvent};
use phonelink_engine::types::{
    DcgClient, DcgSendStatus, Environment, HeaderMap, MessageType, OutboundMessage, PartnerDisconnectReason,
    SendOptions, TraceContext, TransportMessage,
};
use rmpv::Value;

const SECOND: Duration = Duration::from_secs(1);
const PHONE_SESSION: &str = "9b2e5f1c-0d3a-4e7b-8c6d-5a4b3c2d1e0f";

struct E2e {
    h: Harness,
    trust: Arc<TrustSet>,
    dcg: Arc<DcgTransport>,
    sink: Arc<RecordingMessageSink>,
    session: Arc<HubSession>,
}

async fn setup(adjust: impl FnOnce(&mut EngineConfig)) -> E2e {
    let hub = FakeHub::start().await;
    hub.auto_ack_fragments(PHONE);
    let mut h = Harness::with_hub(hub, adjust);
    let trust = TrustSet::trusting(&[phone(), DcgClient::new("silent-peer", Environment::Prod)]);

    // The relay opens the connection before each fragment and ack (an open one is used as it is);
    // identifiers of this transport's own, so the first MessageId and SequenceNumber are 1.
    let dcg = DcgTransport::new(
        h.config.clone(),
        h.relay.clone(),
        h.relay.clone(),
        trust.clone(),
        h.tap.clone(),
        DcgIdentifiers::new(),
    );
    h.relay.set_packet_sink(dcg.clone());
    let sink = Arc::new(RecordingMessageSink::default());
    dcg.set_message_sink(sink.clone());

    let session = h.open_session().await;
    session.send(on_partner_connected(PHONE, Value::Nil));
    next_event(&mut h.events, 2 * SECOND, |e| matches!(e, RelayEvent::PartnerConnected(p) if *p == phone())).await;

    E2e { h, trust, dcg, sink, session }
}

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    HeaderMap::from_pairs(pairs.iter().copied()).unwrap()
}

fn phone_fragments(envelope: &[u8], size: usize, message_id: i32, first_sequence: i32) -> Vec<Fragment> {
    let parts: Vec<&[u8]> = envelope.chunks(size).collect();
    parts
        .iter()
        .enumerate()
        .map(|(k, part)| Fragment {
            version: 1.0,
            session_id: PHONE_SESSION.into(),
            sequence_number: first_sequence + k as i32,
            message_id,
            message_type_raw: Some(0),
            fragment_id: k as i32 + 1,
            fragment_count: parts.len() as i32,
            raw: part.to_vec(),
        })
        .collect()
}

#[tokio::test]
async fn a_multi_fragment_message_reaches_the_peer_and_is_acknowledged() {
    let e = setup(|c| c.dcg.fragment_size = 64).await;
    let message = TransportMessage::new(
        MessageType::Platform,
        headers(&[("_route", "/SessionValidation"), ("_requestId", "1"), ("ms-content-type", "text/json")]),
        serde_json::to_vec(&serde_json::json!({ "key": "x".repeat(150) })).unwrap(),
    );

    let status = e
        .dcg
        .send(OutboundMessage {
            target: phone(),
            message: message.clone(),
            priority: 0,
            options: SendOptions::NONE,
            connection_session_id: None,
            trace: TraceContext::new_root(Vec::new()),
        })
        .await;
    assert_eq!(status, DcgSendStatus::Success);

    let fragments = e.h.hub.peer_fragments(PHONE);
    let bytes: Vec<u8> = fragments.iter().flat_map(|f| f.raw.clone()).collect();
    assert_eq!(fragments.len(), bytes.len().div_ceil(64));
    assert!(fragments.len() > 2);

    let session_id = e.dcg.session_id(&phone());
    for (k, fragment) in fragments.iter().enumerate() {
        assert_eq!(fragment.fragment_id, k as i32 + 1);
        assert_eq!(fragment.sequence_number, k as i32 + 1);
        assert_eq!(fragment.message_id, 1);
        assert_eq!(fragment.message_type_raw, Some(1));
        assert_eq!(fragment.session_id, session_id);
    }

    let decoded = envelope::decode(&bytes).unwrap();
    assert_eq!(decoded.headers, message.headers);
    assert_eq!(decoded.payload, message.payload);
    assert_eq!(reassemble(&fragments), vec![message.clone()], "the fake hub's reassembly helper agrees");

    let sends = e.session.invocations().into_iter().filter(|i| i.target == method::SEND_MESSAGE).count();
    assert_eq!(sends, fragments.len(), "one SendMessageAsync per fragment, no retries");
}

#[tokio::test]
async fn a_message_from_the_phone_is_acked_over_the_hub_and_delivered() {
    let e = setup(|_| {}).await;
    let sent_headers = headers(&[("_route", "/legacy/phonecontent"), ("_requestId", "3e1c")]);
    let sent = TransportMessage::new(MessageType::App, sent_headers.clone(), vec![0x0A, 0x00]);
    let fragments = message_fragments(&sent, PHONE_SESSION, 77, 500, 20);
    assert!(fragments.len() >= 2);
    assert_eq!(fragments, phone_fragments(&envelope::encode(&sent_headers, &[0x0A, 0x00]).unwrap(), 20, 77, 500));

    e.session.send_fragments(PHONE, &fragments);

    let messages = e.sink.wait(1, 2 * SECOND).await;
    assert_eq!(messages[0].source, phone());
    assert_eq!(messages[0].message.headers, sent_headers);
    assert_eq!(messages[0].message.payload, vec![0x0A, 0x00]);
    assert_eq!(messages[0].message.message_type, MessageType::App);

    let acks = e
        .session
        .wait_invocation(2 * SECOND, |i| {
            i.target == method::SEND_MESSAGE
                && matches!(i.dcg(), Some(ParsedPacket::Ack(a)) if a.sequence_number == 500 + fragments.len() as i32 - 1)
        })
        .await;
    assert_eq!(acks.target_id(), Some(PHONE));

    let all_acks: Vec<Ack> = e
        .session
        .invocations()
        .into_iter()
        .filter_map(|i| match i.dcg() {
            Some(ParsedPacket::Ack(a)) => Some(a),
            _ => None,
        })
        .collect();
    let mut acked: Vec<i32> = all_acks.iter().map(|a| a.sequence_number).collect();
    acked.sort_unstable();
    assert_eq!(acked, (500..500 + fragments.len() as i32).collect::<Vec<_>>());
    assert!(all_acks.iter().all(|a| a.session_id == PHONE_SESSION && a.success && a.error_no == 0));

    // The ack carries the inbound trace: the phone's TraceId, the PC's new correlation id as ParentId.
    let trace = acks.trace().unwrap();
    assert_eq!(trace.trace_id.as_deref(), Some("4bf92f3577b34da6a3ce929d0e0e4736"));
    assert_ne!(trace.parent_id.as_deref(), Some("00f067aa0ba902b7"));
    assert_eq!(trace.parent_id.as_ref().map(String::len), Some(16));
}

#[tokio::test]
async fn session_based_fragments_are_acked_session_based() {
    let e = setup(|_| {}).await;
    let envelope =
        envelope::encode(&headers(&[("_route", "/DeviceProxyClient/TransportMiddleware")]), b"proxy").unwrap();
    let fragment = phone_fragments(&envelope, 1 << 20, 5, 9).remove(0);

    e.session.send(on_receive_session_based_message(PHONE, &fragment.to_packet(), "cs-77"));
    let ack = e.session.wait_invocation(2 * SECOND, |i| i.target == method::SEND_SESSION_BASED_MESSAGE).await;
    assert_eq!(ack.connection_session_id(), Some("cs-77"));
    assert!(matches!(ack.dcg(), Some(ParsedPacket::Ack(a)) if a.sequence_number == 9));

    let messages = e.sink.wait(1, 2 * SECOND).await;
    assert_eq!(messages[0].connection_session_id.as_deref(), Some("cs-77"));
}

#[tokio::test]
async fn unacknowledged_messages_trip_the_partner_breaker_through_the_relay() {
    let mut e = setup(|c| {
        c.dcg.ack_timeout = Duration::from_millis(100);
        c.dcg.ack_retry_count = 1;
        c.dcg.partner_disconnect_ack_timeouts = 2;
    })
    .await;

    let silent = DcgClient::new("silent-peer", Environment::Prod);
    e.session.send(on_partner_connected(&silent.id, Value::Nil));
    next_event(&mut e.h.events, 2 * SECOND, |ev| matches!(ev, RelayEvent::PartnerConnected(p) if *p == silent)).await;

    let message = |options: SendOptions| OutboundMessage {
        target: silent.clone(),
        message: TransportMessage::new(
            MessageType::Platform,
            headers(&[("_route", "/x"), ("_requestId", "1")]),
            vec![],
        ),
        priority: 0,
        options,
        connection_session_id: None,
        trace: TraceContext::new_root(Vec::new()),
    };

    assert_eq!(e.dcg.send(message(SendOptions::NONE)).await, DcgSendStatus::TimedOut);
    assert_eq!(e.dcg.send(message(SendOptions::NONE)).await, DcgSendStatus::TimedOut);

    let removed =
        next_event(&mut e.h.events, 2 * SECOND, |ev| matches!(ev, RelayEvent::PartnerDisconnected { .. })).await;
    assert_eq!(
        removed,
        RelayEvent::PartnerDisconnected {
            partner: silent.clone(),
            reason: PartnerDisconnectReason::TooManyAckTimeouts
        }
    );
    assert!(!e.h.relay.is_partner_connected(&silent));

    let skip_wake = SendOptions { skip_wake_if_not_connected: true, ..SendOptions::NONE };
    assert_eq!(e.dcg.send(message(skip_wake)).await, DcgSendStatus::PartnerNotConnected);
    assert!(e.h.relay.is_partner_connected(&phone()), "other partners are unaffected");
}

#[tokio::test]
async fn an_untrusted_sender_gets_no_ack() {
    let mut e = setup(|_| {}).await;
    let envelope = envelope::encode(&headers(&[("_route", "/x")]), b"").unwrap();
    let fragment = phone_fragments(&envelope, 1 << 20, 1, 1).remove(0);

    e.session.send(on_receive_message("stranger", &fragment.to_packet()));
    next_event(&mut e.h.events, 2 * SECOND, |ev| matches!(ev, RelayEvent::PartnerConnected(p) if p.id == "stranger"))
        .await;

    // Packets are handled in order and acks go out in order: once the phone's fragment is acked, the
    // stranger's was handled.
    e.session.send(on_receive_message(PHONE, &fragment.to_packet()));
    e.session.wait_invocation(2 * SECOND, |i| i.target_id() == Some(PHONE)).await;

    assert!(e.session.invocations().iter().all(|i| i.target_id() != Some("stranger")), "no ack to an untrusted source");
    assert!(e.sink.messages().iter().all(|m| m.source == phone()), "nothing of the stranger's is delivered");
    assert_eq!(e.trust.untrusted_reports(), vec![DcgClient::new("stranger", Environment::Prod)]);
}
