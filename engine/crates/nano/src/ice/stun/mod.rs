//! STUN messages (RFC 5389) with the ICE (RFC 8445), transmit-counter (RFC 7982) and TURN (RFC 8656)
//! attributes the Nano path uses.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;

pub const MAGIC_COOKIE: u32 = 0x2112_A442;
pub const HEADER_LEN: usize = 20;
const FINGERPRINT_XOR: u32 = 0x5354_554E;
/// `SOFTWARE` on every request the PC sends.
pub const SOFTWARE: &str = "libnano";

pub mod method {
    pub const BINDING: u16 = 0x001;
    pub const ALLOCATE: u16 = 0x003;
    pub const REFRESH: u16 = 0x004;
    pub const SEND: u16 = 0x006;
    pub const DATA: u16 = 0x007;
    pub const CREATE_PERMISSION: u16 = 0x008;
    pub const CHANNEL_BIND: u16 = 0x009;
}

pub mod attr {
    pub const MAPPED_ADDRESS: u16 = 0x0001;
    pub const USERNAME: u16 = 0x0006;
    pub const MESSAGE_INTEGRITY: u16 = 0x0008;
    pub const ERROR_CODE: u16 = 0x0009;
    pub const CHANNEL_NUMBER: u16 = 0x000C;
    pub const LIFETIME: u16 = 0x000D;
    pub const XOR_PEER_ADDRESS: u16 = 0x0012;
    pub const DATA: u16 = 0x0013;
    pub const REALM: u16 = 0x0014;
    pub const NONCE: u16 = 0x0015;
    pub const XOR_RELAYED_ADDRESS: u16 = 0x0016;
    pub const REQUESTED_ADDRESS_FAMILY: u16 = 0x0017;
    pub const REQUESTED_TRANSPORT: u16 = 0x0019;
    pub const DONT_FRAGMENT: u16 = 0x001A;
    pub const XOR_MAPPED_ADDRESS: u16 = 0x0020;
    pub const PRIORITY: u16 = 0x0024;
    pub const USE_CANDIDATE: u16 = 0x0025;
    pub const SOFTWARE: u16 = 0x8022;
    pub const ALTERNATE_SERVER: u16 = 0x8023;
    pub const TRANSACTION_TRANSMIT_COUNTER: u16 = 0x8025;
    pub const FINGERPRINT: u16 = 0x8028;
    pub const ICE_CONTROLLED: u16 = 0x8029;
    pub const ICE_CONTROLLING: u16 = 0x802A;
    /// Microsoft's TURN client-context attribute.
    pub const MS_CLIENT_CONTEXT: u16 = 0x8094;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Request,
    Indication,
    Success,
    Error,
}

impl Class {
    fn bits(self) -> u16 {
        match self {
            Class::Request => 0x000,
            Class::Indication => 0x010,
            Class::Success => 0x100,
            Class::Error => 0x110,
        }
    }

    fn from_type(t: u16) -> Self {
        match t & 0x110 {
            0x000 => Class::Request,
            0x010 => Class::Indication,
            0x100 => Class::Success,
            _ => Class::Error,
        }
    }
}

fn message_type(class: Class, method: u16) -> u16 {
    (method & 0x000F) | ((method & 0x0070) << 1) | ((method & 0x0F80) << 2) | class.bits()
}

fn method_of(t: u16) -> u16 {
    (t & 0x000F) | ((t >> 1) & 0x0070) | ((t >> 2) & 0x0F80)
}

pub type TransactionId = [u8; 12];

/// A failure decoding a STUN message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StunError {
    #[error("not a STUN message")]
    NotStun,
    #[error("truncated STUN message")]
    Truncated,
    #[error("MESSAGE-INTEGRITY does not verify")]
    Integrity,
    #[error("FINGERPRINT does not verify")]
    Fingerprint,
}

/// Whether `data` is a STUN message on a Nano path: at least 20 bytes, first byte below `0x40`, a
/// length that is a multiple of 4, and the magic cookie.
pub fn is_stun(data: &[u8]) -> bool {
    data.len() >= HEADER_LEN
        && data[0] < 0x40
        && u16::from_be_bytes([data[2], data[3]]) % 4 == 0
        && u32::from_be_bytes([data[4], data[5], data[6], data[7]]) == MAGIC_COOKIE
}

/// A decoded or to-be-encoded STUN message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub class: Class,
    pub method: u16,
    pub transaction: TransactionId,
    /// Attributes in order, without MESSAGE-INTEGRITY and FINGERPRINT (which [`Message::encode`] adds).
    pub attributes: Vec<(u16, Vec<u8>)>,
}

/// What [`Message::decode`] found besides the attributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    pub message: Message,
    /// Offset of MESSAGE-INTEGRITY's header, if present.
    integrity_at: Option<usize>,
    raw: Vec<u8>,
}

impl Decoded {
    #[cfg(feature = "test-support")]
    pub fn has_integrity(&self) -> bool {
        self.integrity_at.is_some()
    }

