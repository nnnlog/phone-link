//! The receiver: ack, dedupe, reassemble and dispatch, the handling of unknown or invalid inbound
//! traffic, session-based replies, and the envelope reader rules.
//!
//! Per inbound packet, in arrival order: (1) the packet checks of [`parse_packet`]; (2) the trust
//! check — an untrusted fragment is dropped without ack, an untrusted ack is still delivered to the
//! sender loop, tagged; (3) ack first, asynchronously, for every accepted fragment (duplicates,
//! out-of-range ones and ones without `Raw` included) over the inbound `connectionSessionId` — the
//! hub connection opened first, the ack sent once; (4) reassembly under
//! `(source, MessageId, SessionId)` with no timer eviction, the message taking its type, trace
//! context and `connectionSessionId` from the fragment that created its entry; then the envelope is
//! parsed and the message handed to the [`MessageSink`](phonelink_core::seams::MessageSink) once.

use std::collections::BTreeMap;
use std::sync::Weak;

use tokio::sync::mpsc;

use super::DcgTransport;
use phonelink_core::codec::dcg_packet::{Ack, Fragment, ParsedPacket, parse_packet};
use phonelink_core::codec::envelope;
use phonelink_core::codec::hub::MultiplexPacket;
use phonelink_core::sync::MutexExt;
use phonelink_core::tap::Direction;
use phonelink_core::types::{
    ConnectReason, DcgClient, HubSendResult, MessageType, OpenStatus, TraceContext, TransportMessage,
};

/// One packet handed over by the relay.
pub(super) struct Inbound {
    pub sender: DcgClient,
    pub trace: TraceContext,
    pub packet: MultiplexPacket,
    pub connection_session_id: Option<String>,
}

/// `(source DcgClient, MessageId, SessionId)`.
pub(super) type ReassemblyKey = (DcgClient, i32, String);

/// A pending message: what the fragment that created it fixed — the slot count, the message type,
/// the trace context and the `connectionSessionId`, all taken from that fragment — and the filled
/// slots. A slot holds `None` for a fragment that arrived without `Raw`.
pub(super) struct Reassembly {
    count: i32,
    message_type: MessageType,
    trace: TraceContext,
    connection_session_id: Option<String>,
    parts: BTreeMap<i32, Option<Vec<u8>>>,
    /// Every fragment arrived but the message could not be assembled (a fragment without `Raw`, an
    /// envelope that does not decode). The key is kept, so every later fragment of the key is
    /// dropped as already seen.
    failed: bool,
}

/// The receive task: one packet at a time, in arrival order; ends with the transport.
pub(super) async fn receive_loop(transport: Weak<DcgTransport>, mut rx: mpsc::UnboundedReceiver<Inbound>) {
    while let Some(item) = rx.recv().await {
        let Some(transport) = transport.upgrade() else { return };
        transport.process_inbound(item);
    }
}

impl DcgTransport {
    fn process_inbound(&self, item: Inbound) {
        let Inbound { sender, trace, packet, connection_session_id } = item;
        self.tap_packet(Direction::In, &sender, &packet, connection_session_id.as_deref());

        let parsed = match parse_packet(&packet) {
            Ok(parsed) => parsed,
            Err(error) => {
                // Not ms-dcg, Version > 1.0 on a fragment, a bad Type or a missing/mistyped
                // property: dropped without reply.
                self.undecodable(&sender, "ms-dcg packet", &error.to_string(), packet.raw.as_deref());
                return;
            }
        };

        match parsed {
            ParsedPacket::Presence(packet_type) => {
                tracing::debug!(partner = %sender, packet_type, "presence packet ignored");
            }
            ParsedPacket::Ack(ack) => self.on_ack(&sender, ack),
            ParsedPacket::Fragment(fragment) => {
                if !self.trust.is_trusted(&sender) {
                    if self.first_untrusted(&sender) {
                        tracing::warn!(partner = %sender, message_id = fragment.message_id, "fragment from an untrusted source dropped without ack");
                    } else {
                        tracing::debug!(partner = %sender, message_id = fragment.message_id, "fragment from an untrusted source dropped without ack");
                    }
                    self.trust.on_untrusted_source(&sender);
                    return;
                }

                self.trusted_again(&sender);
                self.send_ack(&sender, &trace, &fragment, connection_session_id.clone());

                // `parse_packet` reads a nil `Raw` as an empty payload; the packet tells them apart.
                let raw = packet.raw.is_some().then_some(fragment.raw.clone());
                self.reassemble(sender, trace, fragment, raw, connection_session_id);
            }
        }
    }

