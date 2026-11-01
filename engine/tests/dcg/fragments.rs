//! The `ms-dcg` fragment layer over an in-memory `HubLink` and `HubOpener`: the stop-and-wait
//! sender, identifiers and per-partner state, the receiver (ack, dedupe, reassemble, dispatch),
//! unknown or invalid inbound traffic, and session-based replies. Tokio time is paused: ack
//! timeouts elapse instantly when every task is idle.

use crate::support::relay_world as fixtures;

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use fixtures::{PHONE, RecordingMessageSink, TrustSet, phone};
use phonelink_engine::codec::dcg_packet::{Ack, Fragment, ParsedPacket, parse_packet};
use phonelink_engine::codec::envelope;
use phonelink_engine::codec::hub::{MultiplexPacket, to_bytes};
use phonelink_engine::config::EngineConfig;
use phonelink_engine::dcg::{DcgIdentifiers, DcgTransport, HubOpener};
use phonelink_engine::seams::{HubLink, MessageTransport, PacketSink, PartnerConnector};
use phonelink_engine::tap::{MemoryTap, TapLayer};
use phonelink_engine::types::{
    ConnectReason, DcgClient, DcgSendStatus, Environment, HeaderMap, HubSendResult, MessageType, OpenStatus, Opened,
    OutboundMessage, PartnerDisconnectReason, SendOptions, TraceContext, TransportMessage, WakeStatus,
};
use rmpv::Value;
use tokio::sync::Notify;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

// ------------------------------------------------------------------------------------------------
// An in-memory HubLink
// ------------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Sent {
    target: DcgClient,
    trace: TraceContext,
    packet: MultiplexPacket,
    connection_session_id: Option<String>,
}

impl Sent {
    fn fragment(&self) -> Fragment {
        match parse_packet(&self.packet) {
            Ok(ParsedPacket::Fragment(f)) => f,
            other => panic!("not a fragment: {other:?}"),
        }
    }

    fn ack(&self) -> Ack {
        match parse_packet(&self.packet) {
            Ok(ParsedPacket::Ack(a)) => a,
            other => panic!("not an ack: {other:?}"),
        }
    }

    fn is_ack(&self) -> bool {
        matches!(parse_packet(&self.packet), Ok(ParsedPacket::Ack(_)))
    }
}

type Hook = Arc<dyn Fn(&Sent) + Send + Sync>;

#[derive(Default)]
struct FakeHubLink {
    sent: Mutex<Vec<Sent>>,
    results: Mutex<VecDeque<HubSendResult>>,
    connected: Mutex<HashSet<DcgClient>>,
    removed: Mutex<Vec<(DcgClient, PartnerDisconnectReason)>>,
    hook: Mutex<Option<Hook>>,
    /// Every `HubOpener::open_for` call, and the scripted answers (default `Success`).
    opens: Mutex<Vec<(DcgClient, ConnectReason)>>,
    open_results: Mutex<VecDeque<OpenStatus>>,
    /// A `NotConnected` result also takes the target out of the partner set, as a hub connection
    /// that dropped between the partner check and the send does.
    drop_partner_on_not_connected: AtomicBool,
    notify: Notify,
}

impl FakeHubLink {
    fn opens(&self) -> Vec<(DcgClient, ConnectReason)> {
        self.opens.lock().unwrap().clone()
    }

    fn opens_for(&self, reason: ConnectReason) -> usize {
        self.opens().iter().filter(|(_, r)| *r == reason).count()
    }

    fn script_opens(&self, results: &[OpenStatus]) {
        self.open_results.lock().unwrap().extend(results.iter().copied());
    }

    fn sent(&self) -> Vec<Sent> {
        self.sent.lock().unwrap().clone()
    }

    fn fragments(&self) -> Vec<Sent> {
        self.sent().into_iter().filter(|s| !s.is_ack()).collect()
    }

    fn acks(&self) -> Vec<Sent> {
        self.sent().into_iter().filter(Sent::is_ack).collect()
    }

    fn script(&self, results: &[HubSendResult]) {
        self.results.lock().unwrap().extend(results.iter().copied());
    }

    fn connect(&self, partner: &DcgClient) {
        self.connected.lock().unwrap().insert(partner.clone());
    }

    fn disconnect(&self, partner: &DcgClient) {
        self.connected.lock().unwrap().remove(partner);
    }

    fn removed(&self) -> Vec<(DcgClient, PartnerDisconnectReason)> {
        self.removed.lock().unwrap().clone()
    }

    fn set_hook(&self, hook: Option<Hook>) {
        *self.hook.lock().unwrap() = hook;
    }

    /// Waits (virtual time) until `n` packets matching `pred` were sent.
    async fn wait_for(&self, n: usize, pred: impl Fn(&Sent) -> bool) -> Vec<Sent> {
        let deadline = Instant::now() + Duration::from_secs(3600);

        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            let found: Vec<Sent> = self.sent().into_iter().filter(|s| pred(s)).collect();
            if found.len() >= n {
                return found;
            }

            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                panic!("expected {n} matching packets, got {}", found.len());
            }
        }
    }

    async fn wait_fragments(&self, n: usize) -> Vec<Sent> {
        self.wait_for(n, |s| !s.is_ack()).await
    }

    async fn wait_acks(&self, n: usize) -> Vec<Sent> {
        self.wait_for(n, Sent::is_ack).await
    }
}

#[async_trait]
impl HubLink for FakeHubLink {
    async fn send_packet(
        &self,
        target: &DcgClient,
        trace: &TraceContext,
        packet: MultiplexPacket,
        connection_session_id: Option<String>,
    ) -> HubSendResult {
        let sent = Sent { target: target.clone(), trace: trace.clone(), packet, connection_session_id };
        self.sent.lock().unwrap().push(sent.clone());

        let result = self.results.lock().unwrap().pop_front().unwrap_or(HubSendResult::Success);
        if result == HubSendResult::Success {
            let hook = self.hook.lock().unwrap().clone();
            if let Some(hook) = hook {
                hook(&sent);
            }
        }

        if result == HubSendResult::NotConnected && self.drop_partner_on_not_connected.load(Ordering::SeqCst) {
            self.disconnect(target);
        }

        self.notify.notify_waiters();
        result
    }

    fn is_partner_connected(&self, partner: &DcgClient) -> bool {
        self.connected.lock().unwrap().contains(partner)
    }

    fn remove_partner(&self, partner: &DcgClient, reason: PartnerDisconnectReason) {
        self.removed.lock().unwrap().push((partner.clone(), reason));
    }
}

#[async_trait]
impl HubOpener for FakeHubLink {
    async fn open_for(&self, partner: &DcgClient, reason: ConnectReason, _trace: &TraceContext) -> Opened {
        self.opens.lock().unwrap().push((partner.clone(), reason));
        self.open_results.lock().unwrap().pop_front().unwrap_or(OpenStatus::Success).into()
    }
}

// ------------------------------------------------------------------------------------------------
// Fixture
// ------------------------------------------------------------------------------------------------

struct Fixture {
    hub: Arc<FakeHubLink>,
    trust: Arc<TrustSet>,
    sink: Arc<RecordingMessageSink>,
    tap: Arc<MemoryTap>,
    transport: Arc<DcgTransport>,
}

