//! Test doubles shared by the relay and DCG tests: a scripted [`TokenSource`], fixed
//! identification headers, a test configuration pointed at a [`FakeHub`], recording sinks, trust
//! gates, and a [`Harness`] that wires a [`RelayManager`] to a fake hub. `tests/main.rs`
//! declares it as `crate::support::relay_world`, next to the `crate::support::fake_hub` it uses.

#![allow(dead_code)]

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use phonelink_engine::codec::hub::MultiplexPacket;
use phonelink_engine::config::EngineConfig;
use phonelink_engine::edge::standins::FixedReachability;
use phonelink_engine::identification::ClientIdentification;
use phonelink_engine::relay::RelayManager;
use phonelink_engine::seams::{
    HubControl, MessageSink, PacketSink, RelayEvent, TokenError, TokenSource, TrustGate, TrustRotationError,
};
use phonelink_engine::tap::MemoryTap;
use phonelink_engine::types::{ConnectReason, DcgClient, Environment, OpenStatus, TraceContext, TransportMessage};
use tokio::sync::{Notify, broadcast};

use crate::support::fake_hub::{FakeHub, HubSession};

pub const PHONE: &str = "phone-dcg-client-id";

pub fn phone() -> DcgClient {
    DcgClient::new(PHONE, Environment::Prod)
}

// ------------------------------------------------------------------------------------------------
// Token source
// ------------------------------------------------------------------------------------------------

/// Answers `services-token` unless a scripted answer is queued; records every call.
pub struct FakeTokens {
    calls: Mutex<Vec<(Environment, bool)>>,
    script: Mutex<VecDeque<Result<String, TokenError>>>,
}

impl FakeTokens {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { calls: Mutex::new(Vec::new()), script: Mutex::new(VecDeque::new()) })
    }

    pub fn push(&self, answer: Result<String, TokenError>) {
        self.script.lock().unwrap().push_back(answer);
    }

    pub fn calls(&self) -> Vec<(Environment, bool)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl TokenSource for FakeTokens {
    async fn services_token(&self, env: Environment, force_refresh: bool) -> Result<String, TokenError> {
        self.calls.lock().unwrap().push((env, force_refresh));
        self.script.lock().unwrap().pop_front().unwrap_or_else(|| Ok("services-token".to_string()))
    }
}

// ------------------------------------------------------------------------------------------------
// Identification and configuration
// ------------------------------------------------------------------------------------------------

pub fn ident() -> ClientIdentification {
    ClientIdentification {
        logical_device_id: "bG9naWNhbC1kZXZpY2UtaWQtZm9yLXRlc3RzLTMyYnl0ZXM=".into(),
        app_version: "1.26082.74.0".into(),
        app_id: "MicrosoftWindows.CrossDevice_cw5n1h2txyewy".into(),
        session_id: "0f8fad5b-d9cb-469f-a165-70867728950e".into(),
        ring_name: "Public".into(),
        os: "Windows".into(),
        os_version: "10.0.26100".into(),
    }
}

/// The protocol defaults with the prod base at the fake hub and short start-retry / reconnect /
/// breaker timers; keepalive, server timeout and heartbeats keep their defaults.
pub fn test_config(hub: &FakeHub) -> EngineConfig {
    let mut config = EngineConfig::default();
    config.endpoints.prod_base = hub.base_url();
    config.relay.start_retry_sleep = Duration::from_millis(20);
    config.relay.reconnect_delays = vec![Duration::ZERO, Duration::from_millis(150), Duration::from_millis(300)];
    config.relay.too_many_requests_base = Duration::from_millis(400);
    config.relay.too_many_requests_variance = Duration::ZERO;
    config
}

// ------------------------------------------------------------------------------------------------
// Sinks and trust
// ------------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ReceivedPacket {
    pub sender: DcgClient,
    pub trace: TraceContext,
    pub packet: MultiplexPacket,
    pub connection_session_id: Option<String>,
}

/// Records every packet the relay hands over.
#[derive(Default)]
pub struct RecordingPacketSink {
    packets: Mutex<Vec<ReceivedPacket>>,
    notify: Notify,
}

impl RecordingPacketSink {
    pub fn packets(&self) -> Vec<ReceivedPacket> {
        self.packets.lock().unwrap().clone()
    }

    pub async fn wait(&self, n: usize, timeout: Duration) -> Vec<ReceivedPacket> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            let packets = self.packets();
            if packets.len() >= n {
                return packets;
            }

            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                panic!("expected {n} packets, got {}", self.packets().len());
            }
        }
    }
}

impl PacketSink for RecordingPacketSink {
    fn on_packet(
        &self,
        sender: DcgClient,
        trace: TraceContext,
        packet: MultiplexPacket,
        connection_session_id: Option<String>,
    ) {
        self.packets.lock().unwrap().push(ReceivedPacket { sender, trace, packet, connection_session_id });
        self.notify.notify_waiters();
    }
}

#[derive(Debug, Clone)]
pub struct ReceivedMessage {
    pub source: DcgClient,
    pub connection_session_id: Option<String>,
    pub trace: TraceContext,
    pub message: TransportMessage,
}