    /// Acks reach the sender loop of the partner with the same `dcgClientId` and environment.
    fn on_ack(&self, sender: &DcgClient, ack: Ack) {
        let trusted = self.trust.is_trusted(sender);
        if trusted {
            self.trusted_again(sender);
        } else if self.first_untrusted(sender) {
            tracing::warn!(partner = %sender, sequence_number = ack.sequence_number, "ack from a source that is no longer trusted");
        } else {
            tracing::debug!(partner = %sender, sequence_number = ack.sequence_number, "ack from a source that is no longer trusted");
        }

        let target = self.targets.lock_or_panic().get(sender).cloned();
        match target {
            Some(target) => target.deliver_ack(ack, trusted),
            None => {
                tracing::debug!(partner = %sender, sequence_number = ack.sequence_number, "ack from a partner the PC never sent to ignored")
            }
        }
    }

    /// `{ Version: 1.0, Type: 1, SessionId, SequenceNumber, Success: true, ErrorNo: 0 }` to the
    /// fragment's sender, sent asynchronously before reassembly over the connection serving the
    /// sender — opened first, as for a fragment — with `SendSessionBasedMessageAsync` when the
    /// fragment arrived with a `connectionSessionId`. Sent once: a failed open or send is logged, never
    /// retried.
    fn send_ack(
        &self,
        to: &DcgClient,
        trace: &TraceContext,
        fragment: &Fragment,
        connection_session_id: Option<String>,
    ) {
        let sequence_number = fragment.sequence_number;
        let packet = Ack::success(fragment.session_id.clone(), sequence_number).to_packet();
        let me = self.me.clone();
        let to = to.clone();
        let trace = trace.clone();

        tokio::spawn(async move {
            let Some(transport) = me.upgrade() else { return };
            let opener = transport.opener.clone();
            drop(transport);

            let opened = opener.open_for(&to, ConnectReason::SendAcknowledgement, &trace).await;
            if opened.status != OpenStatus::Success {
                tracing::warn!(
                    partner = %to,
                    sequence_number,
                    open_status = ?opened.status,
                    error = opened.cause.as_deref().unwrap_or_default(),
                    "ack not sent; the hub connection is not open"
                );
                return;
            }

            let Some(transport) = me.upgrade() else { return };
            transport.tap_packet(Direction::Out, &to, &packet, connection_session_id.as_deref());
            let hub = transport.hub.clone();
            drop(transport);

            match hub.send_packet(&to, &trace, packet, connection_session_id).await {
                HubSendResult::Success => {}
                other => {
                    tracing::warn!(partner = %to, sequence_number, result = ?other, "ack not sent; the phone sends the fragment again")
                }
            }
        });
    }

    /// Reassembly, then the envelope and the hand-off. `raw` is `None` for a fragment that
    /// arrived without `Raw`.
    fn reassemble(
        &self,
        source: DcgClient,
        trace: TraceContext,
        fragment: Fragment,
        raw: Option<Vec<u8>>,
        connection_session_id: Option<String>,
    ) {
        let key: ReassemblyKey = (source.clone(), fragment.message_id, fragment.session_id.clone());
        let Some(complete) = self.store_fragment(&key, &source, &fragment, trace, raw, connection_session_id) else {
            return;
        };

        let Reassembly { count, message_type, trace, connection_session_id, parts, .. } = complete;
        let fragments = parts.len();

        if parts.values().any(Option::is_none) {
            // Such a fragment is stored and the message fails when the payloads are concatenated;
            // every fragment was already acked and the message is discarded without reply.
            self.undecodable(&source, "fragment payloads", "a fragment of the message arrived without Raw", None);
            self.keep_failed(key, count, message_type, trace);
            return;
        }

        let bytes = join_parts(parts);
        let decoded = match envelope::decode(&bytes) {
            Ok(decoded) => decoded,
            Err(error) => {
                // Every fragment was already acked; the message is discarded without reply.
                self.undecodable(&source, "envelope", &error.to_string(), Some(&bytes));
                self.keep_failed(key, count, message_type, trace);
                return;
            }
        };

        let message = TransportMessage::new(message_type, decoded.headers, decoded.payload);
        self.tap_envelope(Direction::In, &source, &message, Some(fragment.message_id), fragments);

        match self.message_sink() {
            Some(sink) => sink.on_message(source, connection_session_id, trace, message),
            None if self.first_without_sink() => {
                tracing::error!(partner = %source, "inbound message dropped; no message sink is set");
            }
            None => tracing::debug!(partner = %source, "inbound message dropped; no message sink is set"),
        }
    }