fn other() -> DcgClient {
    DcgClient::new("second-phone", Environment::Prod)
}

/// A transport with identifiers of its own (first `MessageId` and `SequenceNumber` 1).
fn fixture(adjust: impl FnOnce(&mut EngineConfig)) -> Fixture {
    fixture_with(adjust, DcgIdentifiers::new())
}

fn fixture_with(adjust: impl FnOnce(&mut EngineConfig), ids: Arc<DcgIdentifiers>) -> Fixture {
    let mut config = EngineConfig::default();
    adjust(&mut config);

    let hub = Arc::new(FakeHubLink::default());
    hub.connect(&phone());
    hub.connect(&other());
    let trust = TrustSet::trusting(&[phone(), other()]);

    let tap = MemoryTap::new();
    let transport = DcgTransport::new(Arc::new(config), hub.clone(), hub.clone(), trust.clone(), tap.clone(), ids);
    let sink = Arc::new(RecordingMessageSink::default());
    transport.set_message_sink(sink.clone());

    Fixture { hub, trust, sink, tap, transport }
}

impl Fixture {
    /// The peers acknowledge every fragment they receive (through `PacketSink::on_packet`).
    fn auto_ack(&self) {
        let transport: Weak<DcgTransport> = Arc::downgrade(&self.transport);
        self.hub.set_hook(Some(Arc::new(move |sent: &Sent| {
            if let Ok(ParsedPacket::Fragment(f)) = parse_packet(&sent.packet) {
                if let Some(transport) = transport.upgrade() {
                    let ack = Ack::success(f.session_id, f.sequence_number).to_packet();
                    transport.on_packet(sent.target.clone(), TraceContext::new_root(Vec::new()), ack, None);
                }
            }
        })));
    }

    /// Delivers `ack` as if `from` had sent it.
    fn deliver(&self, from: &DcgClient, packet: MultiplexPacket, connection_session_id: Option<&str>) {
        self.transport.on_packet(
            from.clone(),
            TraceContext::new_root(Vec::new()),
            packet,
            connection_session_id.map(str::to_string),
        );
    }

    fn ack_fragment(&self, sent: &Sent) {
        let f = sent.fragment();
        self.deliver(&sent.target, Ack::success(f.session_id, f.sequence_number).to_packet(), None);
    }

    fn spawn_send(&self, message: OutboundMessage) -> tokio::task::JoinHandle<DcgSendStatus> {
        let transport = self.transport.clone();
        tokio::spawn(async move { transport.send(message).await })
    }
}

async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    HeaderMap::from_pairs(pairs.iter().copied()).unwrap()
}

fn outbound(target: DcgClient, message_type: MessageType, payload: Vec<u8>, priority: u32) -> OutboundMessage {
    OutboundMessage {
        target,
        message: TransportMessage::new(message_type, headers(&[("_route", "/t"), ("_requestId", "7")]), payload),
        priority,
        options: SendOptions::NONE,
        connection_session_id: None,
        trace: TraceContext::new_root(vec![("ypScenarioId".into(), "Test".into())]),
    }
}

/// The derived test vector: `_route=/t`, `_requestId=7`, payload `ValueSet { "a": String "b" }` →
/// 64 envelope bytes.
const VECTOR_PAYLOAD: [u8; 12] = [0x0A, 0x0A, 0x0A, 0x01, 0x61, 0x12, 0x05, 0x08, 0x0C, 0x6A, 0x01, 0x62];
const VECTOR_ENVELOPE: &str = "
    01
    27 00 00 00
    02 00 00 00
    06 00 00 00  5F 72 6F 75 74 65
    02 00 00 00  2F 74
    0A 00 00 00  5F 72 65 71 75 65 73 74 49 64
    01 00 00 00  37
    0C 00 00 00 00 00 00 00
    0A 0A
      0A 01 61
      12 05
        08 0C
        6A 01 62";

fn vector_envelope() -> Vec<u8> {
    hex::decode(VECTOR_ENVELOPE.split_whitespace().collect::<String>()).unwrap()
}

fn keys(packet: &MultiplexPacket) -> Vec<&str> {
    packet.properties.iter().map(|(k, _)| k.as_str()).collect()
}

fn is_lower_guid(s: &str) -> bool {
    s.len() == 36
        && s.chars().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit() && !c.is_ascii_uppercase(),
        })
}

// ------------------------------------------------------------------------------------------------
// Sender
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn an_app_message_is_one_fragment_with_the_spec_property_order() {
    let f = fixture(|_| {});
    f.auto_ack();
    let message = outbound(phone(), MessageType::App, VECTOR_PAYLOAD.to_vec(), 50);
    let trace = message.trace.clone();
    assert_eq!(f.transport.send(message).await, DcgSendStatus::Success);

    let sent = f.hub.fragments();
    assert_eq!(sent.len(), 1);
    let packet = &sent[0].packet;
    assert_eq!(sent[0].target, phone());
    assert_eq!(sent[0].connection_session_id, None);
    assert_eq!(sent[0].trace, trace);

    assert_eq!(packet.handler_type.as_deref(), Some("ms-dcg"));
    assert_eq!(
        keys(packet),
        ["Version", "Type", "SessionId", "SequenceNumber", "MessageId", "MessageType", "FragmentId", "FragmentCount"]
    );
    assert_eq!(to_bytes(&packet.properties[0].1), [0xCB, 0x3F, 0xF0, 0, 0, 0, 0, 0, 0], "Version is float64 1.0");

    let session_id = f.transport.session_id(&phone());
    assert!(is_lower_guid(&session_id), "{session_id}");
    let expected = Fragment {
        version: 1.0,
        session_id,
        sequence_number: 1,
        message_id: 1,
        message_type_raw: Some(0),
        fragment_id: 1,
        fragment_count: 1,
        raw: vector_envelope(),
    };
    assert_eq!(sent[0].fragment(), expected);
    assert_eq!(packet.raw.as_ref().unwrap().len(), 64);

    let layers: Vec<TapLayer> = f.tap.snapshot().iter().map(|e| e.layer).collect();
    assert!(layers.contains(&TapLayer::Envelope) && layers.contains(&TapLayer::DcgPacket));
}

#[tokio::test(start_paused = true)]
async fn platform_messages_and_identifiers() {
    let f = fixture(|_| {});
    f.auto_ack();
    assert_eq!(f.transport.send(outbound(phone(), MessageType::Platform, vec![1], 0)).await, DcgSendStatus::Success);
    assert_eq!(f.transport.send(outbound(other(), MessageType::App, vec![2], 0)).await, DcgSendStatus::Success);
    assert_eq!(f.transport.send(outbound(phone(), MessageType::App, vec![3], 0)).await, DcgSendStatus::Success);

    let fragments: Vec<(DcgClient, Fragment)> =
        f.hub.fragments().iter().map(|s| (s.target.clone(), s.fragment())).collect();
    assert_eq!(fragments[0].1.message_type_raw, Some(1), "Platform = 1");
    assert_eq!(fragments[1].1.message_type_raw, Some(0), "App = 0");

    let message_ids: Vec<i32> = fragments.iter().map(|(_, f)| f.message_id).collect();
    assert_eq!(message_ids, [1, 2, 3], "one process-wide MessageId counter");
    let sequence_numbers: Vec<i32> = fragments.iter().map(|(_, f)| f.sequence_number).collect();
    assert_eq!(sequence_numbers, [1, 1, 2], "SequenceNumber per target");
    assert_eq!(fragments[0].1.session_id, fragments[2].1.session_id, "one SessionId per target");
    assert_ne!(fragments[0].1.session_id, fragments[1].1.session_id);
}

