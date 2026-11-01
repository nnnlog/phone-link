//! The `ms-dcg` fragment layer: per-partner stop-and-wait sender and the receiver.
//!
//! - `ids`: the `MessageId` counter and the per-target `SessionId` and `SequenceNumber`, which
//!   belong to the process.
//! - `sender`: one priority queue and exactly one sender loop per target partner; fragment attempts
//!   — each try opens the hub connection, checks (or wakes) the partner and invokes the hub, a
//!   `Failure` or `NotConnected` repeating the three —, the ack wait and its retries, and the engine's
//!   partner-disconnect breaker.
//! - `receiver`: packet checks, the trust gate, ack-first, reassembly and delivery, including unknown
//!   or invalid inbound traffic and session-based replies.
//!
//! [`DcgTransport`] sits on a [`HubLink`] (the relay) and a [`HubOpener`] below and hands
//! reassembled messages to a [`MessageSink`] (the platform router) above. Inbound traffic that fails
//! a check is logged (tracing and a tap `Undecodable` event) and dropped; nothing here tears the hub
//! connection down. The Nano protobuf DCG path is not implemented (Phone Link on Windows does not use
//! it either).

mod ids;
mod receiver;
mod sender;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use phonelink_core::codec::dcg_packet::split_into_fragments;
use phonelink_core::codec::envelope;
use phonelink_core::codec::hub::{MultiplexPacket, value_to_json};
use phonelink_core::config::EngineConfig;
use phonelink_core::seams::{HubLink, MessageSink, MessageTransport, PacketSink, PartnerConnector, TrustGate};
use phonelink_core::tap::{Direction, Tap, TapEvent, TapLayer};
use phonelink_core::types::{
    ConnectReason, DcgClient, DcgSendStatus, MessageType, Opened, OutboundMessage, TraceContext, TransportMessage,
};

pub use ids::DcgIdentifiers;
use phonelink_core::sync::{MutexExt, RwLockExt};
use receiver::{Inbound, Reassembly, ReassemblyKey};
use sender::{SendOp, Target};

/// The default queue id: no send queue may be keyed by this literal.
const DEFAULT_QUEUE_ID: &str = "default";

/// Gets the hub connection serving a partner and opens it, as every fragment try and every ack does
/// before it sends.
///
/// An open connection is used as it is (`Success` at once); otherwise the relay
/// runs its open sequence. The engine implements it over the relay.
#[async_trait]
pub trait HubOpener: Send + Sync {
    async fn open_for(&self, partner: &DcgClient, reason: ConnectReason, trace: &TraceContext) -> Opened;
}

/// The fragment layer over one hub link.
///
/// Every transport the engine builds shares the process's identifiers
/// ([`DcgIdentifiers::process`]), so the `MessageId` counter is process-wide and each target's
/// `SessionId` and `SequenceNumber` last for the process.
pub struct DcgTransport {
    me: Weak<DcgTransport>,
    config: Arc<EngineConfig>,
    hub: Arc<dyn HubLink>,
    opener: Arc<dyn HubOpener>,
    trust: Arc<dyn TrustGate>,
    tap: Tap,
    message_sink: RwLock<Option<Arc<dyn MessageSink>>>,
    connector: RwLock<Option<Arc<dyn PartnerConnector>>>,
    ids: Arc<DcgIdentifiers>,
    targets: Mutex<HashMap<DcgClient, Arc<Target>>>,
    reassembly: Mutex<HashMap<ReassemblyKey, Reassembly>>,
    /// Partners whose untrusted traffic was logged at `warn`: later traffic of theirs is logged at
    /// `debug` until a trusted packet of theirs arrives, since every fragment and ack repeats it.
    untrusted_noted: Mutex<HashSet<DcgClient>>,
    /// The missing message sink was logged at `error`; every later message logs at `debug`.
    no_sink_noted: AtomicBool,
    inbound: mpsc::UnboundedSender<Inbound>,
    shutdown: CancellationToken,
}

