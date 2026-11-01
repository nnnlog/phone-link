//! `ms-dcg` packets.
//!
//! It covers the property maps of fragments (`Type = 2`) and acknowledgements (`Type = 1`) carried
//! in a [`MultiplexPacket`], their enumerations, and the packet checks a receiver runs before
//! anything else.
//!
//! Writers emit the property order Phone Link on Windows sends and the smallest `MessagePack` integer
//! encodings; readers accept lenient forms of each property.

use rmpv::Value;

use crate::codec::hub::MultiplexPacket;
use crate::types::MessageType;

/// The only handler type (the multiplex packet's `Type`).
pub const HANDLER_TYPE: &str = "ms-dcg";
/// `Version` written on every packet.
pub const VERSION: f64 = 1.0;

/// The DCG message type, the value of `Properties["Type"]`.
pub mod dcg_message_type {
    /// `0`: a DCG message type the receiver does not recognize.
    pub const UNKNOWN: i32 = 0;
    /// `1`: an acknowledgement of a received fragment.
    pub const ACKNOWLEDGEMENT: i32 = 1;
    /// `2`: a fragment carrying part of an App or Platform message.
    pub const FRAGMENT: i32 = 2;
    /// `3`: a presence announcement; the receiver logs and ignores it.
    pub const PRESENCE_ANNOUNCEMENT: i32 = 3;
    /// `4`: a presence request; the receiver logs and ignores it.
    pub const PRESENCE_REQUEST: i32 = 4;
    /// `5`: a presence response; the receiver logs and ignores it.
    pub const PRESENCE_RESPONSE: i32 = 5;
}

/// One fragment (`Type = 2`).
#[derive(Debug, Clone, PartialEq)]
pub struct Fragment {
    pub version: f64,
    pub session_id: String,
    pub sequence_number: i32,
    pub message_id: i32,
    /// The raw `MessageType` property, `None` when absent; read it through [`Fragment::message_type`].
    pub message_type_raw: Option<i32>,
    /// 1-based index within the message.
    pub fragment_id: i32,
    pub fragment_count: i32,
    /// The packet's `Raw`. [`parse_packet`] reads a nil `Raw` as empty: the receiver tells the two
    /// apart through the packet ([`MultiplexPacket::raw`]).
    pub raw: Vec<u8>,
}

impl Fragment {
    /// Receive rule: `MessageType` present and `== 1` ⇒ Platform; absent or any other value ⇒ App.
    pub fn message_type(&self) -> MessageType {
        MessageType::from_wire(self.message_type_raw.map(i64::from))
    }

    /// `{ Version, Type, SessionId, SequenceNumber, MessageId, MessageType, FragmentId, FragmentCount }`
    /// in that order, `Type: "ms-dcg"`, `Raw` = the fragment bytes.
    pub fn to_packet(&self) -> MultiplexPacket {
        let mut properties = vec![
            ("Version".to_string(), Value::F64(self.version)),
            ("Type".to_string(), Value::from(dcg_message_type::FRAGMENT)),
            ("SessionId".to_string(), Value::from(self.session_id.as_str())),
            ("SequenceNumber".to_string(), Value::from(self.sequence_number)),
            ("MessageId".to_string(), Value::from(self.message_id)),
        ];

        if let Some(mt) = self.message_type_raw {
            properties.push(("MessageType".to_string(), Value::from(mt)));
        }
        properties.push(("FragmentId".to_string(), Value::from(self.fragment_id)));
        properties.push(("FragmentCount".to_string(), Value::from(self.fragment_count)));
        MultiplexPacket { properties, handler_type: Some(HANDLER_TYPE.to_string()), raw: Some(self.raw.clone()) }
    }
}

/// One acknowledgement (`Type = 1`); `Raw` is nil.
#[derive(Debug, Clone, PartialEq)]
pub struct Ack {
    pub version: f64,
    pub session_id: String,
    pub sequence_number: i32,
    pub success: bool,
    pub error_no: i32,
    /// Read only for logs: a non-string value is kept in its string form.
    pub error_message: Option<String>,
}

