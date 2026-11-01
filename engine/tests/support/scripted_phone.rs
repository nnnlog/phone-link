//! A scripted phone at the `ms-dcg` level, built only from the codecs: it consumes the PC's packets
//! (acks arrive as acks, fragments are acked and reassembled into messages) and builds the hub
//! arguments that carry its own acks and messages to the PC. The test decides what the phone answers.

#![allow(dead_code)]

use std::collections::HashMap;

use phonelink_engine::codec::{dcg_packet, envelope, hub, valueset};
use phonelink_engine::types::{HeaderMap, MessageType, TraceContext, TransportMessage, header};
use rmpv::Value;

/// What one PC packet meant to the phone.
#[derive(Debug)]
pub enum FromPc {
    /// The PC acknowledged one of the phone's fragments.
    Ack(dcg_packet::Ack),
    /// A fragment arrived; `ack` must be sent back. `message` is set when it completed a message.
    Fragment { ack: dcg_packet::Ack, message: Option<TransportMessage> },
    /// Not an `ms-dcg` packet the phone understands.
    Other(String),
}

/// Reassembly slots keyed `(MessageId, SessionId)`: the fragment count and one slot per fragment.
type Pending = HashMap<(i32, String), (i32, Vec<Option<Vec<u8>>>)>;

pub struct ScriptedPhone {
    pub id: String,
    session_id: String,
    next_sequence: i32,
    next_message_id: i32,
    pending: Pending,
}

impl ScriptedPhone {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            session_id: uuid::Uuid::new_v4().hyphenated().to_string(),
            next_sequence: 1,
            next_message_id: 1,
            pending: HashMap::new(),
        }
    }

    /// Interprets the `HubRelayMultiplexPacket` argument of a PC `SendMessageAsync` invocation.
    pub fn on_pc_packet(&mut self, packet_value: &Value) -> FromPc {
        let packet = match hub::MultiplexPacket::from_value(packet_value) {
            Ok(p) => p,
            Err(e) => return FromPc::Other(e.to_string()),
        };

        match dcg_packet::parse_packet(&packet) {
            Ok(dcg_packet::ParsedPacket::Ack(a)) => FromPc::Ack(a),
            Ok(dcg_packet::ParsedPacket::Fragment(f)) => {
                let ack = dcg_packet::Ack::success(f.session_id.clone(), f.sequence_number);
                let key = (f.message_id, f.session_id.clone());

                let entry = self
                    .pending
                    .entry(key.clone())
                    .or_insert_with(|| (f.fragment_count, vec![None; f.fragment_count.max(0) as usize]));

                let idx = (f.fragment_id - 1) as usize;
                if f.fragment_id >= 1 && idx < entry.1.len() && entry.1[idx].is_none() {
                    entry.1[idx] = Some(f.raw.clone());
                }

                let message = if entry.1.iter().all(Option::is_some) {
                    let (_, parts) = self.pending.remove(&key).unwrap();
                    let bytes: Vec<u8> = parts.into_iter().flatten().flatten().collect();
                    let env = envelope::decode(&bytes).expect("PC sent a malformed envelope");
                    Some(TransportMessage::new(f.message_type(), env.headers, env.payload))
                } else {
                    None
                };

                FromPc::Fragment { ack, message }
            }
            Ok(other) => FromPc::Other(format!("{other:?}")),
            Err(e) => FromPc::Other(e.to_string()),
        }
    }

    /// `OnReceiveMessage` arguments `[sender, ctx, data]` carrying `ack` to the PC.
    pub fn ack_arguments(&self, ack: &dcg_packet::Ack) -> Vec<Value> {
        self.arguments(ack.to_packet())
    }

    /// `OnReceiveMessage` arguments carrying `message` as one fragment; returns them with the
    /// fragment's `SequenceNumber` (the PC's ack echoes it).
    pub fn message_arguments(&mut self, message: &TransportMessage) -> (Vec<Value>, i32) {
        let bytes = envelope::encode(&message.headers, &message.payload).expect("phone envelope");

        let sequence_number = self.next_sequence;
        self.next_sequence += 1;
        let message_id = self.next_message_id;
        self.next_message_id += 1;

        let fragment = dcg_packet::Fragment {
            version: dcg_packet::VERSION,
            session_id: self.session_id.clone(),
            sequence_number,
            message_id,
            message_type_raw: Some(message.message_type.wire_value() as i32),
            fragment_id: 1,
            fragment_count: 1,
            raw: bytes,
        };
        (self.arguments(fragment.to_packet()), sequence_number)
    }

    fn arguments(&self, packet: hub::MultiplexPacket) -> Vec<Value> {
        let ctx = TraceContext::new_root(Vec::new());
        vec![Value::from(self.id.as_str()), hub::TraceContextPacket::from_context(&ctx).to_value(), packet.to_value()]
    }
}

/// A Platform response to `request`: `/internal/response` with `_originalRequestId`.
pub fn platform_response(request: &TransportMessage, content_type: &str, body: Vec<u8>) -> TransportMessage {
    let mut headers = HeaderMap::new();
    headers.insert(header::ROUTE, header::RESPONSE_ROUTE);
    headers.insert(header::ORIGINAL_REQUEST_ID, request.headers.get(header::REQUEST_ID).expect("request id"));
    headers.insert(header::REJECTION_VERSION, "1");
    headers.insert(header::MS_CONTENT_TYPE, content_type);
    TransportMessage::new(MessageType::Platform, headers, body)
}

/// An App response to `request` carrying `vs` (with the `result` header when `vs` has an `Int32` `result`).
pub fn app_response(request: &TransportMessage, vs: &valueset::ValueSet) -> TransportMessage {
    let mut headers = HeaderMap::new();
    headers.insert(header::ORIGINAL_REQUEST_ID, request.headers.get(header::REQUEST_ID).expect("request id"));
    headers.insert(header::ROUTE, header::RESPONSE_ROUTE);
    if let Some(r) = vs.i32("result") {
        headers.insert(header::RESULT, r.to_string());
    }
    headers.insert(header::REJECTION_VERSION, "1");

    TransportMessage::new(MessageType::App, headers, valueset::encode(vs))
}

/// A phone-originated App request on `route` with `vs` as payload.
pub fn app_request(route: &str, request_id: &str, vs: &valueset::ValueSet) -> TransportMessage {
    let mut headers = HeaderMap::new();
    headers.insert(header::REQUEST_ID, request_id);
    headers.insert(header::ROUTE, route);
    headers.insert(header::REJECTION_VERSION, "1");
    TransportMessage::new(MessageType::App, headers, valueset::encode(vs))
}

/// A phone-originated Platform request on `route` with a `text/json` body.
pub fn platform_request(route: &str, request_id: &str, json_body: &str) -> TransportMessage {
    let mut headers = HeaderMap::new();
    headers.insert(header::ROUTE, route);
    headers.insert(header::REQUEST_ID, request_id);
    headers.insert(header::REJECTION_VERSION, "1");
    headers.insert(header::MS_CONTENT_TYPE, header::CONTENT_TYPE_JSON);
    TransportMessage::new(MessageType::Platform, headers, json_body.as_bytes().to_vec())
}
