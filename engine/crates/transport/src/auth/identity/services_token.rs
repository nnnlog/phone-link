//! The services token the relay and REST calls carry.

use super::AuthCallFailure;
use crate::auth::crypto::KeyPair;
use crate::auth::dcg_rest::TokenResponse;
use crate::auth::{AuthError, AuthService, crypto};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::{Environment, TraceContext};

impl AuthService {
    /// The services token refresh policy: the cached token of `env` unless missing, `force`d, or
    /// `epochExpirationTime − leeway < now`; then `GenerateNonce` + `SignIn` with the existing
    /// identity certificate, followed by the key-rotation check.
    pub async fn services_token_for(&self, env: Environment, force: bool) -> Result<String, AuthError> {
        let _guard = self.token_lock.lock().await;

        {
            let st = self.state.lock_or_panic();
            let slot = st.env(env);
            if slot.identity.is_none() || slot.identity_keys.is_none() {
                return Err(AuthError::NotProvisioned(format!("no identity in {env}")));
            }

            if !force {
                if let Some(token) = &slot.services_token {
                    let fresh = token
                        .expires_at
                        .checked_sub(self.config.auth.token_refresh_leeway)
                        .is_some_and(|limit| limit >= self.now());
                    if fresh {
                        return Ok(token.access_token.clone());
                    }
                }
            }
        }

        let msa = self.msa_token(env).await?;
        self.sign_in_identity(env, &msa).await
    }

    /// The refresh policy's `GenerateNonce` + `SignIn` with the existing identity certificate. When
    /// `SignIn` rejects it (`InvalidCertificate`) while a key rotation is pending — the server took
    /// `RotateKeys` but the promotion never happened (the persisted state keeps, briefly, the
    /// outgoing key during rotation) — `SignIn` is retried with the pending key, which is promoted
    /// on success; only when that key is rejected too is the identity cleared (`InvalidIdentity`).
    pub(super) async fn sign_in_identity(&self, env: Environment, msa: &str) -> Result<String, AuthError> {
        let (device_id, current, pending) = {
            let st = self.state.lock_or_panic();
            let slot = st.env(env);
            match (&slot.identity, &slot.identity_keys) {
                (Some(r), Some(p)) => (r.device_id.clone(), p.clone(), slot.pending_identity_keys.clone()),
                _ => return Err(AuthError::NotProvisioned(format!("no identity in {env}"))),
            }
        };

        let (token, promote) = match (self.sign_in_with(env, msa, &device_id, &current).await, pending) {
            (Ok(token), _) => (token, None),
            (Err(AuthCallFailure::Rest(e)), Some(pending)) if e.is_invalid_certificate() => {
                tracing::warn!(%env, error = %e, "SignIn rejected the identity certificate during a key rotation; signing in with the incoming key");
                match self.sign_in_with(env, msa, &device_id, &pending).await {
                    Ok(token) => (token, Some(pending)),
                    Err(AuthCallFailure::Rest(e)) => return Err(self.auth_failure(env, e)),
                    Err(AuthCallFailure::Local(e)) => return Err(e),
                }
            }
            (Err(AuthCallFailure::Rest(e)), _) => return Err(self.auth_failure(env, e)),
            (Err(AuthCallFailure::Local(e)), _) => return Err(e),
        };

        if !token.device_id.eq_ignore_ascii_case(&device_id) {
            return Err(AuthError::Protocol("SignIn deviceId echo differs from the stored deviceId".into()));
        }

        let (access, pending_key_outcome) = self.store_signed_in(env, &device_id, &token, promote)?;
        tracing::info!(%env, ?pending_key_outcome, "services token refreshed with SignIn");

        self.rotate_identity_key_if_due(env, msa).await?;
        Ok(access)
    }

    /// `GenerateNonce(deviceId)` → nonce JWT signed with `pair` → `SignIn`; `InvalidNonce` is retried
    /// with a fresh nonce up to `auth_recover_retries` times.
    pub(super) async fn sign_in_with(
        &self,
        env: Environment,
        msa: &str,
        device_id: &str,
        pair: &KeyPair,
    ) -> Result<TokenResponse, AuthCallFailure> {
        let retries = self.config.auth.auth_recover_retries;
        let mut attempt = 0;

        loop {
            let scenario = TraceContext::new_root(Vec::new());
            let nonce = self
                .rest
                .generate_nonce(env, msa, device_id, &scenario.child())
                .await
                .map_err(AuthCallFailure::Rest)?;

            let jwt = crypto::nonce_jwt(pair, &nonce, self.now())?;
            match self.rest.sign_in(env, msa, &jwt, &scenario.child()).await {
                Ok(token) => return Ok(token),
                Err(e) if attempt < retries && e.is_invalid_nonce() => {
                    attempt += 1;
                    tracing::warn!(%env, attempt, "SignIn InvalidNonce; retrying with a new nonce");
                }
                Err(e) => return Err(AuthCallFailure::Rest(e)),
            }
        }
    }

    /// A successful `SignIn` is an identity write (`last_accessed = now`) and stores
    /// `keyValidRemainingDays`. With `promote` (the pending rotation key signed in) that key becomes
    /// the identity key; otherwise a pending key is one the server never took, and it is dropped.
    /// Returns the access token and what became of a pending key.
    pub(super) fn store_signed_in(
        &self,
        env: Environment,
        device_id: &str,
        token: &TokenResponse,
        promote: Option<KeyPair>,
    ) -> Result<(String, PendingKey), AuthError> {
        let now = self.now();
        self.check_token_expiry(token, now);

        let mut st = self.state.lock_or_panic();
        let (promoted, had_pending) = {
            let slot = st.env_mut(env);
            let record = slot
                .identity
                .as_mut()
                .filter(|r| r.device_id == device_id)
                .ok_or_else(|| AuthError::NotProvisioned(format!("the identity of {env} changed during SignIn")))?;

            record.last_accessed = now;
            record.key_validity_remaining_days = Self::key_validity(token);
            slot.services_token = Some(Self::services_token(token));

            let had_pending = slot.pending_identity_keys.take().is_some();
            let promoted = promote.is_some();
            if let Some(pair) = promote {
                slot.identity_keys = Some(pair);
            }
            (promoted, had_pending)
        };

        let pending = if promoted {
            self.persist_identity(&st, env)?;
            PendingKey::Promoted
        } else {
            self.persist_identity_record(&st, env)?;
            if had_pending {
                self.persist_pending_identity(&st, env)?;
                PendingKey::Dropped
            } else {
                PendingKey::None
            }
        };

        Ok((token.access_token.clone(), pending))
    }
}

/// What a `SignIn` did with a pending rotation key (the field of its line).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PendingKey {
    /// There was none.
    None,
    /// It signed in: the key rotation is complete.
    Promoted,
    /// The current key signed in: the server never took the pending key, which is dropped.
    Dropped,
}
