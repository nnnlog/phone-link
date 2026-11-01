//! `SignalR` `MessagePack` hub protocol, version 2: framing, the hub message layouts (types 1–9), argument
//! encoding, and the hub payload objects.
//!
//! Payload objects stay close to the wire: maps are ordered `rmpv` values, so the writer controls key
//! order and the reader skips unknown keys. `object`-typed hub arguments are kept as opaque
//! [`rmpv::Value`]s.

use rmpv::Value;

use crate::types::TraceContext;

mod decode;
pub use decode::decode_message;

/// Codec failures. The caller decides what a failure means for the connection (unknown inbound traffic
/// is logged and dropped, not fatal).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HubCodecError {
    /// A length prefix still continuing after 5 bytes, or whose 5th byte is > 7 ("over 2 GB").
    #[error("frame length prefix is invalid")]
    BadLengthPrefix,
    #[error("hub message is not valid MessagePack: {0}")]
    MessagePack(String),
    #[error("hub message is malformed: {0}")]
    Malformed(String),
    /// A completion with a result kind outside {1, 2, 3}.
    #[error("unknown completion result kind {0}")]
    UnknownCompletionKind(i64),
    /// A trace context packet the receiver rejects.
    #[error("trace context rejected: {0}")]
    RejectedTraceContext(String),
    #[error("handshake response is malformed: {0}")]
    Handshake(String),
}

// ------------------------------------------------------------------------------------------------
// Handshake
// ------------------------------------------------------------------------------------------------

/// The record separator that ends a handshake message.
pub const RECORD_SEPARATOR: u8 = 0x1E;

/// `{"protocol":"messagepack","version":2}` + `0x1E` — the ASP.NET Core `SignalR` client library's
/// default handshake.
pub const HANDSHAKE_REQUEST: &[u8] = b"{\"protocol\":\"messagepack\",\"version\":2}\x1e";

/// The server's handshake answer: `{}` (success) or `{"error":"…"}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeResponse {
    pub error: Option<String>,
}

/// Parses a handshake response at the start of `buf`.
///
/// Returns `Ok(None)` until the record separator has arrived; otherwise the response and the number
/// of bytes consumed — the server may coalesce the first hub frames after the separator in the same
/// WebSocket message, and those bytes are left for the frame reader.
pub fn parse_handshake_response(buf: &[u8]) -> Result<Option<(HandshakeResponse, usize)>, HubCodecError> {
    let Some(end) = buf.iter().position(|b| *b == RECORD_SEPARATOR) else {
        return Ok(None);
    };

    let json: serde_json::Value =
        serde_json::from_slice(&buf[..end]).map_err(|e| HubCodecError::Handshake(e.to_string()))?;
    let obj = json.as_object().ok_or_else(|| HubCodecError::Handshake("not a JSON object".into()))?;

    let error = match obj.get("error") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(other) => Some(other.to_string()),
    };
    Ok(Some((HandshakeResponse { error }, end + 1)))
}

// ------------------------------------------------------------------------------------------------
// Frames: VarInt length prefix + one MessagePack array
// ------------------------------------------------------------------------------------------------

/// Appends `payload` to `out` with its `VarInt` length prefix (7 bits per byte, least-significant group
/// first, continuation bit `0x80`).
pub fn write_frame(payload: &[u8], out: &mut Vec<u8>) {
    let mut len = payload.len();
    loop {
        let mut byte = (len & 0x7F) as u8;
        len >>= 7;
        if len != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if len == 0 {
            break;
        }
    }

    out.extend_from_slice(payload);
}

/// Reads one frame from the start of `buf`: `Ok(None)` when the prefix or payload is incomplete,
/// otherwise the payload range and the total bytes consumed.
pub fn read_frame(buf: &[u8]) -> Result<Option<(&[u8], usize)>, HubCodecError> {
    let mut len: usize = 0;
    let mut shift = 0;
    let mut idx = 0;

    loop {
        if idx == 5 {
            return Err(HubCodecError::BadLengthPrefix);
        }
        let Some(&byte) = buf.get(idx) else {
            return Ok(None);
        };
        if idx == 4 && byte > 7 {
            return Err(HubCodecError::BadLengthPrefix);
        }
        len |= ((byte & 0x7F) as usize) << shift;
        idx += 1;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
    }

    let end = idx + len;
    if buf.len() < end {
        return Ok(None);
    }
    Ok(Some((&buf[idx..end], end)))
}