/// The `MessageId` counter, the `SessionId` and the `SequenceNumber` belong to the process — a
/// transport built again over the same identifiers (an engine core rebuilt in the process)
/// continues the counters and keeps the `SessionId`.
#[tokio::test(start_paused = true)]
async fn a_transport_built_again_continues_the_process_identifiers() {
    let ids = DcgIdentifiers::new();
    let first = fixture_with(|_| {}, ids.clone());
    first.auto_ack();
    for payload in [1, 2] {
        let status = first.transport.send(outbound(phone(), MessageType::App, vec![payload], 0)).await;
        assert_eq!(status, DcgSendStatus::Success);
    }

    let before = first.hub.fragments().iter().map(Sent::fragment).collect::<Vec<_>>();
    drop(first);

    let rebuilt = fixture_with(|_| {}, ids.clone());
    rebuilt.auto_ack();
    assert_eq!(rebuilt.transport.send(outbound(phone(), MessageType::App, vec![3], 0)).await, DcgSendStatus::Success);

    let after = rebuilt.hub.fragments()[0].fragment();
    assert_eq!(after.message_id, before[1].message_id + 1, "the MessageId counter continues");
    assert_eq!(after.sequence_number, before[1].sequence_number + 1, "the SequenceNumber is never reset");
    assert_eq!(after.session_id, before[0].session_id, "the SessionId is kept");
    assert!(Arc::ptr_eq(rebuilt.transport.identifiers(), &ids));

    // Fresh identifiers start again.
    let other = fixture(|_| {});
    other.auto_ack();
    assert_eq!(other.transport.send(outbound(phone(), MessageType::App, vec![4], 0)).await, DcgSendStatus::Success);

    let fresh = other.hub.fragments()[0].fragment();
    assert_eq!((fresh.message_id, fresh.sequence_number), (1, 1));
    assert_ne!(fresh.session_id, after.session_id);
}

#[tokio::test(start_paused = true)]
async fn multi_fragment_messages_carry_their_byte_ranges() {
    let f = fixture(|c| c.dcg.fragment_size = 16);
    f.auto_ack();
    let message = outbound(phone(), MessageType::App, VECTOR_PAYLOAD.to_vec(), 50);
    assert_eq!(f.transport.send(message).await, DcgSendStatus::Success);

    let envelope = vector_envelope();
    let fragments: Vec<Fragment> = f.hub.fragments().iter().map(Sent::fragment).collect();
    assert_eq!(fragments.len(), 4, "ceil(64 / 16)");

    for (k, fragment) in fragments.iter().enumerate() {
        assert_eq!(fragment.fragment_id, k as i32 + 1);
        assert_eq!(fragment.fragment_count, 4);
        assert_eq!(fragment.message_id, 1);
        assert_eq!(fragment.sequence_number, k as i32 + 1, "one SequenceNumber per fragment submission");
        assert_eq!(fragment.raw, envelope[k * 16..((k + 1) * 16).min(64)].to_vec());
    }
}

#[tokio::test(start_paused = true)]
async fn fragments_interleave_by_priority_then_message_id() {
    // Envelope overhead of `outbound` is 52 bytes: A (120-byte payload) is 3 fragments of 60
    // bytes, B and C (1-byte payloads) one each.
    let f = fixture(|c| c.dcg.fragment_size = 60);
    let a = f.spawn_send(outbound(phone(), MessageType::App, vec![0xA0; 120], 50));
    let first = f.hub.wait_fragments(1).await;
    assert_eq!(first[0].fragment().fragment_count, 3);

    // While A's first fragment waits for its ack: a high-priority B and a same-priority C.
    let b = f.spawn_send(outbound(phone(), MessageType::Platform, vec![0xB0], 0));
    settle().await;
    let c = f.spawn_send(outbound(phone(), MessageType::App, vec![0xC0], 50));
    settle().await;
    f.ack_fragment(&first[0]);

    for n in 2..=5 {
        let sent = f.hub.wait_fragments(n).await;
        f.ack_fragment(&sent[n - 1]);
    }

    assert_eq!(a.await.unwrap(), DcgSendStatus::Success);
    assert_eq!(b.await.unwrap(), DcgSendStatus::Success);
    assert_eq!(c.await.unwrap(), DcgSendStatus::Success);

    let order: Vec<(i32, i32)> =
        f.hub.fragments().iter().map(|s| s.fragment()).map(|f| (f.message_id, f.fragment_id)).collect();
    assert_eq!(order, [(1, 1), (2, 1), (1, 2), (1, 3), (3, 1)]);
}

#[tokio::test(start_paused = true)]
async fn an_ack_timeout_retries_with_the_same_sequence_number() {
    let f = fixture(|_| {});
    let started = Instant::now();
    let send = f.spawn_send(outbound(phone(), MessageType::App, vec![1, 2, 3], 50));
    f.hub.wait_fragments(1).await;
    let retried = f.hub.wait_fragments(2).await;

    assert_eq!(started.elapsed(), Duration::from_secs(15), "SignalRFragmentSendWithAckTimeout");
    assert_eq!(retried[0].fragment(), retried[1].fragment(), "same SequenceNumber, MessageId, FragmentId and bytes");

    f.ack_fragment(&retried[1]);
    assert_eq!(send.await.unwrap(), DcgSendStatus::Success);
}

#[tokio::test(start_paused = true)]
async fn late_and_mismatched_acks_are_ignored() {
    let f = fixture(|_| {});
    f.auto_ack();
    assert_eq!(f.transport.send(outbound(phone(), MessageType::App, vec![1], 50)).await, DcgSendStatus::Success);

    f.hub.set_hook(None);
    let send = f.spawn_send(outbound(phone(), MessageType::App, vec![2], 50));
    let sent = f.hub.wait_fragments(2).await;
    let in_flight = sent[1].fragment();
    assert_eq!(in_flight.sequence_number, 2);

    let session = in_flight.session_id.clone();
    f.deliver(&phone(), Ack::success(session.clone(), 1).to_packet(), None);
    f.deliver(&phone(), Ack::success(session.clone(), 9).to_packet(), None);
    // From the right id but another environment: not this target's ack.
    f.deliver(&DcgClient::new(PHONE, Environment::Beta), Ack::success(session.clone(), 2).to_packet(), None);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(!send.is_finished(), "only SequenceNumber 2 from the target completes the fragment");

    // The SessionId is carried but not compared.
    f.deliver(&phone(), Ack::success("another-session", 2).to_packet(), None);
    assert_eq!(send.await.unwrap(), DcgSendStatus::Success);
    assert_eq!(f.hub.fragments().len(), 2, "no retry was needed");
}

