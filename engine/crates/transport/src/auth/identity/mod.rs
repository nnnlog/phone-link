//! The identity side of the control plane.
//!
//! It covers the startup and provisioning sequence, steps 1–7 (with the strong-auth retry of
//! `EnrollDevice`; the steps are numbered on [`AuthService::ensure_provisioned`]), the identity
//! lifecycle (active / cleared / unlinked, deprovisioning, and an identity created anew in place and
//! enrolled again), the services token refresh
//! policy, identity key rotation (crash-safe: the incoming key is persisted before `RotateKeys`) and
//! trust certificate rotation; [`AuthService`] implements [`TokenSource`] here.

use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use phonelink_core::seams::{TokenError, TokenSource};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::Environment;

use super::crypto::{self};
use super::dcg_rest::{DeviceMetadata, RestError, TokenResponse};
use super::{AuthError, AuthService};

mod key_rotation;
mod lifecycle;
pub(crate) use lifecycle::Deprovision;
mod provisioning;
mod replacement;
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

/// Why an `/Auth` exchange (`SignIn`, `RotateKeys`, `CreateIdentity`) produced no result.
enum AuthCallFailure {
    /// The service's answer — or no answer ([`RestError::Transport`]) — for the caller's rules.
    Rest(RestError),
    /// The identity has no key and certificate to sign the nonce with (`SignIn` after its
    /// `GenerateNonce`).
    KeyMissing,
    /// A local failure; nothing was sent for that attempt.
    Local(AuthError),
}

impl From<crypto::CryptoError> for AuthCallFailure {
    fn from(e: crypto::CryptoError) -> Self {
        AuthCallFailure::Local(e.into())
    }
}

/// An identity `CreateIdentity` created, not yet stored: the GUID the PC generated, its key and
/// certificate, and the services token the service answered with.
struct NewIdentity {
    device_id: String,
    pair: crypto::KeyPair,
    token: TokenResponse,
}

/// Why a services-token refresh (`SignIn` and the key rotation after it) produced no token.
enum RefreshError {
    /// The identity is lost: the service rejected its certificate (`InvalidCertificate`) on `SignIn`
    /// or on the key rotation, or its key and certificate are missing; it is created anew in place.
    IdentityLost,
    /// Any other failure, its rules applied (an unlinked device already handled).
    Failed(AuthError),
}

impl From<AuthError> for RefreshError {
    fn from(e: AuthError) -> Self {
        RefreshError::Failed(e)
    }
}

#[async_trait]
impl TokenSource for AuthService {
    async fn services_token(&self, env: Environment, force_refresh: bool) -> Result<String, TokenError> {
        self.services_token_for(env, force_refresh).await.map_err(|e| e.to_token_error())
    }

    async fn invalidate_services_token(&self, env: Environment) {
        let _guard = self.token_lock.lock().await;
        self.state.lock_or_panic().env_mut(env).services_token = None;
    }
}

#[cfg(test)]
mod tests;