/// Splits every complete frame off the front of `buf`, leaving an incomplete tail in place.
pub fn drain_frames(buf: &mut Vec<u8>) -> Result<Vec<Vec<u8>>, HubCodecError> {
    let mut frames = Vec::new();
    let mut consumed = 0;
    while let Some((payload, used)) = read_frame(&buf[consumed..])? {
        frames.push(payload.to_vec());
        consumed += used;
    }

    buf.drain(..consumed);
    Ok(frames)
}

// ------------------------------------------------------------------------------------------------
// Hub messages
// ------------------------------------------------------------------------------------------------

/// `headers` of a hub message: `map<string,string>`, empty when there are none.
pub type HubHeaders = Vec<(String, String)>;

/// The completion `resultKind` and what follows it.
#[derive(Debug, Clone, PartialEq)]
pub enum CompletionResult {
    /// Kind 1: the error text.
    Error(String),
    /// Kind 2: void.
    Void,
    /// Kind 3: a result value.
    Value(Value),
}

/// One hub message (types 1–9).
#[derive(Debug, Clone, PartialEq)]
pub enum HubMessage {
    /// Type 1. `invocation_id` is nil when absent; `stream_ids` is always written (empty).
    Invocation {
        headers: HubHeaders,
        invocation_id: Option<String>,
        target: String,
        arguments: Vec<Value>,
        stream_ids: Vec<String>,
    },
    /// Type 2 (parsed, never used).
    StreamItem { headers: HubHeaders, invocation_id: String, item: Value },
    /// Type 3.
    Completion { headers: HubHeaders, invocation_id: String, result: CompletionResult },
    /// Type 4 (parsed, never used).
    StreamInvocation {
        headers: HubHeaders,
        invocation_id: String,
        target: String,
        arguments: Vec<Value>,
        stream_ids: Vec<String>,
    },
    /// Type 5 (parsed, never used).
    CancelInvocation { headers: HubHeaders, invocation_id: String },
    /// Type 6.
    Ping,
    /// Type 7. `allow_reconnect` is read only when present.
    Close { error: Option<String>, allow_reconnect: Option<bool> },
    /// Type 8 (parsed, never produced).
    Ack { sequence_id: i64 },
    /// Type 9 (parsed, never produced).
    Sequence { sequence_id: i64 },
    /// Any other message type: skipped after its frame was consumed.
    Unknown { message_type: i64 },
}

impl HubMessage {
    /// The invocation as Phone Link on Windows sends it: `[1, {}, id, target, args, []]`.
    pub fn invocation(invocation_id: impl Into<String>, target: impl Into<String>, arguments: Vec<Value>) -> Self {
        HubMessage::Invocation {
            headers: Vec::new(),
            invocation_id: Some(invocation_id.into()),
            target: target.into(),
            arguments,
            stream_ids: Vec::new(),
        }
    }

    /// The `MessagePack` hub message type number of this message (`1`–`9`, or the unknown one).
    pub fn type_code(&self) -> i64 {
        match self {
            HubMessage::Invocation { .. } => 1,
            HubMessage::StreamItem { .. } => 2,
            HubMessage::Completion { .. } => 3,
            HubMessage::StreamInvocation { .. } => 4,
            HubMessage::CancelInvocation { .. } => 5,
            HubMessage::Ping => 6,
            HubMessage::Close { .. } => 7,
            HubMessage::Ack { .. } => 8,
            HubMessage::Sequence { .. } => 9,
            HubMessage::Unknown { message_type } => *message_type,
        }
    }
}

fn headers_value(headers: &HubHeaders) -> Value {
    Value::Map(headers.iter().map(|(k, v)| (Value::from(k.as_str()), Value::from(v.as_str()))).collect())
}

fn opt_str(v: &Option<String>) -> Value {
    match v {
        Some(s) => Value::from(s.as_str()),
        None => Value::Nil,
    }
}

fn str_array(items: &[String]) -> Value {
    Value::Array(items.iter().map(|s| Value::from(s.as_str())).collect())
}