/// Records every reassembled message.
#[derive(Default)]
pub struct RecordingMessageSink {
    messages: Mutex<Vec<ReceivedMessage>>,
    notify: Notify,
}

impl RecordingMessageSink {
    pub fn messages(&self) -> Vec<ReceivedMessage> {
        self.messages.lock().unwrap().clone()
    }

    pub async fn wait(&self, n: usize, timeout: Duration) -> Vec<ReceivedMessage> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            let messages = self.messages();
            if messages.len() >= n {
                return messages;
            }

            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                panic!("expected {n} messages, got {}", self.messages().len());
            }
        }
    }
}

impl MessageSink for RecordingMessageSink {
    fn on_message(
        &self,
        source: DcgClient,
        connection_session_id: Option<String>,
        trace: TraceContext,
        message: TransportMessage,
    ) {
        self.messages.lock().unwrap().push(ReceivedMessage { source, connection_session_id, trace, message });
        self.notify.notify_waiters();
    }
}

/// Trusts the partners in its set; records `on_untrusted_source` calls.
#[derive(Default)]
pub struct TrustSet {
    trusted: Mutex<HashSet<DcgClient>>,
    untrusted_reports: Mutex<Vec<DcgClient>>,
}

impl TrustSet {
    pub fn trusting(partners: &[DcgClient]) -> Arc<Self> {
        let set = Self::default();
        set.trusted.lock().unwrap().extend(partners.iter().cloned());
        Arc::new(set)
    }

    pub fn trust(&self, partner: &DcgClient) {
        self.trusted.lock().unwrap().insert(partner.clone());
    }

    pub fn distrust(&self, partner: &DcgClient) {
        self.trusted.lock().unwrap().remove(partner);
    }

    pub fn untrusted_reports(&self) -> Vec<DcgClient> {
        self.untrusted_reports.lock().unwrap().clone()
    }
}

impl TrustGate for TrustSet {
    fn is_trusted(&self, partner: &DcgClient) -> bool {
        self.trusted.lock().unwrap().contains(partner)
    }

    fn partner_key_expiration(&self, _partner: &DcgClient) -> Option<std::time::SystemTime> {
        None
    }

    fn rotate_partner_certificate(
        &self,
        _partner: &DcgClient,
        _certificate_der: &[u8],
    ) -> Result<(), TrustRotationError> {
        Err(TrustRotationError::NotFound)
    }

    fn on_untrusted_source(&self, partner: &DcgClient) {
        self.untrusted_reports.lock().unwrap().push(partner.clone());
    }
}

// ------------------------------------------------------------------------------------------------
// Events
// ------------------------------------------------------------------------------------------------

/// Waits for the next event matching `pred`, skipping others.
pub async fn next_event(
    rx: &mut broadcast::Receiver<RelayEvent>,
    timeout: Duration,
    pred: impl Fn(&RelayEvent) -> bool,
) -> RelayEvent {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Ok(event)) if pred(&event) => return event,
            Ok(Ok(_)) => continue,
            Ok(Err(e)) => panic!("event channel: {e}"),
            Err(_) => panic!("no matching relay event within {timeout:?}"),
        }
    }
}

/// Every event received so far (non-blocking).
pub fn drain_events(rx: &mut broadcast::Receiver<RelayEvent>) -> Vec<RelayEvent> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}

// ------------------------------------------------------------------------------------------------
// Harness
// ------------------------------------------------------------------------------------------------

/// A [`RelayManager`] wired to a [`FakeHub`].
pub struct Harness {
    pub hub: FakeHub,
    pub config: Arc<EngineConfig>,
    pub tokens: Arc<FakeTokens>,
    pub reachability: Arc<FixedReachability>,
    pub tap: Arc<MemoryTap>,
    pub relay: Arc<RelayManager>,
    pub events: broadcast::Receiver<RelayEvent>,
}

impl Harness {
    pub async fn new(adjust: impl FnOnce(&mut EngineConfig)) -> Self {
        let hub = FakeHub::start().await;
        Self::with_hub(hub, adjust)
    }

    pub fn with_hub(hub: FakeHub, adjust: impl FnOnce(&mut EngineConfig)) -> Self {
        let mut config = test_config(&hub);
        adjust(&mut config);
        let config = Arc::new(config);

        let tokens = FakeTokens::new();
        let reachability = Arc::new(FixedReachability::online());
        let tap = MemoryTap::new();

        let relay =
            RelayManager::new(config.clone(), Arc::new(ident()), tokens.clone(), reachability.clone(), tap.clone());
        let events = relay.subscribe();

        Self { hub, config, tokens, reachability, tap, relay, events }
    }

    pub async fn open(&self) -> OpenStatus {
        self.relay.open(&phone(), ConnectReason::ConnectRequested, &TraceContext::new_root(Vec::new())).await.status
    }

    /// Opens the prod connection and returns its first fake-hub session.
    pub async fn open_session(&self) -> Arc<HubSession> {
        assert_eq!(self.open().await, OpenStatus::Success);
        self.hub.session(self.hub.sessions().len().saturating_sub(1)).await
    }
}