    /// Stores one fragment under the reassembly lock; the message once its last fragment is stored,
    /// `None` (logged) when the fragment is dropped or the message is still incomplete.
    fn store_fragment(
        &self,
        key: &ReassemblyKey,
        source: &DcgClient,
        fragment: &Fragment,
        trace: TraceContext,
        raw: Option<Vec<u8>>,
        connection_session_id: Option<String>,
    ) -> Option<Reassembly> {
        let mut table = self.reassembly.lock_or_panic();

        if !table.contains_key(key) && fragment.fragment_count < 1 {
            // A fragment count below 1 creates no pending message: nothing is stored, so the key
            // stays free.
            tracing::warn!(partner = %source, message_id = fragment.message_id, fragment_count = fragment.fragment_count, "fragment with a FragmentCount below 1 dropped");
            return None;
        }

        let entry = table.entry(key.clone()).or_insert_with(|| Reassembly {
            count: fragment.fragment_count,
            message_type: fragment.message_type(),
            trace,
            connection_session_id,
            parts: BTreeMap::new(),
            failed: false,
        });

        if !fragment_fits(source, fragment, entry) {
            return None;
        }

        entry.parts.insert(fragment.fragment_id, raw);
        if entry.parts.len() != usize::try_from(entry.count).expect("a positive count fits usize") {
            return None;
        }
        Some(table.remove(key).expect("the entry was just updated"))
    }

    /// Keeps the key of a message whose assembly failed, so its later fragments are dropped
    /// (`Reassembly::failed`). Nothing removes it while the process runs.
    fn keep_failed(&self, key: ReassemblyKey, count: i32, message_type: MessageType, trace: TraceContext) {
        let failed = Reassembly {
            count,
            message_type,
            trace,
            connection_session_id: None,
            parts: BTreeMap::new(),
            failed: true,
        };

        self.reassembly.lock_or_panic().insert(key, failed);
    }
}

/// Whether a fragment fits its pending message: an id within its own count, a count no larger than
/// the message's, and an id not seen before in a message that has not failed; a misfit is logged.
fn fragment_fits(source: &DcgClient, fragment: &Fragment, entry: &Reassembly) -> bool {
    let id = fragment.fragment_id;

    // The id is checked against the fragment's own count, the count against the slot count.
    if id < 1 || id > fragment.fragment_count {
        tracing::warn!(partner = %source, message_id = fragment.message_id, fragment_id = id, fragment_count = fragment.fragment_count, case = "DroppedFragmentIdOutOfBounds", "fragment with a FragmentId out of bounds dropped");
        return false;
    }
    if fragment.fragment_count > entry.count {
        tracing::warn!(partner = %source, message_id = fragment.message_id, fragment_count = fragment.fragment_count, expected_count = entry.count, case = "DroppedFragmentCountChange", "fragment with a grown FragmentCount dropped");
        return false;
    }
    if entry.failed || entry.parts.contains_key(&id) {
        tracing::debug!(partner = %source, message_id = fragment.message_id, fragment_id = id, failed = entry.failed, case = "DroppedFragmentAlreadySeen", "duplicate fragment dropped");
        return false;
    }

    true
}

/// The payload of a complete message whose every fragment carried `Raw`: its parts in fragment
/// order, concatenated.
fn join_parts(parts: BTreeMap<i32, Option<Vec<u8>>>) -> Vec<u8> {
    let fragments = parts.len();
    let total: usize = parts.values().flatten().map(Vec::len).sum();
    let mut parts = parts.into_values().flatten();

    if fragments == 1 {
        parts.next().expect("a complete message has its one part")
    } else {
        let mut bytes = Vec::with_capacity(total);
        for part in parts {
            bytes.extend_from_slice(&part);
        }
        bytes
    }
}