#[tokio::test(start_paused = true)]
async fn three_unanswered_attempts_time_out() {
    let f = fixture(|_| {});
    let started = Instant::now();
    let status = f.transport.send(outbound(phone(), MessageType::App, vec![1], 50)).await;

    assert_eq!(status, DcgSendStatus::TimedOut);
    assert_eq!(started.elapsed(), Duration::from_secs(45));
    let sequence_numbers: Vec<i32> = f.hub.fragments().iter().map(|s| s.fragment().sequence_number).collect();
    assert_eq!(sequence_numbers, [1, 1, 1]);
}

#[tokio::test(start_paused = true)]
async fn five_consecutive_timeouts_remove_the_partner() {
    let f = fixture(|c| c.dcg.ack_timeout = Duration::from_secs(1));
    for _ in 0..4 {
        assert_eq!(f.transport.send(outbound(phone(), MessageType::App, vec![1], 50)).await, DcgSendStatus::TimedOut);
    }

    // An acknowledged fragment resets the count.
    f.auto_ack();
    assert_eq!(f.transport.send(outbound(phone(), MessageType::App, vec![1], 50)).await, DcgSendStatus::Success);

    f.hub.set_hook(None);
    for _ in 0..4 {
        assert_eq!(f.transport.send(outbound(phone(), MessageType::App, vec![1], 50)).await, DcgSendStatus::TimedOut);
    }

    assert!(f.hub.removed().is_empty());
    assert_eq!(f.transport.send(outbound(phone(), MessageType::App, vec![1], 50)).await, DcgSendStatus::TimedOut);
    assert_eq!(f.hub.removed(), vec![(phone(), PartnerDisconnectReason::TooManyAckTimeouts)]);

    // The count restarts after the removal.
    for _ in 0..4 {
        assert_eq!(f.transport.send(outbound(phone(), MessageType::App, vec![1], 50)).await, DcgSendStatus::TimedOut);
    }
    assert_eq!(f.hub.removed().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_zero_breaker_threshold_never_removes() {
    let f = fixture(|c| {
        c.dcg.ack_timeout = Duration::from_secs(1);
        c.dcg.partner_disconnect_ack_timeouts = 0;
    });

    for _ in 0..7 {
        assert_eq!(f.transport.send(outbound(phone(), MessageType::App, vec![1], 50)).await, DcgSendStatus::TimedOut);
    }
    assert!(f.hub.removed().is_empty());
}

#[tokio::test(start_paused = true)]
async fn hub_failures_and_not_connected_are_retried_twice_without_delay() {
    let f = fixture(|_| {});
    f.auto_ack();
    f.hub.script(&[HubSendResult::Failure, HubSendResult::NotConnected]);
    let started = Instant::now();

    assert_eq!(f.transport.send(outbound(phone(), MessageType::App, vec![1], 50)).await, DcgSendStatus::Success);
    assert_eq!(started.elapsed(), Duration::ZERO);
    assert_eq!(f.hub.fragments().len(), 3);
    assert_eq!(f.hub.opens_for(ConnectReason::SendFragment), 3, "every try opens the hub connection first");

    f.hub.script(&[HubSendResult::Failure, HubSendResult::Failure, HubSendResult::Failure]);
    let status = f.transport.send(outbound(phone(), MessageType::App, vec![1], 50)).await;
    assert_eq!(status, DcgSendStatus::HubSendFailed { result: HubSendResult::Failure, cause: None });
    assert_eq!(f.hub.fragments().len(), 6);
    assert_eq!(started.elapsed(), Duration::ZERO, "no ack is awaited after a failed hub send");

    for result in [HubSendResult::InternetError, HubSendResult::BrokenCircuitError] {
        let before = f.hub.fragments().len();
        f.hub.script(&[result]);
        let status = f.transport.send(outbound(phone(), MessageType::App, vec![1], 50)).await;
        assert_eq!(status, DcgSendStatus::HubSendFailed { result, cause: None });
        assert_eq!(f.hub.fragments().len(), before + 1, "{result:?} is not retried");
    }
}

/// The stop-and-wait sender: a hub `NotConnected` repeats the whole try — the open, the partner
/// check and the send — so a connection that dropped after the partner check (the partner left the
/// set with it) is opened again and the partner woken before the next send; with *skip wake* the
/// repeated check fails `PartnerNotConnected`.
#[tokio::test(start_paused = true)]
async fn a_hub_send_retry_opens_and_checks_the_partner_again() {
    let f = fixture(|_| {});
    f.auto_ack();
    f.hub.drop_partner_on_not_connected.store(true, Ordering::SeqCst);
    let connector =
        Arc::new(ScriptedConnector { hub: f.hub.clone(), answer: Mutex::new(Ok(())), calls: Mutex::new(Vec::new()) });
    f.transport.set_partner_connector(connector.clone());

    f.hub.script(&[HubSendResult::NotConnected]);
    assert_eq!(f.transport.send(outbound(phone(), MessageType::App, vec![1], 50)).await, DcgSendStatus::Success);
    assert_eq!(f.hub.fragments().len(), 2);
    assert_eq!(f.hub.opens_for(ConnectReason::SendFragment), 2);
    assert_eq!(connector.calls.lock().unwrap().len(), 1, "the retry woke the partner that left with the connection");

    f.hub.script(&[HubSendResult::NotConnected]);
    let mut skip_wake = outbound(phone(), MessageType::App, vec![2], 50);
    skip_wake.options.skip_wake_if_not_connected = true;
    assert_eq!(f.transport.send(skip_wake).await, DcgSendStatus::PartnerNotConnected);
    assert_eq!(f.hub.fragments().len(), 3, "no send after the failed partner check");
    assert_eq!(connector.calls.lock().unwrap().len(), 1);
}

/// An open that fails ends the fragment with that failure: nothing is sent, no ack is awaited.
#[tokio::test(start_paused = true)]
async fn a_failed_open_ends_the_fragment_without_a_send() {
    let f = fixture(|_| {});
    for (open, expected) in [
        (OpenStatus::InternetError, DcgSendStatus::HubSendFailed { result: HubSendResult::InternetError, cause: None }),
        (
            OpenStatus::BrokenCircuitError,
            DcgSendStatus::HubSendFailed { result: HubSendResult::BrokenCircuitError, cause: None },
        ),
        (
            OpenStatus::NonTransientAuthFailure,
            DcgSendStatus::HubSendFailed { result: HubSendResult::NotConnected, cause: None },
        ),
        (OpenStatus::Cancelled, DcgSendStatus::Canceled),
    ] {
        f.hub.script_opens(&[open]);
        let started = Instant::now();
        assert_eq!(f.transport.send(outbound(phone(), MessageType::App, vec![1], 50)).await, expected, "{open:?}");
        assert_eq!(started.elapsed(), Duration::ZERO);
    }

    assert!(f.hub.sent().is_empty());
    assert_eq!(f.hub.opens_for(ConnectReason::SendFragment), 4, "an open failure is not retried");
}

#[tokio::test(start_paused = true)]
async fn skip_wake_fails_when_the_partner_is_not_connected() {
    let f = fixture(|_| {});
    f.hub.disconnect(&phone());

    let mut message = outbound(phone(), MessageType::Platform, vec![1], 0);
    message.options.skip_wake_if_not_connected = true;
    assert_eq!(f.transport.send(message).await, DcgSendStatus::PartnerNotConnected);

    // Without skip-wake and without a connector the send cannot wake either.
    assert_eq!(
        f.transport.send(outbound(phone(), MessageType::Platform, vec![1], 0)).await,
        DcgSendStatus::PartnerNotConnected
    );
    assert!(f.hub.sent().is_empty());
}

struct ScriptedConnector {
    hub: Arc<FakeHubLink>,
    answer: Mutex<Result<(), DcgSendStatus>>,
    calls: Mutex<Vec<(DcgClient, bool, Duration)>>,
}

#[async_trait]
impl PartnerConnector for ScriptedConnector {
    async fn connect_for_send(
        &self,
        target: &DcgClient,
        ignore_device_disabled: bool,
        max_wait: Duration,
        _trace: &TraceContext,
    ) -> Result<(), DcgSendStatus> {
        self.calls.lock().unwrap().push((target.clone(), ignore_device_disabled, max_wait));

        let answer = self.answer.lock().unwrap().clone();
        if answer.is_ok() {
            self.hub.connect(target);
        }
        answer
    }
}

#[tokio::test(start_paused = true)]
async fn a_send_wakes_a_partner_through_the_connector() {
    let f = fixture(|_| {});
    f.auto_ack();
    f.hub.disconnect(&phone());

    let connector = Arc::new(ScriptedConnector {
        hub: f.hub.clone(),
        answer: Mutex::new(Err(DcgSendStatus::WakeFailed { status: WakeStatus::DeviceUnreachable, cause: None })),
        calls: Mutex::new(Vec::new()),
    });
    f.transport.set_partner_connector(connector.clone());

    let mut message = outbound(phone(), MessageType::App, vec![1], 50);
    message.options.ignore_device_disabled = true;

    assert_eq!(
        f.transport.send(message.clone()).await,
        DcgSendStatus::WakeFailed { status: WakeStatus::DeviceUnreachable, cause: None }
    );
    assert!(f.hub.sent().is_empty());

    *connector.answer.lock().unwrap() = Ok(());
    assert_eq!(f.transport.send(message).await, DcgSendStatus::Success);

    assert_eq!(
        connector.calls.lock().unwrap().clone(),
        vec![(phone(), true, Duration::from_secs(70)), (phone(), true, Duration::from_secs(70))]
    );
}

/// The stop-and-wait sender: a timed-out attempt is retried from the open and the partner check, so
/// a partner that left meanwhile is woken again before the retry.
#[tokio::test(start_paused = true)]
async fn an_ack_timeout_retry_wakes_a_partner_that_left_meanwhile() {
    let f = fixture(|_| {});
    let connector =
        Arc::new(ScriptedConnector { hub: f.hub.clone(), answer: Mutex::new(Ok(())), calls: Mutex::new(Vec::new()) });
    f.transport.set_partner_connector(connector.clone());

    let send = f.spawn_send(outbound(phone(), MessageType::App, vec![1, 2, 3], 50));
    let first = f.hub.wait_fragments(1).await;
    assert!(connector.calls.lock().unwrap().is_empty(), "the partner was on the hub for the first attempt");

    f.hub.disconnect(&phone());
    let retried = f.hub.wait_fragments(2).await;
    assert_eq!(first[0].fragment(), retried[1].fragment(), "same SequenceNumber, MessageId, FragmentId and bytes");
    assert_eq!(connector.calls.lock().unwrap().len(), 1, "the retry woke the partner first");

    f.ack_fragment(&retried[1]);
    assert_eq!(send.await.unwrap(), DcgSendStatus::Success);
}

#[tokio::test(start_paused = true)]
async fn skip_partner_ack_completes_on_hub_success() {
    let f = fixture(|_| {});
    let mut message = outbound(phone(), MessageType::Platform, vec![1], 0);
    message.options.skip_partner_ack = true;

    let started = Instant::now();
    assert_eq!(f.transport.send(message).await, DcgSendStatus::Success);
    assert_eq!(started.elapsed(), Duration::ZERO);
}

type AckMutation = Box<dyn Fn(Ack) -> Ack>;

#[tokio::test(start_paused = true)]
async fn failed_acks_map_to_their_statuses() {
    let f = fixture(|_| {});
    let cases: Vec<(AckMutation, DcgSendStatus)> = vec![
        (Box::new(|a| Ack { success: false, error_no: 1, ..a }), DcgSendStatus::RemoteTransportTooOld),
        (Box::new(|a| Ack { version: 1.1, ..a }), DcgSendStatus::RemoteTransportTooNew),
        (Box::new(|a| Ack { success: false, error_no: 2, ..a }), DcgSendStatus::RemoteTransportError),
        (
            Box::new(|a| Ack { success: false, error_no: 7, error_message: Some("x".into()), ..a }),
            DcgSendStatus::RemoteTransportError,
        ),
    ];

    for (n, (mutate, expected)) in cases.into_iter().enumerate() {
        let send = f.spawn_send(outbound(phone(), MessageType::App, vec![1], 50));
        let sent = f.hub.wait_fragments(n + 1).await;
        let fragment = sent[n].fragment();
        let ack = mutate(Ack::success(fragment.session_id, fragment.sequence_number));
        f.deliver(&phone(), ack.to_packet(), None);
        assert_eq!(send.await.unwrap(), expected);
    }
}

/// The acknowledgement packet: an ack with `Version > 1.0` is read for its `SessionId` and
/// `SequenceNumber` only, so one without `Success` and `ErrorNo` still fails the waiting fragment
/// `RemoteTransportTooNew` at once; an ack whose `ErrorMessage` is nil is a parse failure
/// (dropped), one with a non-string `ErrorMessage` is read.
#[tokio::test(start_paused = true)]
async fn too_new_acks_fail_the_fragment_and_a_nil_error_message_drops_the_ack() {
    let f = fixture(|_| {});
    let send = f.spawn_send(outbound(phone(), MessageType::App, vec![1], 50));
    let sent = f.hub.wait_fragments(1).await;
    let fragment = sent[0].fragment();

    let mut too_new = Ack::success(fragment.session_id.clone(), fragment.sequence_number).to_packet();
    too_new.properties[0].1 = Value::F64(1.5);
    too_new.properties.retain(|(k, _)| k != "Success" && k != "ErrorNo");

    let started = Instant::now();
    f.deliver(&phone(), too_new, None);
    assert_eq!(send.await.unwrap(), DcgSendStatus::RemoteTransportTooNew);
    assert_eq!(started.elapsed(), Duration::ZERO, "no ack timeout");

    let send = f.spawn_send(outbound(phone(), MessageType::App, vec![2], 50));
    let sent = f.hub.wait_fragments(2).await;
    let fragment = sent[1].fragment();

    let with_message = |v: Value| {
        let mut p = Ack::success(fragment.session_id.clone(), fragment.sequence_number).to_packet();
        p.properties.push(("ErrorMessage".to_string(), v));
        p
    };

    f.deliver(&phone(), with_message(Value::Nil), None);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(!send.is_finished(), "an ack with a nil ErrorMessage is dropped");
    assert!(f.tap.snapshot().iter().any(|e| e.layer == TapLayer::Undecodable));

    f.deliver(&phone(), with_message(Value::from(5)), None);
    assert_eq!(send.await.unwrap(), DcgSendStatus::Success, "a non-string ErrorMessage is read");
}

#[tokio::test(start_paused = true)]
async fn an_ack_from_a_no_longer_trusted_source_loses_trust() {
    let f = fixture(|_| {});
    let send = f.spawn_send(outbound(phone(), MessageType::App, vec![1], 50));
    let sent = f.hub.wait_fragments(1).await;

    f.trust.distrust(&phone());
    f.ack_fragment(&sent[0]);
    assert_eq!(send.await.unwrap(), DcgSendStatus::LostTrustWithPartner);
}

#[tokio::test(start_paused = true)]
async fn cancellation_reports_canceled_and_frees_the_partner_queue() {
    let f = fixture(|_| {});
    let cancel = CancellationToken::new();
    let transport = f.transport.clone();
    let token = cancel.clone();
    let send = tokio::spawn(async move {
        transport.send_cancellable(outbound(phone(), MessageType::App, vec![1], 50), token).await
    });

    f.hub.wait_fragments(1).await;
    cancel.cancel();
    assert_eq!(send.await.unwrap(), DcgSendStatus::Canceled);

    // A dropped send future frees the queue too.
    let dropped = f.spawn_send(outbound(phone(), MessageType::App, vec![2], 50));
    f.hub.wait_fragments(2).await;
    dropped.abort();

    f.auto_ack();
    let started = Instant::now();
    assert_eq!(f.transport.send(outbound(phone(), MessageType::App, vec![3], 50)).await, DcgSendStatus::Success);
    assert!(started.elapsed() < Duration::from_secs(1), "the next message did not wait for the abandoned one");
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "DefaultQueueId")]
async fn the_default_queue_literal_is_not_a_partner() {
    let f = fixture(|_| {});
    let _ =
        f.transport.send(outbound(DcgClient::new("default", Environment::Prod), MessageType::App, vec![1], 0)).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "cannot be serialized")]