    /// Verifies MESSAGE-INTEGRITY with `key`.
    pub fn verify_integrity(&self, key: &[u8]) -> Result<(), StunError> {
        let at = self.integrity_at.ok_or(StunError::Integrity)?;
        let mut covered = self.raw[..at].to_vec();
        let length = (at + 24 - HEADER_LEN) as u16;
        covered[2..4].copy_from_slice(&length.to_be_bytes());

        let expected = hmac_sha1(key, &covered);
        if self.raw[at + 4..at + 24] == expected { Ok(()) } else { Err(StunError::Integrity) }
    }
}

fn hmac_sha1(key: &[u8], data: &[u8]) -> [u8; 20] {
    let mut mac = <Hmac<Sha1> as KeyInit>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

impl Message {
    pub fn new(class: Class, method: u16, transaction: TransactionId) -> Self {
        Self { class, method, transaction, attributes: Vec::new() }
    }

    pub fn request(method: u16) -> Self {
        Self::new(Class::Request, method, rand::random())
    }

    pub fn add(&mut self, kind: u16, value: impl Into<Vec<u8>>) -> &mut Self {
        self.attributes.push((kind, value.into()));
        self
    }

    pub fn with(mut self, kind: u16, value: impl Into<Vec<u8>>) -> Self {
        self.add(kind, value);
        self
    }

    pub fn get(&self, kind: u16) -> Option<&[u8]> {
        self.attributes.iter().find(|(k, _)| *k == kind).map(|(_, v)| v.as_slice())
    }

    #[cfg(feature = "test-support")]
    pub fn has(&self, kind: u16) -> bool {
        self.get(kind).is_some()
    }

    /// Encodes the message, appending MESSAGE-INTEGRITY keyed with `integrity_key` when given, and
    /// FINGERPRINT when `fingerprint` is set.
    pub fn encode(&self, integrity_key: Option<&[u8]>, fingerprint: bool) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + 64);
        out.extend_from_slice(&message_type(self.class, self.method).to_be_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        out.extend_from_slice(&self.transaction);

        for (kind, value) in &self.attributes {
            push_attribute(&mut out, *kind, value);
        }

        if let Some(key) = integrity_key {
            let length = (out.len() + 24 - HEADER_LEN) as u16;
            out[2..4].copy_from_slice(&length.to_be_bytes());
            let mac = hmac_sha1(key, &out);
            push_attribute(&mut out, attr::MESSAGE_INTEGRITY, &mac);
        }

        if fingerprint {
            let length = (out.len() + 8 - HEADER_LEN) as u16;
            out[2..4].copy_from_slice(&length.to_be_bytes());
            let crc = crc32fast::hash(&out) ^ FINGERPRINT_XOR;
            push_attribute(&mut out, attr::FINGERPRINT, &crc.to_be_bytes());
        }

        let length = (out.len() - HEADER_LEN) as u16;
        out[2..4].copy_from_slice(&length.to_be_bytes());
        out
    }

    /// Decodes a STUN message and verifies a FINGERPRINT when present. Attributes after
    /// MESSAGE-INTEGRITY (other than FINGERPRINT) are ignored, as RFC 5389 requires.
    pub fn decode(data: &[u8]) -> Result<Decoded, StunError> {
        if !is_stun(data) {
            return Err(StunError::NotStun);
        }

        let t = u16::from_be_bytes([data[0], data[1]]);
        let length = usize::from(u16::from_be_bytes([data[2], data[3]]));
        if data.len() < HEADER_LEN + length {
            return Err(StunError::Truncated);
        }

        let raw = data[..HEADER_LEN + length].to_vec();
        let mut message = Message::new(Class::from_type(t), method_of(t), raw[8..20].try_into().expect("12 bytes"));
        let mut integrity_at = None;
        let mut at = HEADER_LEN;

        while at + 4 <= raw.len() {
            let kind = u16::from_be_bytes([raw[at], raw[at + 1]]);
            let len = usize::from(u16::from_be_bytes([raw[at + 2], raw[at + 3]]));
            let value_end = at + 4 + len;
            if value_end > raw.len() {
                return Err(StunError::Truncated);
            }

            let value = &raw[at + 4..value_end];
            match kind {
                attr::FINGERPRINT => {
                    if len != 4 {
                        return Err(StunError::Fingerprint);
                    }

                    let mut covered = raw[..at].to_vec();
                    covered[2..4].copy_from_slice(&((at + 8 - HEADER_LEN) as u16).to_be_bytes());
                    let crc = crc32fast::hash(&covered) ^ FINGERPRINT_XOR;
                    if value != crc.to_be_bytes() {
                        return Err(StunError::Fingerprint);
                    }
                    break;
                }
                attr::MESSAGE_INTEGRITY => {
                    if len != 20 {
                        return Err(StunError::Integrity);
                    }
                    integrity_at = Some(at);
                }
                _ if integrity_at.is_none() => message.attributes.push((kind, value.to_vec())),
                _ => {}
            }

            at = value_end + (4 - len % 4) % 4;
        }

        Ok(Decoded { message, integrity_at, raw })
    }

    // Typed attribute values.

    pub fn mapped_address(&self) -> Option<SocketAddr> {
        self.get(attr::XOR_MAPPED_ADDRESS)
            .and_then(|v| decode_xor_address(v, &self.transaction))
            .or_else(|| self.get(attr::MAPPED_ADDRESS).and_then(decode_address))
    }

    pub fn xor_address(&self, kind: u16) -> Option<SocketAddr> {
        self.get(kind).and_then(|v| decode_xor_address(v, &self.transaction))
    }

    pub fn error_code(&self) -> Option<u16> {
        let v = self.get(attr::ERROR_CODE)?;
        (v.len() >= 4).then(|| u16::from(v[2] & 0x7) * 100 + u16::from(v[3]))
    }

    pub fn u32_attribute(&self, kind: u16) -> Option<u32> {
        let v = self.get(kind)?;
        Some(u32::from_be_bytes(v.get(..4)?.try_into().ok()?))
    }

    pub fn text(&self, kind: u16) -> Option<String> {
        self.get(kind).map(|v| String::from_utf8_lossy(v).into_owned())
    }
}

