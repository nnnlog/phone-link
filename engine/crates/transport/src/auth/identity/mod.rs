//! The identity side of the control plane.
//!
//! It covers the startup and provisioning sequence, steps 1–7 (with the strong-auth retry of
//! `EnrollDevice`; the steps are numbered on [`AuthService::ensure_provisioned`]), the identity
//! lifecycle (active / cleared / unlinked, deprovisioning), the services token refresh policy,
//! identity key rotation (crash-safe: the incoming key is persisted before `RotateKeys`) and trust
//! certificate rotation; [`AuthService`] implements [`TokenSource`] here.

use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use phonelink_core::seams::{TokenError, TokenSource};
use phonelink_core::types::Environment;

use super::crypto::{self};
use super::dcg_rest::{DeviceMetadata, RestError};
use super::{AuthError, AuthService};

mod key_rotation;
mod lifecycle;
pub(crate) use lifecycle::Deprovision;
mod provisioning;
mod services_token;

/// The metadata hash — any stable hash over `clientType + clientVersion + displayName + osName +
/// osVersion + capabilities` (the client-info refresh, step 7).
///
/// SHA-256, lower-case hex, with separators so field boundaries cannot shift.
///
/// Local only.
pub fn metadata_hash(m: &DeviceMetadata) -> String {
    let mut h = Sha256::new();
    for part in [&m.client_type, &m.client_version, &m.display_name, &m.os_name, &m.os_version] {
        h.update(part.as_bytes());
        h.update([0u8]);
    }

    for capability in &m.capabilities {
        h.update(capability.as_bytes());
        h.update([0x1f]);
    }

    hex::encode(h.finalize())
}

/// Environment discovery result classes; `Unauthorized` and the token failures abort before a
/// value exists.
enum Discovery {
    Success(Environment),
    Defaulted,
}

fn since(now: SystemTime, then: Option<SystemTime>) -> Option<Duration> {
    then.map(|t| now.duration_since(t).unwrap_or(Duration::ZERO))
}

/// Why an `/Auth` exchange with one key (`SignIn`, `RotateKeys`) produced no result.
enum AuthCallFailure {
    /// The service's answer — or no answer ([`RestError::Transport`]) — for the caller's rules.
    Rest(RestError),
    /// A local failure; nothing was sent for that attempt.
    Local(AuthError),
}

impl From<crypto::CryptoError> for AuthCallFailure {
    fn from(e: crypto::CryptoError) -> Self {
        AuthCallFailure::Local(e.into())
    }
}

impl AuthService {}

#[async_trait]
impl TokenSource for AuthService {
    async fn services_token(&self, env: Environment, force_refresh: bool) -> Result<String, TokenError> {
        self.services_token_for(env, force_refresh).await.map_err(|e| e.to_token_error())
    }
}

#[cfg(test)]
mod tests;