/// Encodes one hub message as a `MessagePack` array (without the length prefix).
pub fn encode_message(msg: &HubMessage) -> Vec<u8> {
    let value = match msg {
        HubMessage::Invocation { headers, invocation_id, target, arguments, stream_ids } => Value::Array(vec![
            Value::from(1),
            headers_value(headers),
            opt_str(invocation_id),
            Value::from(target.as_str()),
            Value::Array(arguments.clone()),
            str_array(stream_ids),
        ]),
        HubMessage::StreamItem { headers, invocation_id, item } => Value::Array(vec![
            Value::from(2),
            headers_value(headers),
            Value::from(invocation_id.as_str()),
            item.clone(),
        ]),
        HubMessage::Completion { headers, invocation_id, result } => {
            let mut items = vec![Value::from(3), headers_value(headers), Value::from(invocation_id.as_str())];
            match result {
                CompletionResult::Error(e) => {
                    items.push(Value::from(1));
                    items.push(Value::from(e.as_str()));
                }
                CompletionResult::Void => items.push(Value::from(2)),
                CompletionResult::Value(v) => {
                    items.push(Value::from(3));
                    items.push(v.clone());
                }
            }
            Value::Array(items)
        }
        HubMessage::StreamInvocation { headers, invocation_id, target, arguments, stream_ids } => Value::Array(vec![
            Value::from(4),
            headers_value(headers),
            Value::from(invocation_id.as_str()),
            Value::from(target.as_str()),
            Value::Array(arguments.clone()),
            str_array(stream_ids),
        ]),
        HubMessage::CancelInvocation { headers, invocation_id } => {
            Value::Array(vec![Value::from(5), headers_value(headers), Value::from(invocation_id.as_str())])
        }
        HubMessage::Ping => Value::Array(vec![Value::from(6)]),
        HubMessage::Close { error, allow_reconnect } => {
            let mut items = vec![Value::from(7), opt_str(error)];
            if let Some(allow) = allow_reconnect {
                items.push(Value::Boolean(*allow));
            }
            Value::Array(items)
        }
        HubMessage::Ack { sequence_id } => Value::Array(vec![Value::from(8), Value::from(*sequence_id)]),
        HubMessage::Sequence { sequence_id } => Value::Array(vec![Value::from(9), Value::from(*sequence_id)]),
        HubMessage::Unknown { message_type } => Value::Array(vec![Value::from(*message_type)]),
    };

    to_bytes(&value)
}

/// Encodes `msg` and prefixes its length: one complete frame.
pub fn encode_frame(msg: &HubMessage) -> Vec<u8> {
    let payload = encode_message(msg);
    let mut out = Vec::with_capacity(payload.len() + 5);
    write_frame(&payload, &mut out);
    out
}

/// Serializes one `MessagePack` value.
pub fn to_bytes(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    rmpv::encode::write_value(&mut out, value).expect("writing MessagePack to a Vec cannot fail");
    out
}

/// Parses one `MessagePack` value from the start of `bytes`.
pub fn from_bytes(bytes: &[u8]) -> Result<Value, HubCodecError> {
    let mut cursor = bytes;
    rmpv::decode::read_value(&mut cursor).map_err(|e| HubCodecError::MessagePack(e.to_string()))
}

fn malformed(what: impl Into<String>) -> HubCodecError {
    HubCodecError::Malformed(what.into())
}

fn read_headers(v: &Value) -> Result<HubHeaders, HubCodecError> {
    match v {
        Value::Nil => Ok(Vec::new()),
        Value::Map(entries) => entries
            .iter()
            .map(|(k, v)| match (k.as_str(), v.as_str()) {
                (Some(k), Some(v)) => Ok((k.to_string(), v.to_string())),
                _ => Err(malformed("header entry is not string → string")),
            })
            .collect(),
        _ => Err(malformed("headers is not a map")),
    }
}