impl DcgTransport {
    /// Must be called inside a Tokio runtime: it starts the receive task. `ids` are the identifiers
    /// it allocates from — the process's ([`DcgIdentifiers::process`]) unless the caller needs
    /// counters of its own.
    pub fn new(
        config: Arc<EngineConfig>,
        hub: Arc<dyn HubLink>,
        opener: Arc<dyn HubOpener>,
        trust: Arc<dyn TrustGate>,
        tap: Tap,
        ids: Arc<DcgIdentifiers>,
    ) -> Arc<Self> {
        assert!(config.dcg.fragment_size > 0, "FragmentSizeInBytes must be positive");

        let (tx, rx) = mpsc::unbounded_channel();
        let transport = Arc::new_cyclic(|me| Self {
            me: me.clone(),
            config,
            hub,
            opener,
            trust,
            tap,
            message_sink: RwLock::new(None),
            connector: RwLock::new(None),
            ids,
            targets: Mutex::new(HashMap::new()),
            reassembly: Mutex::new(HashMap::new()),
            untrusted_noted: Mutex::new(HashSet::new()),
            no_sink_noted: AtomicBool::new(false),
            inbound: tx,
            shutdown: CancellationToken::new(),
        });

        tokio::spawn(receiver::receive_loop(Arc::downgrade(&transport), rx));
        transport
    }

    /// The identifiers this transport allocates from.
    pub fn identifiers(&self) -> &Arc<DcgIdentifiers> {
        &self.ids
    }

    /// Where reassembled inbound messages go (the platform router).
    pub fn set_message_sink(&self, sink: Arc<dyn MessageSink>) {
        *self.message_sink.write_or_panic() = Some(sink);
    }

    /// What a fragment send calls when the hub does not report the partner connected and the
    /// message may wake it. Without one such a send fails `PartnerNotConnected`.
    pub fn set_partner_connector(&self, connector: Arc<dyn PartnerConnector>) {
        *self.connector.write_or_panic() = Some(connector);
    }

    /// The per-target `SessionId` the PC writes on its fragments to `target` (created on first use
    /// and kept for the process lifetime).
    pub fn session_id(&self, target: &DcgClient) -> String {
        self.ids.session_id(target)
    }

    /// Sends `message` like [`MessageTransport::send`]; cancelling `cancel` (or dropping the
    /// future) abandons it with `Canceled`.
    pub async fn send_cancellable(&self, message: OutboundMessage, cancel: CancellationToken) -> DcgSendStatus {
        let OutboundMessage { target, message, priority, options, connection_session_id, trace } = message;
        let queue = self.target(&target);

        let envelope = envelope::encode(&message.headers, &message.payload).unwrap_or_else(|e| {
            panic!("dcg: outbound message to {target} cannot be serialized under the envelope writer rules: {e}")
        });

        let fragment_size = self.config.dcg.fragment_size;
        // A message no larger than the fragment size is its own single fragment (the default:
        // `FragmentSizeInBytes` is 512 MiB); the envelope then moves instead of being copied.
        let fragments: Vec<Vec<u8>> = if envelope.len() <= fragment_size {
            vec![envelope]
        } else {
            split_into_fragments(&envelope, fragment_size).into_iter().map(<[u8]>::to_vec).collect()
        };
        assert!(
            i32::try_from(fragments.len()).is_ok(),
            "dcg: FragmentCount must fit an int32 ({} fragments)",
            fragments.len()
        );

        let message_id = self.ids.next_message_id();
        self.tap_envelope(Direction::Out, &target, &message, Some(message_id), fragments.len());

        let op_cancel = cancel.child_token();
        let _cancel_when_dropped = op_cancel.clone().drop_guard();
        let (tx, rx) = oneshot::channel();
        let op = Arc::new(SendOp::new(
            message_id,
            priority,
            message.message_type,
            options,
            connection_session_id,
            trace,
            fragments,
            op_cancel,
            tx,
        ));

        queue.push(op);
        tokio::select! {
            biased;
            status = rx => status.unwrap_or(DcgSendStatus::Canceled),
            _ = cancel.cancelled() => DcgSendStatus::Canceled,
        }
    }

