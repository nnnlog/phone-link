//! The per-identity MSA access-token provider.
//!
//! It is a thin view over the shared [`LiveBroker`]: each identity asks the broker for a token under
//! its own client id and the RPS DCG or strong-auth scope of the target environment. The broker owns
//! the device credential, the device ticket and the one signed-in user session both identities share
//! ([`crate::auth::live`]); this type only maps the broker's outcomes onto [`MsaError`] and tracks
//! whether the account needs the user's attention.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::auth::live::broker::{BrokerError, LiveBroker};
use phonelink_core::constants::ScopeFamily;
use phonelink_core::types::Environment;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MsaError {
    #[error("the {0:?} scope family cannot be used by the device-bound sign-in")]
    UnsupportedScopeFamily(ScopeFamily),
    #[error("MSA sign-in needs the user: {0}")]
    InteractionRequired(String),
    #[error("the user declined the sign-in")]
    Declined,
    /// The token endpoint could not be reached: no connectivity, or the request got no HTTP response
    /// because of the network.
    #[error("no internet connectivity")]
    Internet,
    #[error("MSA token exchange failed: {0}")]
    Endpoint(String),
    #[error("unexpected MSA response: {0}")]
    InvalidResponse(String),
    #[error("secure store: {0}")]
    Store(String),
}

impl From<BrokerError> for MsaError {
    fn from(e: BrokerError) -> Self {
        match e {
            BrokerError::Internet => MsaError::Internet,
            BrokerError::InteractionRequired(r) => MsaError::InteractionRequired(r),
            BrokerError::Declined => MsaError::Declined,
            BrokerError::Device(r) | BrokerError::Exchange(r) => MsaError::Endpoint(r),
            BrokerError::DeviceStep(e) => MsaError::Endpoint(e.to_string()),
            BrokerError::Store(r) => MsaError::Store(r),
            BrokerError::StoreStep(e) => MsaError::Store(e.to_string()),
        }
    }
}

/// One identity's MSA token provider over the shared broker.
pub struct MsaClient {
    broker: Arc<LiveBroker>,
    client_id: String,
    family: ScopeFamily,
    /// The account needs the user's attention: the last silent token request answered that the user
    /// must sign in. Cleared by the next success.
    needs_attention: AtomicBool,
}

impl MsaClient {
    pub fn new(broker: Arc<LiveBroker>, client_id: &str, family: ScopeFamily) -> Self {
        Self { broker, client_id: client_id.to_string(), family, needs_attention: AtomicBool::new(false) }
    }

    /// The RPS DCG scope of `env` (the control-plane bearer).
    pub fn scope(&self, env: Environment) -> Result<String, MsaError> {
        match self.family {
            ScopeFamily::Rps => Ok(self.family.dcg_scope(env).to_string()),
            other => Err(MsaError::UnsupportedScopeFamily(other)),
        }
    }

    /// The RPS strong-auth (SAPI) scope of `env` (the `EnrollDevice` repair bearer).
    pub fn strong_auth_scope(&self, env: Environment) -> Result<String, MsaError> {
        match self.family {
            ScopeFamily::Rps => Ok(self.family.strong_auth_scope(env).to_string()),
            other => Err(MsaError::UnsupportedScopeFamily(other)),
        }
    }

    /// The access token of `env`'s DCG scope, minted through the broker (the device-bound exchange,
    /// running the web view for a sign-in or consent when needed).
    pub async fn access_token(&self, env: Environment) -> Result<String, MsaError> {
        let scope = self.scope(env)?;
        self.token(&scope).await
    }

    /// The access token of `env`'s strong-auth scope.
    pub async fn strong_auth_token(&self, env: Environment) -> Result<String, MsaError> {
        let scope = self.strong_auth_scope(env)?;
        self.token(&scope).await
    }

    async fn token(&self, scope: &str) -> Result<String, MsaError> {
        match self.broker.token(scope, &self.client_id).await {
            Ok(t) => {
                // A sign-in clears it in `ensure_signed_in`, whose caller logs the sign-in.
                if self.needs_attention.swap(false, Ordering::SeqCst) {
                    tracing::info!("the account no longer needs the user's attention; a token was issued");
                }
                Ok(t)
            }
            Err(e @ (BrokerError::InteractionRequired(_) | BrokerError::Declined)) => {
                self.mark_needs_attention(&e);
                Err(e.into())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Ensures a signed-in user session (running the interactive sign-in when there is none) and
    /// returns the account PUID.
    pub async fn ensure_signed_in(&self) -> Result<String, MsaError> {
        match self.broker.sign_in().await {
            Ok(puid) => {
                self.needs_attention.store(false, Ordering::SeqCst);
                Ok(puid)
            }
            Err(e @ (BrokerError::InteractionRequired(_) | BrokerError::Declined)) => {
                self.mark_needs_attention(&e);
                Err(e.into())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// The PUID of the stored user session, or `None` when not signed in.
    pub fn account_puid(&self) -> Option<String> {
        self.broker.account_puid().ok().flatten()
    }

    /// The sign-in name of the stored user session, when it has one.
    pub fn account_sign_in_name(&self) -> Option<String> {
        self.broker.account_sign_in_name().ok().flatten()
    }

    /// Marks the account as needing the user's attention after `e`, logging only the change.
    fn mark_needs_attention(&self, e: &BrokerError) {
        if !self.needs_attention.swap(true, Ordering::SeqCst) {
            tracing::info!(error = %e, "the account needs the user's attention; no token without a sign-in");
        }
    }

    pub fn needs_attention(&self) -> bool {
        self.needs_attention.load(Ordering::SeqCst)
    }

    /// Drops the cached DCG-scope token of `env` (after DCG rejected it).
    pub fn invalidate(&self, env: Environment) {
        if let Ok(scope) = self.scope(env) {
            self.broker.invalidate(&scope, &self.client_id);
        }
    }

    /// Drops the cached strong-auth token of `env`.
    pub fn invalidate_strong_auth(&self, env: Environment) {
        if let Ok(scope) = self.strong_auth_scope(env) {
            self.broker.invalidate(&scope, &self.client_id);
        }
    }

    /// Forgets the user session and cached tokens (sign-out), keeping the machine's device
    /// credential. Shared across identities; idempotent.
    pub fn sign_out(&self) -> Result<(), MsaError> {
        self.needs_attention.store(true, Ordering::SeqCst);
        Ok(self.broker.forget_user()?)
    }

    /// The shared broker, for operations that are not per-identity (reset device registration).
    pub fn broker(&self) -> &Arc<LiveBroker> {
        &self.broker
    }
}
