//! ICE candidates and the session description string carried in `ice.candidates`.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};

use serde_json::{Map, Value};

use crate::ptree::{self, PtreeError, leaf};

/// The session description version every agent writes and requires.
pub const DESCRIPTION_VERSION: &str = "1";
/// Check pacing, initial STUN RTO and STUN retry count the PC writes into its description.
pub const PACING_MS: u32 = 50;
pub const STUN_TIMEOUT_MS: u32 = 200;
pub const STUN_RETRY_COUNT: u32 = 4;
/// The transport string of every UDP candidate; agents pair only candidates whose strings are equal.
pub const TRANSPORT_UDP: &str = "udp";

/// `type` of a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CandidateType {
    Host = 0,
    ServerReflexive = 1,
    PeerReflexive = 2,
    Relayed = 3,
}

impl CandidateType {
    pub fn from_wire(value: u64) -> Option<Self> {
        Some(match value {
            0 => Self::Host,
            1 => Self::ServerReflexive,
            2 => Self::PeerReflexive,
            3 => Self::Relayed,
            _ => return None,
        })
    }

    /// The type preference of the candidate priority.
    pub fn type_preference(self) -> u32 {
        match self {
            Self::Host => 126,
            Self::ServerReflexive => 100,
            Self::PeerReflexive => 110,
            Self::Relayed => 0,
        }
    }
}

/// `addressType`: the classification of an address both agents use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddressType {
    Unicast = 0,
    Multicast = 1,
    Loopback = 2,
    Private = 3,
    LinkLocal = 4,
    SiteLocal = 5,
    Ipv4Mapped = 6,
    Ipv4Compatible = 7,
    Unspecified = 8,
    Invalid = 9,
}

impl AddressType {
    pub fn from_wire(value: u64) -> Option<Self> {
        Some(match value {
            0 => Self::Unicast,
            1 => Self::Multicast,
            2 => Self::Loopback,
            3 => Self::Private,
            4 => Self::LinkLocal,
            5 => Self::SiteLocal,
            6 => Self::Ipv4Mapped,
            7 => Self::Ipv4Compatible,
            8 => Self::Unspecified,
            9 => Self::Invalid,
            _ => return None,
        })
    }
}

/// Classifies `ip` as the agents do.
pub fn classify(ip: IpAddr) -> AddressType {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => classify_v6(v6),
    }
}

fn classify_v4(ip: Ipv4Addr) -> AddressType {
    let o = ip.octets();
    if ip == Ipv4Addr::BROADCAST {
        AddressType::Invalid
    } else if o[0] >= 224 {
        AddressType::Multicast
    } else if o[0] == 127 {
        AddressType::Loopback
    } else if o[0] == 10 || (o[0] == 172 && (o[1] & 0xF0) == 16) || (o[0] == 192 && o[1] == 168) {
        AddressType::Private
    } else if o[0] == 169 && o[1] == 254 {
        AddressType::LinkLocal
    } else if o[0] == 0 {
        AddressType::Unspecified
    } else {
        AddressType::Unicast
    }
}

fn classify_v6(ip: Ipv6Addr) -> AddressType {
    let s = ip.segments();
    if ip.is_unspecified() {
        AddressType::Unspecified
    } else if ip.is_loopback() {
        AddressType::Loopback
    } else if s[0] & 0xFF00 == 0xFF00 {
        AddressType::Multicast
    } else if s[0] & 0xFE00 == 0xFC00 {
        AddressType::Private
    } else if s[0] & 0xFFC0 == 0xFE80 {
        AddressType::LinkLocal
    } else if s[0] & 0xFFC0 == 0xFEC0 {
        AddressType::SiteLocal
    } else if s[..5] == [0; 5] && s[5] == 0xFFFF {
        AddressType::Ipv4Mapped
    } else if s[..6] == [0; 6] {
        AddressType::Ipv4Compatible
    } else if s[0] == 0x0100 && s[1..4] == [0; 3] {
        AddressType::Invalid
    } else {
        AddressType::Unicast
    }
}

/// `typePref << 24 | localPref << 8 | 255`.
pub fn priority(kind: CandidateType, local_preference: u16) -> u32 {
    kind.type_preference() << 24 | u32::from(local_preference) << 8 | 255
}

/// The local preference of a candidate on the interface with `interface_index`: `65535 − index`.
pub fn local_preference(interface_index: u32) -> u16 {
    65_535u32.saturating_sub(interface_index) as u16
}

/// `a.b.c.d:port` or `[v6]:port` (no zone).
pub fn format_address(address: SocketAddr) -> String {
    match address {
        SocketAddr::V4(a) => format!("{}:{}", a.ip(), a.port()),
        SocketAddr::V6(a) => format!("[{}]:{}", a.ip(), a.port()),
    }
}

