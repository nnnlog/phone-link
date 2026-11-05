//! The connection contract: the offer the PC creates and the answer the phone returns, exchanged over
//! DCG.
//!
//! Every leaf is a string (`ptree`); keys the contract does not name are ignored, so the
//! mirroring keys can sit beside the contract keys.

use std::net::{IpAddr, SocketAddr};
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde_json::{Map, Value};

use super::ice::candidate::{DescriptionError, SessionDescription};
use super::ptree::{self, PtreeError, leaf, put};

/// Master key and master salt lengths.
pub const MASTER_KEY_LEN: usize = 16;
pub const MASTER_SALT_LEN: usize = 14;

/// Default ports of a STUN or TURN entry written without one.
pub const STUN_DEFAULT_PORT: u16 = 3478;
pub const TURN_DEFAULT_PORT: u16 = 3478;

/// The SRTP master key and salt both directions use.
#[derive(Clone, PartialEq, Eq)]
pub struct MasterKey {
    pub key: [u8; MASTER_KEY_LEN],
    pub salt: [u8; MASTER_SALT_LEN],
}

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MasterKey(..)")
    }
}

impl MasterKey {
    /// A fresh key and salt from the CSPRNG.
    pub fn random() -> Self {
        Self { key: rand::random(), salt: rand::random() }
    }
}

/// A failure reading an answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContractError {
    #[error(transparent)]
    Tree(#[from] PtreeError),
    #[error("ice.candidates: {0}")]
    Description(#[from] DescriptionError),
    #[error("`{0}` is not valid hex of {1} bytes")]
    Hex(&'static str, usize),
    #[error("the answer lists no usable phone address")]
    NoAddress,
    #[error("`{0}` is out of range: {1}")]
    Range(&'static str, u64),
}

/// Lowercase hex, two digits per byte.
pub fn to_hex(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

/// Hex of exactly `N` bytes, either case.
fn from_hex<const N: usize>(text: &str, key: &'static str) -> Result<[u8; N], ContractError> {
    let bytes = hex::decode(text.trim()).map_err(|_| ContractError::Hex(key, N))?;
    bytes.try_into().map_err(|_| ContractError::Hex(key, N))
}

/// The STUN and TURN servers an `IceSrtp` offer names; the phone gathers with the same strings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IceServers {
    /// `ice.stun.url`: whitespace-separated STUN servers.
    pub stun_url: String,
    /// `ice.turn.url`: one TURN server, the whole string as given.
    pub turn_url: String,
    pub turn_username: String,
    pub turn_password: String,
}

impl IceServers {
    /// The STUN servers of `stun_url`: trimmed, split on whitespace, each entry `host[:port]`.
    pub fn stun_servers(&self) -> Vec<ServerEntry> {
        self.stun_url.split_whitespace().map(|e| ServerEntry::parse(e, STUN_DEFAULT_PORT)).collect()
    }

    /// The TURN server of `turn_url`: the string as given, neither trimmed nor split; `None` when it
    /// is empty.
    pub fn turn_server(&self) -> Option<ServerEntry> {
        (!self.turn_url.is_empty()).then(|| ServerEntry::parse(&self.turn_url, TURN_DEFAULT_PORT))
    }
}

/// Whether the PC contacts the TURN host at its resolved `address`: on both Nano paths only over IPv4,
/// so the TURN requests go out from IPv4 addresses and the relayed candidates are IPv4.
pub fn turn_address_usable(address: &SocketAddr) -> bool {
    address.is_ipv4()
}

/// One STUN or TURN server entry as the agents read it: anything that is not `host`, `host:port` or
/// `[v6]:port` becomes a host name as a whole (and then does not resolve).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerEntry {
    pub host: String,
    pub port: u16,
}

static SERVER_ENTRY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:\[(.*)\]|([^:]*))(?::(\d+))?$").expect("valid server-entry pattern"));

impl ServerEntry {
    pub fn parse(entry: &str, default_port: u16) -> Self {
        if let Some(c) = SERVER_ENTRY.captures(entry) {
            let host = c.get(1).or_else(|| c.get(2)).map(|m| m.as_str()).unwrap_or_default();
            let port = match c.get(3) {
                Some(p) => p.as_str().parse::<u16>().ok(),
                None => Some(default_port),
            };
            if let Some(port) = port {
                return Self { host: host.to_string(), port };
            }
        }
        Self { host: entry.to_string(), port: default_port }
    }
}

/// The `IceSrtp` offer.
#[derive(Debug, Clone)]
pub struct IceSrtpOffer {
    pub key: MasterKey,
    pub description: SessionDescription,
    pub servers: IceServers,
}

impl IceSrtpOffer {
    pub fn to_json(&self) -> Map<String, Value> {
        let mut root = Map::new();
        put(&mut root, "security.key", leaf(to_hex(&self.key.key)));
        put(&mut root, "security.salt", leaf(to_hex(&self.key.salt)));
        put(&mut root, "ice.candidates", leaf(self.description.to_json_string()));
        put(&mut root, "ice.stun.url", leaf(&self.servers.stun_url));
        put(&mut root, "ice.turn.url", leaf(&self.servers.turn_url));
        put(&mut root, "ice.turn.username", leaf(&self.servers.turn_username));
        put(&mut root, "ice.turn.password", leaf(&self.servers.turn_password));
        root
    }
}

/// The `IceSrtp` answer: the phone's description and its keepalive warning timeout. It carries no SRTP
/// key; both directions use the PC's.
#[derive(Debug, Clone)]
pub struct IceSrtpAnswer {
    pub description: SessionDescription,
    pub warning_timeout: Duration,
}

impl IceSrtpAnswer {
    pub fn parse(answer: &Value) -> Result<Self, ContractError> {
        let candidates = ptree::required_text(answer, "ice.candidates")?;
        let description = SessionDescription::parse(&candidates)?;
        Ok(Self { description, warning_timeout: warning_timeout(answer)? })
    }
}

/// `keepalive.warningtimeout` (ms), required in both answers.
fn warning_timeout(answer: &Value) -> Result<Duration, ContractError> {
    Ok(Duration::from_millis(ptree::required_uint(answer, "keepalive.warningtimeout")?))
}

/// The `IpSrtp` offer: no contract keys at all. (A `random` key `"0"` would make the phone use an
/// all-zero key and salt.)
pub fn ip_srtp_offer() -> Map<String, Value> {
    Map::new()
}

/// One entry of the `IpSrtp` answer's `ip` list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneInterface {
    pub name: String,
    pub address: IpAddr,
}

