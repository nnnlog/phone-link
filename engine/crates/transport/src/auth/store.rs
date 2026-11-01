//! What the control plane persists across a restart, under `auth/` keys of the [`SecureStore`].
//!
//! It covers the saved environment, the local account keys, per environment the identity record,
//! account id, identity key + certificate (and, during a key rotation, the incoming one) and trust
//! key + certificate, the trust relationships, the `AccountInfo` records and the peer cache.
//!
//! Records are
//! JSON. Each private key is stored **in one entry with its certificate** ([`KeyPairEntry`]: PKCS#8
//! DER + certificate DER), because a store write is atomic per entry only: a key can then never be
//! left without its certificate, or with another key's. The services token is never persisted.

use std::collections::BTreeMap;
use std::time::SystemTime;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use phonelink_core::edge::SecureStore;
use phonelink_core::types::{DeviceInfoItem, Environment};

use super::crypto::KeyPair;

/// Store keys.
pub mod keys {
    use phonelink_core::types::Environment;

    /// The snapped environment.
    pub const ENVIRONMENT: &str = "auth/environment";
    /// The local account (account id + CID).
    pub const ACCOUNT: &str = "auth/account";
    /// Every crypto-trust relationship.
    pub const TRUST_RELATIONSHIPS: &str = "auth/trust-relationships";
    /// `AccountInfo` records by account id.
    pub const ACCOUNT_INFO: &str = "auth/account-info";
    /// Last `GetDeviceInfoList` linked peers by account id.
    pub const PEER_CACHE: &str = "auth/peer-cache";
    /// This identity's own entry of the last `GetDeviceInfoList` (the peer cache excludes it).
    pub const OWN_ENTRY: &str = "auth/own-entry";

    fn env(e: Environment) -> &'static str {
        match e {
            Environment::Prod => "prod",
            Environment::Beta => "beta",
            Environment::Dogfood => "dogfood",
        }
    }

    /// Identity record of the environment.
    pub fn identity(e: Environment) -> String {
        format!("auth/{}/identity", env(e))
    }
    /// Account id of the environment.
    pub fn account_id(e: Environment) -> String {
        format!("auth/{}/account-id", env(e))
    }
    /// The identity key with its certificate `CN=<dcgClientId>` ([`super::KeyPairEntry`]).
    pub fn identity_pair(e: Environment) -> String {
        format!("auth/{}/identity-pair", env(e))
    }
    /// The incoming identity key + certificate of a key rotation.
    ///
    /// Written before `RotateKeys`, removed once the rotation is promoted or rejected — or, when
    /// `RotateKeys` got no reply, once a `SignIn` shows which key the server holds.
    pub fn identity_pending_pair(e: Environment) -> String {
        format!("auth/{}/identity-pair-pending", env(e))
    }
    /// The trust key with its certificate `CN=trust_<dcgClientId>`.
    pub fn trust_pair(e: Environment) -> String {
        format!("auth/{}/trust-pair", env(e))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("secure store: {0}")]
pub struct PersistError(pub String);

/// Milliseconds since the Unix epoch (persisted form of every UTC time).
pub mod time_ms {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    /// A time before the Unix epoch, which the store cannot hold.
    #[derive(Debug, thiserror::Error)]
    #[error("time before the Unix epoch")]
    pub struct BeforeEpoch;

    pub fn to_ms(t: &SystemTime) -> Result<u64, BeforeEpoch> {
        t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).map_err(|_| BeforeEpoch)
    }

    pub fn from_ms(ms: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(ms)
    }

    pub fn serialize<S: Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(to_ms(t).map_err(serde::ser::Error::custom)?)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        Ok(from_ms(u64::deserialize(d)?))
    }
}

/// `Option<SystemTime>` as optional milliseconds.
pub mod opt_time_ms {
    use super::time_ms;
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::SystemTime;

    pub fn serialize<S: Serializer>(t: &Option<SystemTime>, s: S) -> Result<S::Ok, S::Error> {
        match t {
            Some(t) => s.serialize_some(&time_ms::to_ms(t).map_err(serde::ser::Error::custom)?),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<SystemTime>, D::Error> {
        Ok(Option::<u64>::deserialize(d)?.map(time_ms::from_ms))
    }
}

/// Bytes as standard base64.
pub mod b64 {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::prelude::BASE64_STANDARD.encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        base64::prelude::BASE64_STANDARD.decode(text).map_err(serde::de::Error::custom)
    }
}

/// Optional bytes as standard base64.
pub mod opt_b64 {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(v) => s.serialize_some(&base64::prelude::BASE64_STANDARD.encode(v)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        match Option::<String>::deserialize(d)? {
            Some(text) => base64::prelude::BASE64_STANDARD.decode(text).map(Some).map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

/// The local account ids (account id and CID): one random GUID each, minted per signed-in Microsoft
/// account and never sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalAccount {
    pub account_id: String,
    pub cid: String,
    /// The Microsoft account's **PUID**, from the interactive sign-in that created this local account
    /// ([`crate::auth::live::login::DaProperty`]). `None` for a store written before the device-bound
    /// sign-in (which named the account by an ID token's `sub`, under a different key); such a store
    /// matches no sign-in, so the first device-bound sign-in switches to a freshly minted account.
    #[serde(default)]
    pub puid: Option<String>,
}

/// The identity record of one environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityRecord {
    /// `dcgClientId` — the GUID the PC generated.
    pub device_id: String,
    #[serde(with = "time_ms")]
    pub last_accessed: SystemTime,
    #[serde(with = "time_ms")]
    pub next_key_rotation: SystemTime,
    /// `keyValidRemainingDays` when the server reported a non-negative value, else `-1`.
    pub key_validity_remaining_days: i64,
    /// The environment base URL the identity belongs to.
    pub endpoint: String,
}

/// `AccountState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AccountState {
    NotProvisioned,
    Provisioned,
    Enrolled,
    /// Nothing in the enrollment flow produces it, so the engine never writes it and treats a stored
    /// one like `Provisioned`.
    EnrolledEnvironmentMismatch,
}

/// One `AccountInfo` record. `msa_cid` is kept here (local only) so deprovisioning can find the
/// account's relationships.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountInfo {
    pub account_id: String,
    pub msa_cid: String,
    pub state: AccountState,
    pub environment: Option<Environment>,
    pub dcg_client_id: Option<String>,
    #[serde(with = "opt_time_ms", default)]
    pub last_update_time: Option<SystemTime>,
    #[serde(with = "opt_time_ms", default)]
    pub last_attempt_update_time: Option<SystemTime>,
    #[serde(with = "opt_time_ms", default)]
    pub last_get_trusted_peer_clients_time: Option<SystemTime>,
    pub metadata_hash: Option<String>,
    pub msa_scope: Option<String>,
}