/// The `ErrorMessage` of the ack [`parse_packet`] synthesizes for an ack with `Version > 1.0`.
pub const TOO_NEW_ERROR_MESSAGE: &str = "RemoteTransportTooNew";

/// What a received ack means for the fragment waiting on its `SequenceNumber` in the stop-and-wait
/// sender.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckOutcome {
    Acknowledged,
    /// `Success: false`, `ErrorNo 1`.
    RemoteTransportTooOld,
    /// Synthesized for an ack with `Version > 1.0`, whatever else it carries.
    RemoteTransportTooNew,
    /// `Success: false` with any other `ErrorNo`.
    RemoteTransportError {
        error_no: i32,
    },
}

/// The result an `ErrorNo` is read as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DcgMessageResult {
    Success,
    RemoteTransportTooOld,
    InvalidHandler,
    UnspecifiedError,
}

/// Maps an acknowledgement's `ErrorNo` to its result; values above `2` are unspecified errors.
pub fn error_no_result(error_no: i32) -> DcgMessageResult {
    match error_no {
        0 => DcgMessageResult::Success,
        1 => DcgMessageResult::RemoteTransportTooOld,
        2 => DcgMessageResult::InvalidHandler,
        _ => DcgMessageResult::UnspecifiedError,
    }
}

impl Ack {
    /// The only ack sent, as Phone Link on Windows sends it: `Success: true, ErrorNo: 0`, no
    /// `ErrorMessage`.
    pub fn success(session_id: impl Into<String>, sequence_number: i32) -> Self {
        Self {
            version: VERSION,
            session_id: session_id.into(),
            sequence_number,
            success: true,
            error_no: 0,
            error_message: None,
        }
    }

    /// What this acknowledgement means for the sent fragment: accepted, or why it was not.
    pub fn outcome(&self) -> AckOutcome {
        if self.version > VERSION {
            AckOutcome::RemoteTransportTooNew
        } else if self.success {
            AckOutcome::Acknowledged
        } else if self.error_no == 1 {
            AckOutcome::RemoteTransportTooOld
        } else {
            AckOutcome::RemoteTransportError { error_no: self.error_no }
        }
    }

    /// `{ Version, Type, SessionId, SequenceNumber, Success, ErrorNo [, ErrorMessage] }`, `Raw: nil`;
    /// `ErrorMessage` only when non-blank.
    pub fn to_packet(&self) -> MultiplexPacket {
        let mut properties = vec![
            ("Version".to_string(), Value::F64(self.version)),
            ("Type".to_string(), Value::from(dcg_message_type::ACKNOWLEDGEMENT)),
            ("SessionId".to_string(), Value::from(self.session_id.as_str())),
            ("SequenceNumber".to_string(), Value::from(self.sequence_number)),
            ("Success".to_string(), Value::Boolean(self.success)),
            ("ErrorNo".to_string(), Value::from(self.error_no)),
        ];

        if let Some(msg) = self.error_message.as_deref().filter(|m| !m.trim().is_empty()) {
            properties.push(("ErrorMessage".to_string(), Value::from(msg)));
        }
        MultiplexPacket { properties, handler_type: Some(HANDLER_TYPE.to_string()), raw: None }
    }
}

/// A packet that passed the receiver's packet checks.
#[derive(Debug, Clone, PartialEq)]
pub enum ParsedPacket {
    /// A fragment; one whose `Raw` is nil is accepted with an empty `raw`.
    Fragment(Fragment),
    /// An ack. One with `Version > 1.0` is read as a failed ack: only its `SessionId` and
    /// `SequenceNumber` were read; `success` is `false`, `error_no` `0` (unread) and
    /// `error_message` [`TOO_NEW_ERROR_MESSAGE`] — its outcome is `RemoteTransportTooNew`.
    Ack(Ack),
    /// `Properties["Type"]` ∈ {3, 4, 5}: ignored silently.
    Presence(i32),
}