/// The `IpSrtp` answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpSrtpAnswer {
    pub key: MasterKey,
    pub interfaces: Vec<PhoneInterface>,
    pub port: u16,
    pub warning_timeout: Duration,
}

impl IpSrtpAnswer {
    pub fn parse(answer: &Value) -> Result<Self, ContractError> {
        let key = MasterKey {
            key: from_hex(&ptree::required_text(answer, "security.key")?, "security.key")?,
            salt: from_hex(&ptree::required_text(answer, "security.salt")?, "security.salt")?,
        };

        let port = ptree::required_uint(answer, "port")?;
        let port = u16::try_from(port).map_err(|_| ContractError::Range("port", port))?;

        let count = ptree::uint(answer, "ip.count")?.unwrap_or(0);
        let mut interfaces = Vec::new();
        for i in 0..count {
            let name = ptree::text(answer, &format!("ip.{i}.iname"))?.unwrap_or_default();
            let Some(address) = ptree::text(answer, &format!("ip.{i}.addr"))? else {
                continue;
            };
            let Ok(address) = address.trim().parse::<IpAddr>() else {
                continue;
            };
            interfaces.push(PhoneInterface { name, address });
        }

        Ok(Self { key, interfaces, port, warning_timeout: warning_timeout(answer)? })
    }

    /// The phone address to send to: the interface `wlan0`; else the first whose name starts with
    /// `wl`; else entry 0.
    pub fn select_address(&self) -> Result<SocketAddr, ContractError> {
        let pick = self
            .interfaces
            .iter()
            .find(|i| i.name == "wlan0")
            .or_else(|| self.interfaces.iter().find(|i| i.name.starts_with("wl")))
            .or_else(|| self.interfaces.first())
            .ok_or(ContractError::NoAddress)?;
        Ok(SocketAddr::new(pick.address, self.port))
    }
}

#[cfg(test)]
mod tests;
