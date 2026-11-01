//! The crypto-trust store — relationship records, trust validation rules and inbound JWT
//! validation.
//!
//! It covers at most 100 relationships, each binding one of the PC's trust certificates
//! (`self_client_id`) to one partner certificate.
//!
//! Pure logic over a `now` and a [`TrustPolicy`];
//! [`super::AuthService`] persists it and answers [`phonelink_core::seams::TrustGate`].

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use phonelink_core::config::AuthConfig;
use phonelink_core::constants::lifetime;
use phonelink_core::types::{DcgClient, Environment};

use super::crypto::{self, CryptoError, ParsedCertificate, TRUST_PREFIX};
use super::store::{b64, opt_b64, opt_time_ms, time_ms};

/// `CryptoTrustType`; `Unknown` is never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CryptoTrustType {
    Unknown,
    /// `LegacyD2D` in telemetry.
    ClassicPairingD2D,
    AccountPairingD2D,
    AsyncD2D,
    A2D,
}

impl CryptoTrustType {
    pub fn wire_value(self) -> u8 {
        match self {
            CryptoTrustType::Unknown => 0,
            CryptoTrustType::ClassicPairingD2D => 1,
            CryptoTrustType::AccountPairingD2D => 2,
            CryptoTrustType::AsyncD2D => 3,
            CryptoTrustType::A2D => 4,
        }
    }

    /// The two legacy pairing types.
    pub fn is_pairing(self) -> bool {
        matches!(self, CryptoTrustType::ClassicPairingD2D | CryptoTrustType::AccountPairingD2D)
    }
}

/// `attributes` key of the partner's `dcgClientId` in `env`.
pub fn attribute_key(env: Environment) -> &'static str {
    match env {
        Environment::Prod => "dcg_prod_client_id",
        Environment::Beta => "dcg_beta_client_id",
        Environment::Dogfood => "dcg_df_client_id",
    }
}

/// The legacy attribute (environment `Legacy`, which maps to no host).
pub const LEGACY_ATTRIBUTE: &str = "dcg_client_id";

/// `trust_<id>`.
pub fn trust_client_id(dcg_client_id: &str) -> String {
    format!("{TRUST_PREFIX}{dcg_client_id}")
}

/// One crypto-trust relationship.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TrustRelationship {
    /// `trust_<own dcgClientId>`.
    pub self_client_id: String,
    /// `trust_<partner dcgClientId>` (unique, case-insensitive).
    pub partner_client_id: String,
    /// The account CID on the account path.
    #[serde(default)]
    pub msa_cid: Option<String>,
    pub trust_type: CryptoTrustType,
    #[serde(with = "b64")]
    pub partner_certificate: Vec<u8>,
    #[serde(with = "opt_b64", default)]
    pub partner_temporary_certificate: Option<Vec<u8>>,
    #[serde(with = "time_ms")]
    pub partner_key_expiration_time: SystemTime,
    #[serde(with = "opt_time_ms", default)]
    pub partner_temporary_key_expiration_time: Option<SystemTime>,
    #[serde(with = "time_ms")]
    pub relationship_last_access_time: SystemTime,
    pub attributes: BTreeMap<String, String>,
}

impl TrustRelationship {
    /// The partner's `dcgClientId` in `env`, from `Attributes`.
    pub fn dcg_client_id(&self, env: Environment) -> Option<&str> {
        self.attributes.get(attribute_key(env)).map(String::as_str)
    }

    /// The partner's `DcgClient`s, one per environment attribute (`Prod`, `Beta`, `Dogfood`).
    pub fn dcg_clients(&self) -> Vec<DcgClient> {
        Environment::ALL
            .into_iter()
            .filter_map(|env| self.dcg_client_id(env).map(|id| DcgClient::new(id, env)))
            .collect()
    }

    fn msa_cid(&self) -> Option<&str> {
        self.msa_cid.as_deref().filter(|c| !c.is_empty())
    }
}

/// The timers of the trust store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustPolicy {
    /// `relationship_last_access_time + this > now` (180 d).
    pub relationship_expiration: Duration,
    /// `relationship_last_access_time` is refreshed at most this often (12 h).
    pub last_access_cooldown: Duration,
    /// Grace window of the previous partner certificate (10 min).
    pub partner_temporary_certificate: Duration,
    /// `partner_key_expiration_time ≤ now + this` (365 d).
    pub partner_key_cap: Duration,
    /// At most this many records (100).
    pub max_relationships: usize,
}