    /// The per-target state, created with its sender loop on first use.
    fn target(&self, client: &DcgClient) -> Arc<Target> {
        assert!(!client.id.is_empty(), "dcg: a send queue needs a non-empty dcgClientId");
        assert_ne!(client.id, DEFAULT_QUEUE_ID, "dcg: no send queue may be keyed by the DefaultQueueId literal");

        let mut targets = self.targets.lock_or_panic();
        if let Some(target) = targets.get(client) {
            return target.clone();
        }

        let target = Arc::new(Target::new(client.clone(), self.ids.target(client)));
        targets.insert(client.clone(), target.clone());
        tracing::debug!(partner = %client, session_id = %target.ids.session_id, "send queue created");
        tokio::spawn(sender::sender_loop(self.me.clone(), target.clone(), self.shutdown.clone()));
        target
    }

    fn connector(&self) -> Option<Arc<dyn PartnerConnector>> {
        self.connector.read_or_panic().clone()
    }

    fn message_sink(&self) -> Option<Arc<dyn MessageSink>> {
        self.message_sink.read_or_panic().clone()
    }

    fn tap_packet(
        &self,
        direction: Direction,
        partner: &DcgClient,
        packet: &MultiplexPacket,
        connection_session_id: Option<&str>,
    ) {
        let properties: serde_json::Map<String, serde_json::Value> =
            packet.properties.iter().map(|(k, v)| (k.clone(), value_to_json(v))).collect();

        self.tap.emit(TapEvent::new(
            TapLayer::DcgPacket,
            direction,
            Some(partner.to_string()),
            serde_json::json!({
                "handler_type": packet.handler_type,
                "properties": properties,
                "raw_len": packet.raw.as_ref().map(Vec::len),
                "connection_session_id": connection_session_id,
            }),
            packet.raw.as_deref(),
        ));
    }

    fn tap_envelope(
        &self,
        direction: Direction,
        partner: &DcgClient,
        message: &TransportMessage,
        message_id: Option<i32>,
        fragments: usize,
    ) {
        let headers: Vec<serde_json::Value> = message.headers.iter().map(|(k, v)| serde_json::json!([k, v])).collect();
        let message_type = match message.message_type {
            MessageType::App => "App",
            MessageType::Platform => "Platform",
        };

        self.tap.emit(TapEvent::new(
            TapLayer::Envelope,
            direction,
            Some(partner.to_string()),
            serde_json::json!({
                "message_type": message_type,
                "message_id": message_id,
                "fragments": fragments,
                "headers": headers,
                "payload_len": message.payload.len(),
            }),
            None,
        ));
    }

    /// `true` the first time `partner` sends untrusted traffic since its last trusted packet.
    fn first_untrusted(&self, partner: &DcgClient) -> bool {
        self.untrusted_noted.lock_or_panic().insert(partner.clone())
    }

    /// A trusted packet of `partner`: its next untrusted traffic is logged at `warn` again.
    fn trusted_again(&self, partner: &DcgClient) {
        self.untrusted_noted.lock_or_panic().remove(partner);
    }

    /// `true` the first time a message finds no message sink.
    fn first_without_sink(&self) -> bool {
        !self.no_sink_noted.swap(true, Ordering::Relaxed)
    }

    fn undecodable(&self, partner: &DcgClient, what: &str, error: &str, bytes: Option<&[u8]>) {
        tracing::warn!(partner = %partner, what, error, "invalid inbound ms-dcg traffic dropped");
        self.tap.emit(TapEvent::new(
            TapLayer::Undecodable,
            Direction::In,
            Some(partner.to_string()),
            serde_json::json!({ "layer": "dcg", "what": what, "error": error }),
            bytes,
        ));
    }
}

impl Drop for DcgTransport {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

#[async_trait]
impl MessageTransport for DcgTransport {
    async fn send(&self, message: OutboundMessage) -> DcgSendStatus {
        self.send_cancellable(message, CancellationToken::new()).await
    }
}

impl PacketSink for DcgTransport {
    /// Queues the packet for the receive task, which handles packets one at a time in arrival
    /// order.
    fn on_packet(
        &self,
        sender: DcgClient,
        trace: TraceContext,
        packet: MultiplexPacket,
        connection_session_id: Option<String>,
    ) {
        let item = Inbound { sender, trace, packet, connection_session_id };
        if self.inbound.send(item).is_err() {
            // The receive task ends only with the transport, which is alive here.
            tracing::error!("inbound packet dropped; the receive task stopped");
        }
    }
}