async fn a_message_without_headers_fails_loudly() {
    let f = fixture(|_| {});
    let mut message = outbound(phone(), MessageType::App, vec![1], 0);
    message.message.headers = HeaderMap::new();
    let _ = f.transport.send(message).await;
}

// ------------------------------------------------------------------------------------------------
// Receiver
// ------------------------------------------------------------------------------------------------

fn inbound_fragments(
    envelope: &[u8],
    size: usize,
    message_id: i32,
    first_sequence: i32,
    message_type: i32,
) -> Vec<Fragment> {
    let parts: Vec<&[u8]> = envelope.chunks(size).collect();
    parts
        .iter()
        .enumerate()
        .map(|(k, part)| Fragment {
            version: 1.0,
            session_id: "a5e0b0c2-3d4e-4f60-8a1b-2c3d4e5f6071".into(),
            sequence_number: first_sequence + k as i32,
            message_id,
            message_type_raw: Some(message_type),
            fragment_id: k as i32 + 1,
            fragment_count: parts.len() as i32,
            raw: part.to_vec(),
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn an_inbound_fragment_is_acked_and_its_message_delivered() {
    let f = fixture(|_| {});
    let fragment = inbound_fragments(&vector_envelope(), 1 << 20, 41, 17, 0).remove(0);
    let trace = TraceContext::new_root(vec![("ypScenarioId".into(), "FromPhone".into())]);
    f.transport.on_packet(phone(), trace.clone(), fragment.to_packet(), None);

    let messages = f.sink.wait(1, Duration::from_secs(60)).await;
    let acks = f.hub.wait_acks(1).await;

    let ack = &acks[0];
    assert_eq!(ack.target, phone());
    assert_eq!(ack.connection_session_id, None, "SendMessageAsync for a session-less fragment");
    assert_eq!(ack.trace, trace);
    assert_eq!(ack.packet.raw, None);
    assert_eq!(keys(&ack.packet), ["Version", "Type", "SessionId", "SequenceNumber", "Success", "ErrorNo"]);
    assert_eq!(ack.ack(), Ack::success(fragment.session_id.clone(), 17));

    let received = &messages[0];
    assert_eq!(received.source, phone());
    assert_eq!(received.connection_session_id, None);
    assert_eq!(received.trace, trace);
    assert_eq!(received.message.message_type, MessageType::App);
    assert_eq!(received.message.headers, headers(&[("_route", "/t"), ("_requestId", "7")]));
    assert_eq!(received.message.payload, VECTOR_PAYLOAD.to_vec());
    assert!(f.tap.snapshot().iter().any(|e| e.layer == TapLayer::Envelope));
}

#[tokio::test(start_paused = true)]
async fn platform_fragments_are_delivered_as_platform_messages() {
    let f = fixture(|_| {});
    let fragment = inbound_fragments(&vector_envelope(), 1 << 20, 1, 1, 1).remove(0);
    f.deliver(&phone(), fragment.to_packet(), None);
    assert_eq!(f.sink.wait(1, Duration::from_secs(60)).await[0].message.message_type, MessageType::Platform);

    // An absent MessageType (or any value but 1) is App.
    let mut absent = inbound_fragments(&vector_envelope(), 1 << 20, 2, 2, 0).remove(0);
    absent.message_type_raw = None;
    f.deliver(&phone(), absent.to_packet(), None);

    let mut two = inbound_fragments(&vector_envelope(), 1 << 20, 3, 3, 2).remove(0);
    two.message_type_raw = Some(2);
    f.deliver(&phone(), two.to_packet(), None);

    let messages = f.sink.wait(3, Duration::from_secs(60)).await;
    assert_eq!(messages[1].message.message_type, MessageType::App);
    assert_eq!(messages[2].message.message_type, MessageType::App);
}

#[tokio::test(start_paused = true)]
async fn an_untrusted_fragment_is_dropped_without_ack() {
    let f = fixture(|_| {});
    let stranger = DcgClient::new("stranger", Environment::Prod);
    let fragment = inbound_fragments(&vector_envelope(), 1 << 20, 1, 1, 0).remove(0);
    f.deliver(&stranger, fragment.to_packet(), None);

    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(f.hub.sent().is_empty(), "no ack for an untrusted source");
    assert!(f.sink.messages().is_empty());
    assert_eq!(f.trust.untrusted_reports(), vec![stranger]);
}

#[tokio::test(start_paused = true)]
async fn duplicate_out_of_range_and_grown_fragments_are_acked_then_discarded() {
    let f = fixture(|_| {});
    let envelope = vector_envelope();
    let parts = inbound_fragments(&envelope, 24, 5, 100, 0);
    assert_eq!(parts.len(), 3);

    let mut out_of_range_low = parts[0].clone();
    out_of_range_low.fragment_id = 0;
    out_of_range_low.sequence_number = 200;

    let mut out_of_range_high = parts[0].clone();
    out_of_range_high.fragment_id = 4;
    out_of_range_high.sequence_number = 201;

    let mut grown = parts[1].clone();
    grown.fragment_count = 4;
    grown.sequence_number = 202;

    // The id is checked against the fragment's own count: id 3 of a count of 2 is out of bounds,
    // although the slot count (3, from the first fragment) would hold it.
    let mut beyond_its_count = parts[2].clone();
    beyond_its_count.fragment_count = 2;
    beyond_its_count.sequence_number = 203;

    // A smaller count is not checked against the slot count: id 2 of a count of 2 is stored.
    let mut shrunk = parts[1].clone();
    shrunk.fragment_count = 2;

    let sequence = [
        parts[0].clone(),
        parts[0].clone(),
        out_of_range_low,
        out_of_range_high,
        grown,
        beyond_its_count,
        shrunk,
        parts[1].clone(),
    ];
    for fragment in &sequence {
        f.deliver(&phone(), fragment.to_packet(), None);
    }

    let acks = f.hub.wait_acks(sequence.len()).await;
    let mut acked: Vec<i32> = acks.iter().map(|s| s.ack().sequence_number).collect();
    acked.sort_unstable();
    let mut expected: Vec<i32> = sequence.iter().map(|f| f.sequence_number).collect();
    expected.sort_unstable();
    assert_eq!(acked, expected, "every accepted fragment is acked, discarded ones included");

    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(f.sink.messages().is_empty(), "fragment 3 was discarded: the message waits for it");

    f.deliver(&phone(), parts[2].to_packet(), None);
    let messages = f.sink.wait(1, Duration::from_secs(60)).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(f.sink.messages().len(), 1, "delivered exactly once");
    assert_eq!(messages[0].message.payload, VECTOR_PAYLOAD.to_vec());
}

/// A first fragment whose `FragmentCount` is below 1 is refused: it is acked, and nothing is
/// stored, so the key is free for the message's later fragments.
#[tokio::test(start_paused = true)]
async fn a_first_fragment_with_a_count_below_one_stores_nothing() {
    let f = fixture(|_| {});
    let good = inbound_fragments(&vector_envelope(), 1 << 20, 12, 1, 0).remove(0);

    for (sequence_number, count) in [(50, 0), (51, -3)] {
        let mut bad = good.clone();
        bad.fragment_count = count;
        bad.sequence_number = sequence_number;
        f.deliver(&phone(), bad.to_packet(), None);
    }
    f.hub.wait_acks(2).await;

    f.deliver(&phone(), good.to_packet(), None);
    let messages = f.sink.wait(1, Duration::from_secs(60)).await;
    assert_eq!(messages[0].message.payload, VECTOR_PAYLOAD.to_vec(), "the key was not blocked");
    assert_eq!(f.hub.acks().len(), 3);
}

/// The assembled message takes the message type, the trace context and the `connectionSessionId`
/// from the fragment that created the entry, not from the one that completes it.
#[tokio::test(start_paused = true)]
async fn the_message_keeps_what_its_first_fragment_carried() {
    let f = fixture(|_| {});
    let parts = inbound_fragments(&vector_envelope(), 40, 21, 1, 1);
    assert_eq!(parts.len(), 2);

    let first_trace = TraceContext::new_root(vec![("ypScenarioId".into(), "First".into())]);
    let last_trace = TraceContext::new_root(vec![("ypScenarioId".into(), "Last".into())]);
    let mut last = parts[0].clone();
    last.message_type_raw = Some(0);

    f.transport.on_packet(phone(), first_trace.clone(), parts[1].to_packet(), Some("cs-first".into()));
    f.transport.on_packet(phone(), last_trace, last.to_packet(), Some("cs-last".into()));

    let messages = f.sink.wait(1, Duration::from_secs(60)).await;
    assert_eq!(messages[0].message.message_type, MessageType::Platform, "the first fragment's MessageType");
    assert_eq!(messages[0].trace, first_trace);
    assert_eq!(messages[0].connection_session_id.as_deref(), Some("cs-first"));
    assert_eq!(messages[0].message.payload, VECTOR_PAYLOAD.to_vec());
}

/// A fragment's nil `Raw` is taken as it is: the fragment is acked and stored, and its message
/// fails when it completes — discarded without reply, like a malformed envelope.
#[tokio::test(start_paused = true)]
async fn a_fragment_without_raw_is_acked_and_its_message_discarded() {
    let f = fixture(|_| {});
    let mut single = inbound_fragments(&vector_envelope(), 1 << 20, 31, 1, 0).remove(0).to_packet();
    single.raw = None;
    f.deliver(&phone(), single, None);

    let parts = inbound_fragments(&vector_envelope(), 40, 32, 2, 0);
    let mut second = parts[1].to_packet();
    second.raw = None;
    f.deliver(&phone(), parts[0].to_packet(), None);
    f.deliver(&phone(), second, None);

    let acks = f.hub.wait_acks(3).await;
    assert_eq!(acks.len(), 3, "every fragment is acked");

    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(f.sink.messages().is_empty());
    assert!(f.tap.snapshot().iter().filter(|e| e.layer == TapLayer::Undecodable).count() >= 2);

    // The receiver goes on.
    f.deliver(&phone(), inbound_fragments(&vector_envelope(), 1 << 20, 33, 4, 0).remove(0).to_packet(), None);
    f.sink.wait(1, Duration::from_secs(60)).await;
}

/// The key of a message that cannot be assembled when it completes stays taken, so every later
/// fragment of that key is dropped as already seen; another message id is unaffected.
#[tokio::test(start_paused = true)]
async fn the_key_of_a_message_that_failed_to_assemble_stays_taken() {
    let f = fixture(|_| {});
    let mut bad = vector_envelope();
    bad[5..9].copy_from_slice(&0i32.to_le_bytes()); // headerCount 0
    f.deliver(&phone(), inbound_fragments(&bad, 1 << 20, 41, 1, 0).remove(0).to_packet(), None);
    f.hub.wait_acks(1).await;

    // The same key again, now with a valid envelope (new sequence numbers pass the dedupe).
    f.deliver(&phone(), inbound_fragments(&vector_envelope(), 1 << 20, 41, 2, 0).remove(0).to_packet(), None);
    f.hub.wait_acks(2).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(f.sink.messages().is_empty(), "the failed key keeps dropping its fragments");

    f.deliver(&phone(), inbound_fragments(&vector_envelope(), 1 << 20, 42, 3, 0).remove(0).to_packet(), None);
    f.sink.wait(1, Duration::from_secs(60)).await;
}

#[tokio::test(start_paused = true)]
async fn out_of_order_fragments_reassemble_in_fragment_id_order() {
    let f = fixture(|_| {});
    let envelope = vector_envelope();
    let parts = inbound_fragments(&envelope, 10, 8, 1, 1);
    assert_eq!(parts.len(), 7);

    for index in [6, 2, 0, 5, 1, 4, 3] {
        f.deliver(&phone(), parts[index].to_packet(), None);
    }

    let messages = f.sink.wait(1, Duration::from_secs(60)).await;
    assert_eq!(messages[0].message.payload, VECTOR_PAYLOAD.to_vec());
    assert_eq!(messages[0].message.message_type, MessageType::Platform);

    // Different senders or SessionIds never share a reassembly slot.
    let mut foreign = parts[0].clone();
    foreign.session_id = "another-session".into();
    f.deliver(&phone(), foreign.to_packet(), None);
    f.deliver(&other(), parts[0].to_packet(), None);
    f.hub.wait_acks(9).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(f.sink.messages().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_completed_message_resent_after_ack_loss_is_delivered_again() {
    let f = fixture(|_| {});
    let fragment = inbound_fragments(&vector_envelope(), 1 << 20, 3, 1, 0).remove(0);
    f.deliver(&phone(), fragment.to_packet(), None);
    f.deliver(&phone(), fragment.to_packet(), None);
    let messages = f.sink.wait(2, Duration::from_secs(60)).await;
    assert_eq!(messages.len(), 2, "completed keys are forgotten: delivery is at-least-once");
}

#[tokio::test(start_paused = true)]
async fn session_based_fragments_are_acked_with_their_connection_session_id() {
    let f = fixture(|_| {});
    let fragment = inbound_fragments(&vector_envelope(), 1 << 20, 4, 9, 0).remove(0);
    f.deliver(&phone(), fragment.to_packet(), Some("cs-42"));

    let acks = f.hub.wait_acks(1).await;
    assert_eq!(acks[0].connection_session_id.as_deref(), Some("cs-42"));

    let messages = f.sink.wait(1, Duration::from_secs(60)).await;
    assert_eq!(messages[0].connection_session_id.as_deref(), Some("cs-42"));
}

#[tokio::test(start_paused = true)]
async fn a_malformed_envelope_is_discarded_after_acking() {
    let f = fixture(|_| {});
    let mut bad = vector_envelope();
    bad[5..9].copy_from_slice(&0i32.to_le_bytes()); // headerCount 0
    let fragment = inbound_fragments(&bad, 1 << 20, 6, 3, 0).remove(0);

    f.deliver(&phone(), fragment.to_packet(), None);
    f.hub.wait_acks(1).await;

    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(f.sink.messages().is_empty());
    assert!(f.tap.snapshot().iter().any(|e| e.layer == TapLayer::Undecodable));
}

#[tokio::test(start_paused = true)]
async fn invalid_packets_are_dropped_without_reply() {
    let f = fixture(|_| {});
    let good = inbound_fragments(&vector_envelope(), 1 << 20, 1, 1, 0).remove(0).to_packet();
    let mut cases: Vec<MultiplexPacket> = Vec::new();

    let mut other_handler = good.clone();
    other_handler.handler_type = Some("ms-other".into());
    cases.push(other_handler);

    let mut no_handler = good.clone();
    no_handler.handler_type = None;
    cases.push(no_handler);

    let mut int_version = good.clone();
    int_version.properties[0].1 = Value::from(1);
    cases.push(int_version);

    let mut newer = good.clone();
    newer.properties[0].1 = Value::F64(1.5);
    cases.push(newer);

    for packet_type in [0, 6, 3, 4, 5] {
        let mut p = good.clone();
        p.properties[1].1 = Value::from(packet_type);
        cases.push(p);
    }

    let mut no_type = good.clone();
    no_type.properties.retain(|(k, _)| k != "Type");
    cases.push(no_type);

    let mut no_session = good.clone();
    no_session.properties.retain(|(k, _)| k != "SessionId");
    cases.push(no_session);

    let mut session_not_str = good.clone();
    session_not_str.properties[2].1 = Value::from(5);
    cases.push(session_not_str);

    let mut ack_success_not_bool = Ack::success("s", 1).to_packet();
    ack_success_not_bool.properties[4].1 = Value::from(1);
    cases.push(ack_success_not_bool);

    let count = cases.len();
    for packet in cases {
        f.deliver(&phone(), packet, None);
    }

    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(f.hub.sent().is_empty(), "no reply to any invalid packet");
    assert!(f.sink.messages().is_empty());
    let undecodable = f.tap.snapshot().iter().filter(|e| e.layer == TapLayer::Undecodable).count();
    assert_eq!(undecodable, count - 3, "everything but the three presence packets is reported");

    // The receiver still works.
    f.deliver(&phone(), good, None);
    f.sink.wait(1, Duration::from_secs(60)).await;
}

/// The receiver's ack: the ack's hub connection is opened first and the ack sent once — a hub
/// `Failure` or `NotConnected` is not retried, and a failed open sends nothing.
#[tokio::test(start_paused = true)]
async fn an_ack_is_sent_once_over_the_connection_opened_for_it() {
    let f = fixture(|_| {});
    for (n, result) in
        [HubSendResult::Failure, HubSendResult::NotConnected, HubSendResult::Success].into_iter().enumerate()
    {
        f.hub.script(&[result]);
        let n = i32::try_from(n).unwrap();
        let fragment = inbound_fragments(&vector_envelope(), 1 << 20, n + 1, n + 1, 0).remove(0);
        f.deliver(&phone(), fragment.to_packet(), None);
        f.hub.wait_acks(usize::try_from(n + 1).unwrap()).await;
    }

    f.hub.script_opens(&[OpenStatus::InternetError]);
    let fourth = inbound_fragments(&vector_envelope(), 1 << 20, 4, 4, 0).remove(0);
    f.deliver(&phone(), fourth.to_packet(), None);
    f.sink.wait(4, Duration::from_secs(60)).await;

    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(f.hub.acks().len(), 3, "one send each; none after the failed open");
    assert_eq!(f.hub.opens_for(ConnectReason::SendAcknowledgement), 4, "every ack opens the connection first");
    assert!(f.hub.opens().iter().all(|(partner, _)| *partner == phone()));
    assert_eq!(f.sink.messages().len(), 4, "delivery does not depend on the ack send");
}

#[tokio::test(start_paused = true)]
async fn acks_for_unknown_partners_are_ignored() {
    let f = fixture(|_| {});
    f.deliver(&phone(), Ack::success("s", 1).to_packet(), None);
    f.deliver(&DcgClient::new("stranger", Environment::Prod), Ack::success("s", 1).to_packet(), None);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(f.hub.sent().is_empty());

    f.auto_ack();
    assert_eq!(f.transport.send(outbound(phone(), MessageType::App, vec![1], 50)).await, DcgSendStatus::Success);
}

#[tokio::test(start_paused = true)]
async fn envelope_decoding_accepts_the_exclude_the_count_convention() {
    let f = fixture(|_| {});
    let mut envelope = vector_envelope();
    envelope[1..5].copy_from_slice(&35i32.to_le_bytes());
    let fragment = inbound_fragments(&envelope, 1 << 20, 1, 1, 1).remove(0);

    f.deliver(&phone(), fragment.to_packet(), None);
    let messages = f.sink.wait(1, Duration::from_secs(60)).await;

    assert_eq!(messages[0].message.headers.get("_route"), Some("/t"));
    assert_eq!(envelope::decode(&envelope).unwrap().payload, VECTOR_PAYLOAD.to_vec());
}