/// Parses an address written by [`format_address`] (a zone suffix in a v6 address is accepted).
pub fn parse_address(text: &str) -> Option<SocketAddr> {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix('[') {
        let (host, port) = rest.split_once("]:")?;
        let (host, zone) = match host.split_once('%') {
            Some((h, z)) => (h, z.parse::<u32>().ok()),
            None => (host, None),
        };
        let ip: Ipv6Addr = host.parse().ok()?;
        return Some(SocketAddr::V6(SocketAddrV6::new(ip, port.parse().ok()?, 0, zone.unwrap_or(0))));
    }

    let (host, port) = text.rsplit_once(':')?;
    Some(SocketAddr::new(IpAddr::V4(host.parse().ok()?), port.parse().ok()?))
}

/// One candidate as written in a session description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The address to send checks to.
    pub transport_address: SocketAddr,
    /// `transportAddress` as written, which identifies a remote candidate.
    pub transport_text: String,
    pub base_address: SocketAddr,
    /// The STUN or TURN server of a derived candidate; `None` for a host candidate.
    pub server_address: Option<SocketAddr>,
    /// Relayed candidates: the server-reflexive address of the allocation.
    pub nat_address: Option<SocketAddr>,
    pub kind: CandidateType,
    /// The `ipv6` flag as written: the family of the candidate's base, which can differ from
    /// `transport_address` (an IPv4 address reached from an IPv6-only network). Pairing compares it.
    pub ipv6: bool,
    pub address_type: AddressType,
    pub priority: u32,
    pub foundation: u32,
    pub transport: String,
    pub tunnel_transport: Option<String>,
    pub relay_standard: Option<String>,
}

impl Candidate {
    /// A candidate of the PC with `transport` `udp`.
    pub fn local(
        kind: CandidateType,
        transport_address: SocketAddr,
        base_address: SocketAddr,
        server_address: Option<SocketAddr>,
        local_preference: u16,
    ) -> Self {
        Self {
            transport_address,
            transport_text: format_address(transport_address),
            base_address,
            server_address,
            nat_address: None,
            kind,
            ipv6: transport_address.is_ipv6(),
            address_type: classify(transport_address.ip()),
            priority: priority(kind, local_preference),
            foundation: 0,
            transport: TRANSPORT_UDP.into(),
            tunnel_transport: None,
            relay_standard: None,
        }
    }

    pub fn is_ipv6(&self) -> bool {
        self.ipv6
    }

    fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("transportAddress".into(), leaf(&self.transport_text));
        m.insert("baseAddress".into(), leaf(format_address(self.base_address)));
        m.insert("natAddress".into(), leaf(self.nat_address.map(format_address).unwrap_or_default()));
        m.insert("serverAddress".into(), leaf(self.server_address.map(format_address).unwrap_or_default()));
        m.insert("ipv6".into(), ptree::bool_leaf(self.is_ipv6()));
        m.insert("type".into(), leaf(self.kind as u8));
        m.insert("addressType".into(), leaf(self.address_type as u8));
        m.insert("priority".into(), leaf(self.priority));
        m.insert("foundation".into(), leaf(self.foundation));
        m.insert("transport".into(), leaf(&self.transport));

        if let Some(t) = &self.tunnel_transport {
            m.insert("tunnelTransport".into(), leaf(t));
        }
        if let Some(r) = &self.relay_standard {
            m.insert("relayStandard".into(), leaf(r));
        }

        Value::Object(m)
    }

    fn from_json(node: &Value, at: &str) -> Result<Self, DescriptionError> {
        let path = |key: &str| format!("Candidates.{at}.{key}");
        let address = |key: &str, required: bool| -> Result<Option<(SocketAddr, String)>, DescriptionError> {
            match ptree::text(node, key)? {
                Some(t) if !t.is_empty() => match parse_address(&t) {
                    Some(a) => Ok(Some((a, t))),
                    None => Err(DescriptionError::BadAddress(path(key), t)),
                },
                _ if required => Err(PtreeError::Missing(path(key)).into()),
                _ => Ok(None),
            }
        };

        let (transport_address, transport_text) = address("transportAddress", true)?.expect("required");
        let (base_address, _) = address("baseAddress", true)?.expect("required");
        let kind = ptree::required_uint(node, "type")?;
        let kind = CandidateType::from_wire(kind).ok_or(DescriptionError::BadValue(path("type"), kind))?;
        let address_type = ptree::required_uint(node, "addressType")?;
        let address_type = AddressType::from_wire(address_type)
            .ok_or(DescriptionError::BadValue(path("addressType"), address_type))?;
        let ipv6 = ptree::boolean(node, "ipv6")?.ok_or_else(|| PtreeError::Missing(path("ipv6")))?;
        let priority = ptree::required_uint(node, "priority")?;
        let foundation = ptree::required_uint(node, "foundation")?;

        Ok(Self {
            transport_address,
            transport_text,
            base_address,
            server_address: address("serverAddress", false)?.map(|(a, _)| a),
            nat_address: address("natAddress", false)?.map(|(a, _)| a),
            kind,
            ipv6,
            address_type,
            priority: u32::try_from(priority).map_err(|_| DescriptionError::BadValue(path("priority"), priority))?,
            foundation: u32::try_from(foundation)
                .map_err(|_| DescriptionError::BadValue(path("foundation"), foundation))?,
            transport: ptree::text(node, "transport")?.unwrap_or_default(),
            tunnel_transport: ptree::text(node, "tunnelTransport")?.filter(|t| !t.is_empty()),
            relay_standard: ptree::text(node, "relayStandard")?.filter(|t| !t.is_empty()),
        })
    }
}