fn push_attribute(out: &mut Vec<u8>, kind: u16, value: &[u8]) {
    out.extend_from_slice(&kind.to_be_bytes());
    out.extend_from_slice(&(value.len() as u16).to_be_bytes());
    out.extend_from_slice(value);
    out.resize(out.len() + (4 - value.len() % 4) % 4, 0);
}

/// MAPPED-ADDRESS / ALTERNATE-SERVER value.
pub fn encode_address(address: SocketAddr) -> Vec<u8> {
    let mut v = vec![0];
    match address.ip() {
        IpAddr::V4(ip) => {
            v.push(1);
            v.extend_from_slice(&address.port().to_be_bytes());
            v.extend_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            v.push(2);
            v.extend_from_slice(&address.port().to_be_bytes());
            v.extend_from_slice(&ip.octets());
        }
    }
    v
}

pub fn decode_address(v: &[u8]) -> Option<SocketAddr> {
    let port = u16::from_be_bytes(v.get(2..4)?.try_into().ok()?);
    match v.get(1)? {
        1 => Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(v.get(4..8)?).ok()?)), port)),
        2 => Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(v.get(4..20)?).ok()?)), port)),
        _ => None,
    }
}

fn xor_mask(transaction: &TransactionId) -> [u8; 16] {
    let mut mask = [0u8; 16];
    mask[..4].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
    mask[4..].copy_from_slice(transaction);
    mask
}

/// XORs the port with the cookie's high 16 bits and the address with cookie ‖ transaction id.
fn xor_in_place(v: &mut [u8], transaction: &TransactionId) {
    let mask = xor_mask(transaction);
    for (i, b) in v.iter_mut().enumerate().skip(2) {
        *b ^= if i < 4 { mask[i - 2] } else { mask[i - 4] };
    }
}

/// XOR-MAPPED-ADDRESS / XOR-PEER-ADDRESS / XOR-RELAYED-ADDRESS value.
pub fn encode_xor_address(address: SocketAddr, transaction: &TransactionId) -> Vec<u8> {
    let mut v = encode_address(address);
    xor_in_place(&mut v, transaction);
    v
}

pub fn decode_xor_address(v: &[u8], transaction: &TransactionId) -> Option<SocketAddr> {
    let mut plain = v.get(..20.min(v.len()))?.to_vec();
    xor_in_place(&mut plain, transaction);
    decode_address(&plain)
}

#[cfg(feature = "test-support")]
/// ERROR-CODE value.
pub fn encode_error(code: u16, reason: &str) -> Vec<u8> {
    let mut v = vec![0, 0, (code / 100) as u8, (code % 100) as u8];
    v.extend_from_slice(reason.as_bytes());
    v
}

/// `TRANSACTION_TRANSMIT_COUNTER` value: reserved, request count, response count.
pub fn transmit_counter(request: u8, response: u8) -> Vec<u8> {
    vec![0, 0, request, response]
}

/// The long-term credential key: `MD5(username:realm:password)`.
pub fn long_term_key(username: &str, realm: &str, password: &str) -> [u8; 16] {
    use md5::{Digest, Md5};
    Md5::digest(format!("{username}:{realm}:{password}").as_bytes()).into()
}

/// A TURN `ChannelData` message: channel number, length, data, padded to 4 bytes on UDP.
pub fn channel_data(channel: u16, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + data.len() + 3);
    out.extend_from_slice(&channel.to_be_bytes());
    out.extend_from_slice(&(data.len() as u16).to_be_bytes());
    out.extend_from_slice(data);
    out.resize(out.len() + (4 - data.len() % 4) % 4, 0);
    out
}

/// Splits a `ChannelData` message into its channel and data.
pub fn parse_channel_data(datagram: &[u8]) -> Option<(u16, &[u8])> {
    if datagram.len() < 4 || !(0x40..0x80).contains(&datagram[0]) {
        return None;
    }

    let channel = u16::from_be_bytes([datagram[0], datagram[1]]);
    let len = usize::from(u16::from_be_bytes([datagram[2], datagram[3]]));
    datagram.get(4..4 + len).map(|d| (channel, d))
}

#[cfg(test)]
mod tests;