/// A packet dropped by the receiver's packet checks (every case: drop without reply, log only).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PacketError {
    /// The multiplex packet's `Type` is not `"ms-dcg"` (`InvalidHandler`).
    #[error("handler type {0:?} is not ms-dcg")]
    InvalidHandler(Option<String>),
    /// A fragment with `Version > 1.0` (`UnsupportedVersion`).
    #[error("fragment version {0} is newer than 1.0")]
    UnsupportedVersion(f64),
    /// A required property is missing or of the wrong `MessagePack` type, `Type` is 0, ≥ 6 or missing,
    /// or an ack's `ErrorMessage` is nil.
    #[error("unparseable packet: {0}")]
    Malformed(String),
}

fn malformed(what: impl Into<String>) -> PacketError {
    PacketError::Malformed(what.into())
}

/// An integer-valued property: any `MessagePack` integer code, or a float rounded half to even; out of
/// `i32` range is a parse failure.
fn int_property(p: &MultiplexPacket, key: &str) -> Result<Option<i32>, PacketError> {
    p.property(key).map(|v| read_int32(key, v)).transpose()
}

/// An integer property read from the `MessagePack` value: an integer in range; a float rounded half
/// to even, in range; a bool as `1` or `0`; nil as `0`; a string as a decimal integer with an
/// optional sign and surrounding white space. Anything else — a binary, array, map or extension
/// value, a string that is no integer, a value out of the `Int32` range — fails, which makes the
/// packet unparseable.
fn read_int32(key: &str, v: &Value) -> Result<i32, PacketError> {
    let out_of_range = || malformed(format!("{key} out of range"));
    let rounded = |f: f64| {
        let r = f.round_ties_even();
        if r.is_finite() && r >= f64::from(i32::MIN) && r <= f64::from(i32::MAX) {
            Ok(r as i32)
        } else {
            Err(out_of_range())
        }
    };

    match v {
        Value::Nil => Ok(0),
        Value::Boolean(b) => Ok(i32::from(*b)),
        Value::Integer(i) => i.as_i64().and_then(|n| i32::try_from(n).ok()).ok_or_else(out_of_range),
        Value::F64(f) => rounded(*f),
        Value::F32(f) => rounded(f64::from(*f)),
        Value::String(s) => {
            let text = s.as_str().ok_or_else(|| malformed(format!("{key} is not UTF-8")))?;
            parse_text_int32(text).ok_or_else(|| malformed(format!("{key} is not an integer: {text:?}")))
        }
        _ => Err(malformed(format!("{key} is not convertible to an integer"))),
    }
}

/// An `Int32`: white space (U+0009–U+000D, U+0020) around an optional `+`/`-` sign and at least one
/// ASCII digit; `None` for anything else or a value out of range.
fn parse_text_int32(text: &str) -> Option<i32> {
    let t = text.trim_matches(|c: char| matches!(c, '\u{9}'..='\u{d}' | ' '));
    let (negative, digits) = match t.as_bytes().first()? {
        b'-' => (true, &t[1..]),
        b'+' => (false, &t[1..]),
        _ => (false, t),
    };

    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }

    let mut n: i64 = 0;
    for b in digits.bytes() {
        n = n.checked_mul(10)?.checked_add(i64::from(b - b'0'))?;
        if n > i64::from(i32::MAX) + 1 {
            return None;
        }
    }

    i32::try_from(if negative { -n } else { n }).ok()
}

/// `MessageType` is optional: absent ⇒ App; present, it is read as the integer properties are
/// (`read_int32`) and `1` ⇒ Platform, any other number ⇒ App; a value that does not convert
/// makes the packet unparseable.
fn message_type_property(p: &MultiplexPacket) -> Result<Option<i32>, PacketError> {
    int_property(p, "MessageType")
}

fn required_int(p: &MultiplexPacket, key: &str) -> Result<i32, PacketError> {
    int_property(p, key)?.ok_or_else(|| malformed(format!("{key} missing")))
}

fn required_str(p: &MultiplexPacket, key: &str) -> Result<String, PacketError> {
    match p.property(key) {
        Some(Value::String(s)) => {
            s.as_str().map(str::to_string).ok_or_else(|| malformed(format!("{key} is not UTF-8")))
        }
        Some(_) => Err(malformed(format!("{key} is not a string"))),
        None => Err(malformed(format!("{key} missing"))),
    }
}