impl TrustPolicy {
    pub fn from_config(cfg: &AuthConfig) -> Self {
        Self {
            relationship_expiration: cfg.relationship_expiration,
            last_access_cooldown: cfg.relationship_last_access_cooldown,
            partner_temporary_certificate: cfg.partner_temporary_certificate,
            partner_key_cap: lifetime::PARTNER_KEY_CAP,
            max_relationships: lifetime::MAX_TRUST_RELATIONSHIPS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TrustStoreError {
    #[error("PartnerClientIdConflict: {0} is already bound")]
    PartnerClientIdConflict(String),
    #[error("PartnerDcgClientConflict: {0} is already bound")]
    PartnerDcgClientConflict(String),
    #[error("the trust store already holds {0} relationships")]
    Full(usize),
    #[error("partner certificate rejected: {0}")]
    Certificate(String),
}

/// `min(cert notAfter, now + cap)`; an already-expired certificate is rejected.
pub fn partner_key_expiration(
    cert: &ParsedCertificate,
    now: SystemTime,
    policy: &TrustPolicy,
) -> Result<SystemTime, TrustStoreError> {
    if cert.not_after <= now {
        return Err(TrustStoreError::Certificate("the certificate has expired".into()));
    }
    Ok(cert.not_after.min(now + policy.partner_key_cap))
}

/// Builds a new record for `partner_dcg_id` in `env` with `relationship_last_access_time = now`.
#[expect(clippy::too_many_arguments, reason = "independent inputs of one operation; not yet grouped")]
pub fn new_relationship(
    self_client_id: &str,
    partner_client_id: &str,
    msa_cid: &str,
    trust_type: CryptoTrustType,
    cert: &ParsedCertificate,
    env: Environment,
    partner_dcg_id: &str,
    now: SystemTime,
    policy: &TrustPolicy,
) -> Result<TrustRelationship, TrustStoreError> {
    assert!(trust_type != CryptoTrustType::Unknown, "Unknown is never stored");
    Ok(TrustRelationship {
        self_client_id: self_client_id.to_string(),
        partner_client_id: partner_client_id.to_string(),
        msa_cid: Some(msa_cid.to_string()),
        trust_type,
        partner_certificate: cert.der.clone(),
        partner_temporary_certificate: None,
        partner_key_expiration_time: partner_key_expiration(cert, now, policy)?,
        partner_temporary_key_expiration_time: None,
        relationship_last_access_time: now,
        attributes: BTreeMap::from([(attribute_key(env).to_string(), partner_dcg_id.to_string())]),
    })
}

/// Why an inbound JWT was not accepted.
#[derive(Debug, thiserror::Error)]
pub enum InboundJwtError {
    #[error("malformed JWT: {0}")]
    Malformed(String),
    #[error("malformed JWT: {0}")]
    MalformedJws(#[source] CryptoError),
    #[error("Scope claim {found:?} is not {expected:?}")]
    WrongScope { expected: String, found: Option<String> },
    #[error("no crypto-trust relationship with {0}")]
    Untrusted(String),
    #[error("rejected by every certificate of the relationship: {0}")]
    Invalid(String),
}

/// A validated inbound JWT: the `Data` claim and the signer's environment ids.
#[derive(Debug, Clone, PartialEq)]
pub struct InboundJwt {
    pub partner_client_id: String,
    pub data: Option<String>,
    pub signer: Vec<DcgClient>,
    pub claims: serde_json::Map<String, serde_json::Value>,
}

/// The relationships, in insertion order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TrustStore {
    records: Vec<TrustRelationship>,
}

impl TrustStore {
    pub fn from_records(records: Vec<TrustRelationship>) -> Self {
        Self { records }
    }

    pub fn records(&self) -> &[TrustRelationship] {
        &self.records
    }

    #[cfg(feature = "test-support")]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    #[cfg(feature = "test-support")]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// **Unexpired** ⇔ `partner_key_expiration_time > now` and
    /// `relationship_last_access_time + 180 d > now`.
    pub fn is_unexpired(r: &TrustRelationship, now: SystemTime, policy: &TrustPolicy) -> bool {
        r.partner_key_expiration_time > now && r.relationship_last_access_time + policy.relationship_expiration > now
    }

    /// Trusted: unexpired, and for `AsyncD2D` the `A2D` record with the same `msa_cid` is unexpired
    /// **while one exists** — with no `A2D` record at all it is trusted. The dependency decides
    /// trust only; it never removes a record ([`TrustStore::prune`]).
    pub fn is_valid(&self, r: &TrustRelationship, now: SystemTime, policy: &TrustPolicy) -> bool {
        if !Self::is_unexpired(r, now, policy) {
            return false;
        }
        if r.trust_type != CryptoTrustType::AsyncD2D {
            return true;
        }

        let Some(cid) = r.msa_cid() else { return true };
        match self.records.iter().find(|a| a.trust_type == CryptoTrustType::A2D && a.msa_cid() == Some(cid)) {
            Some(a2d) => Self::is_unexpired(a2d, now, policy),
            None => true,
        }
    }

    /// Removes every **expired** record (as on every access and every add / list) and drops
    /// temporary certificates past their window. An `AsyncD2D` record whose `A2D` expired is not
    /// expired itself: it stays, and with the expired `A2D` gone it is trusted until the next cloud
    /// refresh recreates the `A2D`. Returns whether anything changed.
    pub fn prune(&mut self, now: SystemTime, policy: &TrustPolicy) -> bool {
        let before = self.records.len();
        self.records.retain(|r| Self::is_unexpired(r, now, policy));
        let mut changed = self.records.len() != before;

        for r in &mut self.records {
            if r.partner_temporary_key_expiration_time.is_some_and(|t| t <= now) {
                r.partner_temporary_certificate = None;
                r.partner_temporary_key_expiration_time = None;
                changed = true;
            }
        }
        changed
    }

    /// Whether any unexpired relationship exists (the environment snap rule of environment
    /// discovery).
    pub fn has_unexpired(&self, now: SystemTime, policy: &TrustPolicy) -> bool {
        self.records.iter().any(|r| Self::is_unexpired(r, now, policy))
    }

    /// The record naming `client` (by environment attribute) when it is trusted
    /// ([`TrustStore::is_valid`]).
    pub fn trusted_index_by_dcg_client(
        &self,
        client: &DcgClient,
        now: SystemTime,
        policy: &TrustPolicy,
    ) -> Option<usize> {
        self.index_by_dcg_client(client).filter(|&index| self.is_valid(&self.records[index], now, policy))
    }

    pub fn index_by_partner_client_id(&self, partner_client_id: &str) -> Option<usize> {
        self.records.iter().position(|r| r.partner_client_id.eq_ignore_ascii_case(partner_client_id))
    }

    /// The record whose `Attributes[dcg_<env>_client_id]` equals `client.id` (case-insensitive).
    pub fn index_by_dcg_client(&self, client: &DcgClient) -> Option<usize> {
        self.records
            .iter()
            .position(|r| r.dcg_client_id(client.env).is_some_and(|id| id.eq_ignore_ascii_case(&client.id)))
    }

    pub fn get(&self, index: usize) -> &TrustRelationship {
        &self.records[index]
    }

    pub fn get_mut(&mut self, index: usize) -> &mut TrustRelationship {
        &mut self.records[index]
    }

    /// Adds `record` after pruning; a bound `partner_client_id` or `(dcgClientId, environment)` is a
    /// conflict, and the store holds at most `max_relationships`.
    pub fn add(
        &mut self,
        record: TrustRelationship,
        now: SystemTime,
        policy: &TrustPolicy,
    ) -> Result<(), TrustStoreError> {
        self.prune(now, policy);

        if self.index_by_partner_client_id(&record.partner_client_id).is_some() {
            return Err(TrustStoreError::PartnerClientIdConflict(record.partner_client_id));
        }

        for client in record.dcg_clients() {
            if self.index_by_dcg_client(&client).is_some() {
                return Err(TrustStoreError::PartnerDcgClientConflict(client.to_string()));
            }
        }

        if self.records.len() >= policy.max_relationships {
            return Err(TrustStoreError::Full(self.records.len()));
        }

        self.records.push(record);
        Ok(())
    }

    /// Replaces the partner certificate of record `index`; the previous one becomes the temporary
    /// certificate for `partner_temporary_certificate`. Returns `false` (nothing changed) when the
    /// certificate is the one already stored.
    pub fn rotate_partner_certificate(
        &mut self,
        index: usize,
        cert: &ParsedCertificate,
        now: SystemTime,
        policy: &TrustPolicy,
    ) -> Result<bool, TrustStoreError> {
        let expiration = partner_key_expiration(cert, now, policy)?;
        let r = &mut self.records[index];
        if r.partner_certificate == cert.der {
            return Ok(false);
        }

        let previous = std::mem::replace(&mut r.partner_certificate, cert.der.clone());
        r.partner_temporary_certificate = Some(previous);
        r.partner_temporary_key_expiration_time = Some(now + policy.partner_temporary_certificate);
        r.partner_key_expiration_time = expiration;
        Ok(true)
    }

    /// Refreshes `relationship_last_access_time` of a validated hit when older than the cooldown.
    pub fn touch(&mut self, index: usize, now: SystemTime, policy: &TrustPolicy) -> bool {
        let r = &mut self.records[index];
        if r.relationship_last_access_time + policy.last_access_cooldown < now {
            r.relationship_last_access_time = now;
            true
        } else {
            false
        }
    }

    /// Removes every record matching `f`; returns how many.
    pub fn remove_where(&mut self, mut f: impl FnMut(&TrustRelationship) -> bool) -> usize {
        let before = self.records.len();
        self.records.retain(|r| !f(r));
        before - self.records.len()
    }

    pub fn remove(&mut self, index: usize) -> TrustRelationship {
        self.records.remove(index)
    }

    /// Resolving a partner id to a `DcgClient`: the id as given, then `trust_<id>`; the first
    /// environment in `Beta, Prod, Dogfood, Legacy` order with an attribute (`Legacy` maps to no
    /// host, so a record with only the legacy attribute resolves to `None`).
    pub fn resolve_partner(&self, id: &str) -> Option<DcgClient> {
        let index =
            self.index_by_partner_client_id(id).or_else(|| self.index_by_partner_client_id(&trust_client_id(id)))?;
        let r = &self.records[index];
        [Environment::Beta, Environment::Prod, Environment::Dogfood]
            .into_iter()
            .find_map(|env| r.dcg_client_id(env).map(|id| DcgClient::new(id, env)))
    }

    /// Inbound JWT validation: the unverified `iss` names the relationship (`partner_client_id`);
    /// the JWS must validate with its `partner_certificate`, else with its unexpired
    /// `partner_temporary_certificate` (issuer = CN, audience = `self_client_id` when present,
    /// lifetime with 12 h skew, ECDSA key). Prunes first and refreshes the record's last access on
    /// success; the caller persists.
    pub fn validate_inbound_jwt(
        &mut self,
        token: &str,
        expected_scope: Option<&str>,
        now: SystemTime,
        policy: &TrustPolicy,
    ) -> Result<InboundJwt, InboundJwtError> {
        let jws = crypto::decode_jws(token).map_err(InboundJwtError::MalformedJws)?;
        let iss = jws.claim_str("iss").ok_or_else(|| InboundJwtError::Malformed("no iss claim".into()))?.to_string();

        if let Some(expected) = expected_scope {
            let found = jws.claim_str("Scope");
            if found != Some(expected) {
                return Err(InboundJwtError::WrongScope {
                    expected: expected.to_string(),
                    found: found.map(str::to_string),
                });
            }
        }

        self.prune(now, policy);
        let index = self
            .index_by_partner_client_id(&iss)
            .filter(|&index| self.is_valid(&self.records[index], now, policy))
            .ok_or_else(|| InboundJwtError::Untrusted(iss.clone()))?;

        let r = &self.records[index];
        let mut candidates = vec![r.partner_certificate.clone()];
        if let (Some(temp), Some(until)) = (&r.partner_temporary_certificate, r.partner_temporary_key_expiration_time) {
            if until > now {
                candidates.push(temp.clone());
            }
        }

        let mut errors: Vec<String> = Vec::new();
        let mut accepted = false;
        for der in candidates {
            let outcome: Result<(), CryptoError> = crypto::parse_certificate(&der)
                .and_then(|cert| crypto::validate_jws_with_certificate(&jws, &cert, &r.self_client_id, now));
            match outcome {
                Ok(()) => {
                    accepted = true;
                    break;
                }
                Err(e) => errors.push(e.to_string()),
            }
        }

        if !accepted {
            return Err(InboundJwtError::Invalid(errors.join("; ")));
        }

        let result = InboundJwt {
            partner_client_id: r.partner_client_id.clone(),
            data: jws.claim_str("Data").map(str::to_string),
            signer: r.dcg_clients(),
            claims: jws.claims.clone(),
        };

        self.touch(index, now, policy);
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