/// A `MessagePack` value of another shape where a string was expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StringValueError {
    #[error("{0} is not UTF-8")]
    NotUtf8(&'static str),
    #[error("{0} is not a string")]
    NotString(&'static str),
}

/// A string parameter: a UTF-8 string, or nil (`None`).
pub fn opt_string(v: &Value, what: &'static str) -> Result<Option<String>, StringValueError> {
    match v {
        Value::Nil => Ok(None),
        Value::String(s) => s.as_str().map(|s| Some(s.to_string())).ok_or(StringValueError::NotUtf8(what)),
        _ => Err(StringValueError::NotString(what)),
    }
}

fn read_opt_string(v: &Value, what: &'static str) -> Result<Option<String>, HubCodecError> {
    opt_string(v, what).map_err(|e| malformed(e.to_string()))
}

fn read_string(v: &Value, what: &'static str) -> Result<String, HubCodecError> {
    read_opt_string(v, what)?.ok_or_else(|| malformed(format!("{what} is nil")))
}

fn read_string_array(v: &Value, what: &'static str) -> Result<Vec<String>, HubCodecError> {
    match v {
        Value::Nil => Ok(Vec::new()),
        Value::Array(items) => items.iter().map(|i| read_string(i, what)).collect(),
        _ => Err(malformed(format!("{what} is not an array"))),
    }
}

fn read_i64(v: &Value, what: &str) -> Result<i64, HubCodecError> {
    v.as_i64().ok_or_else(|| malformed(format!("{what} is not an integer")))
}

// ------------------------------------------------------------------------------------------------
// Payload objects (string-keyed maps)
// ------------------------------------------------------------------------------------------------

/// Looks up `key` in a string-keyed map value; `None` when `v` is not a map or the key is absent.
pub fn map_get<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Map(entries) => entries.iter().find(|(k, _)| k.as_str() == Some(key)).map(|(_, v)| v),
        _ => None,
    }
}

/// The trace context packet (a map of 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceContextPacket {
    pub trace_id: Option<String>,
    pub parent_id: Option<String>,
    pub trace_flags: u8,
    pub trace_state: Option<Vec<(String, String)>>,
}

impl TraceContextPacket {
    /// The packet a sender writes for `ctx`: `ParentId` is the context's **correlation id**.
    pub fn from_context(ctx: &TraceContext) -> Self {
        Self {
            trace_id: Some(ctx.trace_id.clone()),
            parent_id: Some(ctx.correlation_id.clone()),
            trace_flags: ctx.flags,
            trace_state: Some(ctx.trace_state.clone()),
        }
    }

    /// `{ TraceId, ParentId, TraceFlags, TraceState }`, all keys written, nil when unset.
    pub fn to_value(&self) -> Value {
        Value::Map(vec![
            (Value::from("TraceId"), opt_str(&self.trace_id)),
            (Value::from("ParentId"), opt_str(&self.parent_id)),
            (Value::from("TraceFlags"), Value::from(self.trace_flags)),
            (
                Value::from("TraceState"),
                match &self.trace_state {
                    Some(state) => headers_value(state),
                    None => Value::Nil,
                },
            ),
        ])
    }

    /// Reads the packet by key name; `Ok(None)` for a nil argument. A value of the wrong type is a
    /// binding failure (`Err`).
    pub fn from_value(v: &Value) -> Result<Option<Self>, HubCodecError> {
        if v.is_nil() {
            return Ok(None);
        }
        if !matches!(v, Value::Map(_)) {
            return Err(malformed("trace context is not a map"));
        }

        let trace_id = match map_get(v, "TraceId") {
            Some(x) => read_opt_string(x, "TraceId")?,
            None => None,
        };
        let parent_id = match map_get(v, "ParentId") {
            Some(x) => read_opt_string(x, "ParentId")?,
            None => None,
        };
        let trace_flags = match map_get(v, "TraceFlags") {
            None | Some(Value::Nil) => 0,
            Some(x) => {
                let n = read_i64(x, "TraceFlags")?;
                u8::try_from(n).map_err(|_| malformed("TraceFlags out of range"))?
            }
        };
        let trace_state = match map_get(v, "TraceState") {
            None | Some(Value::Nil) => None,
            Some(x) => Some(read_headers(x)?),
        };

        Ok(Some(Self { trace_id, parent_id, trace_flags, trace_state }))
    }

    /// The trace context of an inbound message: `TraceId` kept, `ParentId` kept as parent, a new
    /// correlation id, flags and state kept. The packet is rejected when `TraceId` is nil or not 32
    /// ASCII letters or digits with at least one that is not `0`, or when `TraceState` is nil (an
    /// empty map is fine). What a rejection drops is the caller's (the `relay` layer).
    pub fn to_inbound_context(&self) -> Result<TraceContext, HubCodecError> {
        let Some(trace_id) = self.trace_id.as_deref() else {
            return Err(HubCodecError::RejectedTraceContext("TraceId is nil".into()));
        };

        let valid = trace_id.len() == 32
            && trace_id.bytes().all(|b| b.is_ascii_alphanumeric())
            && trace_id.bytes().any(|b| b != b'0');
        if !valid {
            return Err(HubCodecError::RejectedTraceContext(format!(
                "TraceId is not 32 letters or digits with one that is not 0 ({} bytes)",
                trace_id.len()
            )));
        }

        let Some(trace_state) = self.trace_state.clone() else {
            return Err(HubCodecError::RejectedTraceContext("TraceState is nil".into()));
        };

        Ok(TraceContext {
            trace_id: trace_id.to_string(),
            parent_id: self.parent_id.clone(),
            correlation_id: crate::types::random_nonzero_hex(8),
            flags: self.trace_flags,
            trace_state,
        })
    }
}