/// Assigns foundations: equal for candidates sharing (base, server, type), different otherwise,
/// numbered from 1 in order of first appearance.
pub fn assign_foundations(candidates: &mut [Candidate]) {
    let mut seen: HashMap<(SocketAddr, Option<SocketAddr>, CandidateType), u32> = HashMap::new();
    for c in candidates {
        let next = seen.len() as u32 + 1;
        c.foundation = *seen.entry((c.base_address, c.server_address, c.kind)).or_insert(next);
    }
}

/// A failure reading a session description.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DescriptionError {
    #[error("the session description is not a JSON object: {0}")]
    NotJson(String),
    #[error(transparent)]
    Tree(#[from] PtreeError),
    #[error("unsupported session description version {0:?}")]
    Version(String),
    #[error("`{0}` is empty")]
    Empty(&'static str),
    #[error("the session description has no candidates")]
    NoCandidates,
    #[error("`{0}` is not an address: {1:?}")]
    BadAddress(String, String),
    #[error("`{0}` has an unknown value {1}")]
    BadValue(String, u64),
}

/// The ICE session description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDescription {
    /// ICE ufrag.
    pub username: String,
    /// ICE password.
    pub password: String,
    pub pacing_ms: Option<u32>,
    pub stun_timeout_ms: Option<u32>,
    pub stun_retry_count: Option<u32>,
    pub candidates: Vec<Candidate>,
}

impl SessionDescription {
    /// The PC's description: its credentials, its pacing and STUN timer values, and its candidates.
    pub fn local(username: String, password: String, candidates: Vec<Candidate>) -> Self {
        Self {
            username,
            password,
            pacing_ms: Some(PACING_MS),
            stun_timeout_ms: Some(STUN_TIMEOUT_MS),
            stun_retry_count: Some(STUN_RETRY_COUNT),
            candidates,
        }
    }

    /// The JSON text that goes into `ice.candidates`.
    pub fn to_json_string(&self) -> String {
        let mut list = Map::new();
        list.insert("count".into(), leaf(self.candidates.len()));
        for (i, c) in self.candidates.iter().enumerate() {
            list.insert(i.to_string(), c.to_json());
        }

        let mut m = Map::new();
        m.insert("Full".into(), leaf("1"));
        if let Some(p) = self.pacing_ms {
            m.insert("PacingMs".into(), leaf(p));
        }
        m.insert("Version".into(), leaf(DESCRIPTION_VERSION));
        m.insert("Candidates".into(), Value::Object(list));
        m.insert("Username".into(), leaf(&self.username));
        m.insert("Password".into(), leaf(&self.password));
        if let Some(t) = self.stun_timeout_ms {
            m.insert("StunTimeoutMs".into(), leaf(t));
        }
        if let Some(r) = self.stun_retry_count {
            m.insert("StunRetryCount".into(), leaf(r));
        }

        Value::Object(m).to_string()
    }

    /// Reads a peer's description: version `1`, non-empty credentials, at least one candidate.
    pub fn parse(text: &str) -> Result<Self, DescriptionError> {
        let root: Value = serde_json::from_str(text).map_err(|e| DescriptionError::NotJson(e.to_string()))?;
        if !root.is_object() {
            return Err(DescriptionError::NotJson("not an object".into()));
        }

        let version = ptree::required_text(&root, "Version")?;
        if version.trim() != DESCRIPTION_VERSION {
            return Err(DescriptionError::Version(version));
        }

        let username = ptree::required_text(&root, "Username")?;
        if username.is_empty() {
            return Err(DescriptionError::Empty("Username"));
        }

        let password = ptree::required_text(&root, "Password")?;
        if password.is_empty() {
            return Err(DescriptionError::Empty("Password"));
        }

        let small = |key: &str| -> Result<Option<u32>, DescriptionError> {
            match ptree::uint(&root, key)? {
                None => Ok(None),
                Some(v) => u32::try_from(v).map(Some).map_err(|_| DescriptionError::BadValue(key.into(), v)),
            }
        };

        let count = ptree::uint(&root, "Candidates.count")?.unwrap_or(0);
        let list = ptree::get(&root, "Candidates").cloned().unwrap_or(Value::Null);
        let mut candidates = Vec::new();
        for i in 0..count {
            let key = i.to_string();
            let node = list.get(&key).ok_or_else(|| PtreeError::Missing(format!("Candidates.{key}")))?;
            candidates.push(Candidate::from_json(node, &key)?);
        }

        if candidates.is_empty() {
            return Err(DescriptionError::NoCandidates);
        }

        Ok(Self {
            username,
            password,
            pacing_ms: small("PacingMs")?,
            stun_timeout_ms: small("StunTimeoutMs")?,
            stun_retry_count: small("StunRetryCount")?,
            candidates,
        })
    }
}

#[cfg(test)]
mod tests;