impl AccountInfo {
    pub fn new(account: &LocalAccount) -> Self {
        Self {
            account_id: account.account_id.clone(),
            msa_cid: account.cid.clone(),
            state: AccountState::NotProvisioned,
            environment: None,
            dcg_client_id: None,
            last_update_time: None,
            last_attempt_update_time: None,
            last_get_trusted_peer_clients_time: None,
            metadata_hash: None,
            msa_scope: None,
        }
    }

    /// `Provisioned`, `Enrolled` or `EnrolledEnvironmentMismatch`.
    pub fn is_provisioned(&self) -> bool {
        !matches!(self.state, AccountState::NotProvisioned)
    }
}

/// Peer cache: account id → linked peers of the last `GetDeviceInfoList`.
pub type PeerCache = BTreeMap<String, Vec<DeviceInfoItem>>;

/// One private key with its certificate, persisted as **one** store entry:
/// `{ "key": <base64 PKCS#8 DER>, "certificate": <base64 DER> }`. No `Debug`: it holds the key.
#[derive(Serialize, Deserialize)]
pub struct KeyPairEntry {
    #[serde(with = "b64")]
    pub key: Vec<u8>,
    #[serde(with = "b64")]
    pub certificate: Vec<u8>,
}

/// What a key + certificate entry held at load.
pub enum StoredPair {
    Absent,
    Present(Box<KeyPair>),
    /// Present but unusable: not a [`KeyPairEntry`], a key or certificate that does not decode, or
    /// a certificate that does not carry the key's public half.
    Unusable(String),
}

/// Typed access to the secure store.
pub struct Persist<'a>(pub &'a dyn SecureStore);

impl Persist<'_> {
    pub fn bytes(&self, key: &str) -> Result<Option<Vec<u8>>, PersistError> {
        self.0.load(key.to_string()).map_err(|e| PersistError(format!("load {key}: {e}")))
    }

    pub fn string(&self, key: &str) -> Result<Option<String>, PersistError> {
        match self.bytes(key)? {
            Some(b) => String::from_utf8(b).map(Some).map_err(|_| PersistError(format!("{key} is not UTF-8"))),
            None => Ok(None),
        }
    }

    pub fn json<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, PersistError> {
        match self.bytes(key)? {
            Some(b) => serde_json::from_slice(&b).map(Some).map_err(|e| PersistError(format!("{key}: {e}"))),
            None => Ok(None),
        }
    }

    pub fn put_bytes(&self, key: &str, value: Vec<u8>) -> Result<(), PersistError> {
        self.0.store(key.to_string(), value).map_err(|e| PersistError(format!("store {key}: {e}")))
    }

    pub fn put_json<T: Serialize>(&self, key: &str, value: &T) -> Result<(), PersistError> {
        let bytes = serde_json::to_vec(value).map_err(|e| PersistError(format!("{key}: {e}")))?;
        self.put_bytes(key, bytes)
    }

    pub fn delete(&self, key: &str) -> Result<(), PersistError> {
        self.0.delete(key.to_string()).map_err(|e| PersistError(format!("delete {key}: {e}")))
    }

    /// A key + certificate entry. Only a failure of the store itself is an error; an entry that is
    /// there but cannot be used is [`StoredPair::Unusable`].
    pub fn key_pair(&self, key: &str) -> Result<StoredPair, PersistError> {
        let Some(bytes) = self.bytes(key)? else { return Ok(StoredPair::Absent) };
        let entry: KeyPairEntry = match serde_json::from_slice(&bytes) {
            Ok(entry) => entry,
            Err(e) => return Ok(StoredPair::Unusable(format!("not a key pair entry: {e}"))),
        };

        Ok(match KeyPair::from_parts(&entry.key, &entry.certificate) {
            Ok(pair) => StoredPair::Present(Box::new(pair)),
            Err(e) => StoredPair::Unusable(e.to_string()),
        })
    }

    /// Writes `pair` as one entry.
    pub fn put_key_pair(&self, key: &str, pair: &KeyPair) -> Result<(), PersistError> {
        let entry = KeyPairEntry {
            key: pair.pkcs8().map_err(|e| PersistError(format!("{key}: {e}")))?,
            certificate: pair.cert.der.clone(),
        };
        self.put_json(key, &entry)
    }
}