/// `ErrorMessage` of an ack: absent ⇒ none; nil ⇒ a parse failure; a string ⇒ its text (not UTF-8 ⇒
/// a parse failure, as for every string property); any other value ⇒ its string form. The value is
/// only logged.
fn error_message_property(p: &MultiplexPacket) -> Result<Option<String>, PacketError> {
    match p.property("ErrorMessage") {
        None => Ok(None),
        Some(Value::Nil) => Err(malformed("ErrorMessage is nil")),
        Some(Value::String(s)) => {
            s.as_str().map(|s| Some(s.to_string())).ok_or_else(|| malformed("ErrorMessage is not UTF-8"))
        }
        Some(other) => Ok(Some(other.to_string())),
    }
}

/// Applies the receiver's packet checks to one inbound packet. The trust check that follows is the
/// caller's.
pub fn parse_packet(p: &MultiplexPacket) -> Result<ParsedPacket, PacketError> {
    if p.handler_type.as_deref() != Some(HANDLER_TYPE) {
        return Err(PacketError::InvalidHandler(p.handler_type.clone()));
    }

    let version = match p.property("Version") {
        Some(Value::F64(v)) => *v,
        Some(_) => return Err(malformed("Version is not a float64")),
        None => return Err(malformed("Version missing")),
    };

    let packet_type = required_int(p, "Type")?;
    match packet_type {
        dcg_message_type::ACKNOWLEDGEMENT if version > VERSION => {
            // A failed ack from a peer too new: only these two properties are read.
            Ok(ParsedPacket::Ack(Ack {
                version,
                session_id: required_str(p, "SessionId")?,
                sequence_number: required_int(p, "SequenceNumber")?,
                success: false,
                error_no: 0,
                error_message: Some(TOO_NEW_ERROR_MESSAGE.to_string()),
            }))
        }
        dcg_message_type::ACKNOWLEDGEMENT => {
            let success = match p.property("Success") {
                Some(Value::Boolean(b)) => *b,
                Some(_) => return Err(malformed("Success is not a bool")),
                None => return Err(malformed("Success missing")),
            };
            Ok(ParsedPacket::Ack(Ack {
                version,
                session_id: required_str(p, "SessionId")?,
                sequence_number: required_int(p, "SequenceNumber")?,
                success,
                error_no: required_int(p, "ErrorNo")?,
                error_message: error_message_property(p)?,
            }))
        }
        dcg_message_type::FRAGMENT => {
            if version > VERSION {
                return Err(PacketError::UnsupportedVersion(version));
            }

            // `Raw` is taken as it is, nil included; the message such a fragment belongs to
            // fails when it is reassembled (the receiver's rule).
            let raw = p.raw.clone().unwrap_or_default();
            Ok(ParsedPacket::Fragment(Fragment {
                version,
                session_id: required_str(p, "SessionId")?,
                sequence_number: required_int(p, "SequenceNumber")?,
                message_id: required_int(p, "MessageId")?,
                message_type_raw: message_type_property(p)?,
                fragment_id: required_int(p, "FragmentId")?,
                fragment_count: required_int(p, "FragmentCount")?,
                raw,
            }))
        }
        dcg_message_type::PRESENCE_ANNOUNCEMENT
        | dcg_message_type::PRESENCE_REQUEST
        | dcg_message_type::PRESENCE_RESPONSE => Ok(ParsedPacket::Presence(packet_type)),
        other => Err(malformed(format!("packet Type {other}"))),
    }
}

/// Fragments `envelope` into chunks of `fragment_size` bytes: fragment *k* (1-based) carries bytes
/// `[(k−1)·size, min(k·size, total))`; count = `ceil(total / size)`.
///
/// An empty envelope cannot occur (every envelope has a header) and fails.
pub fn split_into_fragments(envelope: &[u8], fragment_size: usize) -> Vec<&[u8]> {
    assert!(fragment_size > 0, "fragment size must be positive");
    assert!(!envelope.is_empty(), "an envelope is never empty");
    envelope.chunks(fragment_size).collect()
}

#[cfg(test)]
mod tests;