/// The multiplex packet (a map of 3, written in this order).
#[derive(Debug, Clone, PartialEq)]
pub struct MultiplexPacket {
    /// The `ms-dcg` property map of a fragment or acknowledgement packet, in insertion order.
    pub properties: Vec<(String, Value)>,
    /// `Type` — the handler type; the only value sent or accepted is `"ms-dcg"`.
    pub handler_type: Option<String>,
    /// `Raw` — fragment bytes, or `None` (nil) on an ack.
    pub raw: Option<Vec<u8>>,
}

impl MultiplexPacket {
    /// The value of the `ms-dcg` property `key`, or `None` when the packet does not carry it.
    pub fn property(&self, key: &str) -> Option<&Value> {
        self.properties.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
}

impl MultiplexPacket {
    /// `{ Properties, Type, Raw }` in that order; `Raw` nil when absent.
    pub fn to_value(&self) -> Value {
        Value::Map(vec![
            (
                Value::from("Properties"),
                Value::Map(self.properties.iter().map(|(k, v)| (Value::from(k.as_str()), v.clone())).collect()),
            ),
            (Value::from("Type"), opt_str(&self.handler_type)),
            (
                Value::from("Raw"),
                match &self.raw {
                    Some(bytes) => Value::Binary(bytes.clone()),
                    None => Value::Nil,
                },
            ),
        ])
    }

    /// Reads the packet by key name, keys in any order, unknown keys skipped. A missing `Properties`
    /// reads as an empty map and a missing `Type` as `None`, which the packet checks then reject.
    pub fn from_value(v: &Value) -> Result<Self, HubCodecError> {
        if !matches!(v, Value::Map(_)) {
            return Err(malformed("multiplex packet is not a map"));
        }

        let properties = match map_get(v, "Properties") {
            None | Some(Value::Nil) => Vec::new(),
            Some(Value::Map(entries)) => entries
                .iter()
                .map(|(k, v)| {
                    k.as_str()
                        .map(|k| (k.to_string(), v.clone()))
                        .ok_or_else(|| malformed("property key is not a string"))
                })
                .collect::<Result<_, _>>()?,
            Some(_) => return Err(malformed("Properties is not a map")),
        };
        let handler_type = match map_get(v, "Type") {
            Some(x) => read_opt_string(x, "Type")?,
            None => None,
        };
        let raw = match map_get(v, "Raw") {
            None | Some(Value::Nil) => None,
            Some(Value::Binary(b)) => Some(b.clone()),
            Some(_) => return Err(malformed("Raw is not bin")),
        };

        Ok(Self { properties, handler_type, raw })
    }
}

/// The `OnConnected` payload (a map of 2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OnConnectedPayload {
    pub region_name: Option<String>,
    /// Informational only: these ids are not added to the partner set.
    pub partners: Option<Vec<String>>,
}

impl OnConnectedPayload {
    /// `{ RegionName, Partners }` in that order, each nil when absent.
    pub fn to_value(&self) -> Value {
        Value::Map(vec![
            (Value::from("RegionName"), opt_str(&self.region_name)),
            (
                Value::from("Partners"),
                match &self.partners {
                    Some(p) => str_array(p),
                    None => Value::Nil,
                },
            ),
        ])
    }

    /// Reads the payload by key name; a nil payload or a missing key reads as `None`.
    pub fn from_value(v: &Value) -> Result<Self, HubCodecError> {
        if v.is_nil() {
            return Ok(Self::default());
        }
        if !matches!(v, Value::Map(_)) {
            return Err(malformed("OnConnected payload is not a map"));
        }

        let region_name = match map_get(v, "RegionName") {
            Some(x) => read_opt_string(x, "RegionName")?,
            None => None,
        };
        let partners = match map_get(v, "Partners") {
            None | Some(Value::Nil) => None,
            Some(x) => Some(read_string_array(x, "Partners")?),
        };

        Ok(Self { region_name, partners })
    }
}

/// Hub method names, outbound and inbound.
pub mod method {
    /// Outbound: sends a multiplex packet to a partner outside a connection session.
    pub const SEND_MESSAGE: &str = "SendMessageAsync";
    /// Outbound: sends a multiplex packet to a partner within a connection session.
    pub const SEND_SESSION_BASED_MESSAGE: &str = "SendSessionBasedMessageAsync";
    /// Outbound: tells a partner that this client has connected to it.
    pub const SEND_CONNECTED: &str = "SendConnectedAsync";
    /// Inbound: the hub confirms the connection and reports its region.
    pub const ON_CONNECTED: &str = "OnConnected";
    /// Inbound: a multiplex packet from a partner, outside a connection session.
    pub const ON_RECEIVE_MESSAGE: &str = "OnReceiveMessage";
    /// Inbound: a multiplex packet from a partner, within a connection session.
    pub const ON_RECEIVE_SESSION_BASED_MESSAGE: &str = "OnReceiveSessionBasedMessage";
    /// Inbound: a partner has connected to the hub.
    pub const ON_PARTNER_CONNECTED: &str = "OnPartnerConnected";
    /// Inbound: a partner has disconnected from the hub.
    pub const ON_PARTNER_DISCONNECTED: &str = "OnPartnerDisconnected";
    /// Inbound: a partner's periodic heartbeat.
    pub const ON_PARTNER_HEARTBEAT: &str = "OnPartnerHeartBeat";
}

/// The `TraceState` of `SendConnectedAsync`.
pub fn send_connected_trace_state() -> Vec<(String, String)> {
    vec![("ypScenarioId".into(), "SendConnectedMessage".into()), ("ypTriggerId".into(), "ConnectingWithPartner".into())]
}

/// Arguments of `SendConnectedAsync`: `[ ctx, targetId, {} ]` — the third is the empty map `0x80`.
pub fn send_connected_arguments(ctx: &TraceContext, target_id: &str) -> Vec<Value> {
    vec![TraceContextPacket::from_context(ctx).to_value(), Value::from(target_id), Value::Map(Vec::new())]
}

/// Arguments of `SendMessageAsync` (`[ctx, targetId, data]`) or, with a session id,
/// `SendSessionBasedMessageAsync` (`[ctx, targetId, data, connectionSessionId]`).
pub fn send_message_arguments(
    ctx: &TraceContext,
    target_id: &str,
    packet: &MultiplexPacket,
    connection_session_id: Option<&str>,
) -> (&'static str, Vec<Value>) {
    let mut args = vec![TraceContextPacket::from_context(ctx).to_value(), Value::from(target_id), packet.to_value()];

    match connection_session_id {
        Some(id) => {
            args.push(Value::from(id));
            (method::SEND_SESSION_BASED_MESSAGE, args)
        }
        None => (method::SEND_MESSAGE, args),
    }
}

/// A JSON view of a `MessagePack` value for the tap (bin as `{"$bin": "<hex>"}`).
pub fn value_to_json(v: &Value) -> serde_json::Value {
    use serde_json::json;
    match v {
        Value::Nil => serde_json::Value::Null,
        Value::Boolean(b) => json!(b),
        Value::Integer(i) => {
            if let Some(n) = i.as_i64() {
                json!(n)
            } else if let Some(n) = i.as_u64() {
                json!(n)
            } else {
                json!(i.to_string())
            }
        }
        Value::F32(f) => json!({ "$f32": f }),
        Value::F64(f) => json!({ "$f64": f }),
        Value::String(s) => match s.as_str() {
            Some(s) => json!(s),
            None => json!({ "$invalid_utf8": hex::encode(s.as_bytes()) }),
        },
        Value::Binary(b) => json!({ "$bin": hex::encode(b) }),
        Value::Array(items) => serde_json::Value::Array(items.iter().map(value_to_json).collect()),
        Value::Map(entries) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in entries {
                let key = match k.as_str() {
                    Some(s) => s.to_string(),
                    None => value_to_json(k).to_string(),
                };
                obj.insert(key, value_to_json(v));
            }
            serde_json::Value::Object(obj)
        }
        Value::Ext(t, data) => json!({ "$ext": t, "data": hex::encode(data) }),
    }
}

#[cfg(test)]
mod tests;
