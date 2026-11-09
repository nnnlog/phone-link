//! The services token the relay and REST calls carry.

use super::{AuthCallFailure, RefreshError};
use crate::auth::crypto::KeyPair;
use crate::auth::dcg_rest::TokenResponse;
use crate::auth::store::{AccountInfo, AccountState};
use crate::auth::{AuthError, AuthService, State, crypto};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::{Environment, TraceContext};

impl AuthService {
    /// The services token refresh policy: the cached token of `env` unless missing, `force`d, or
    /// `epochExpirationTime − leeway < now`; then `GenerateNonce` + `SignIn` with the existing
    /// identity certificate, followed by the key-rotation check.
    ///
    /// A stored identity serves, expired or not, and a lost one is created anew in place: the request
    /// returns the new identity's token, and the account is enrolled again under it before the token
    /// is returned.
    pub async fn services_token_for(&self, env: Environment, force: bool) -> Result<String, AuthError> {
        let (token, replaced) = self.refresh_services_token(env, force).await?;

        if replaced {
            // Boxed: the enrollment asks for the services token again (then cached).
            Box::pin(self.enroll_replaced_identity(env)).await;
        }
        Ok(token)
    }

    /// The refresh under the token lock; `true` with the token when the identity was created anew.
    async fn refresh_services_token(&self, env: Environment, force: bool) -> Result<(String, bool), AuthError> {
        let _guard = self.token_lock.lock().await;

        {
            let st = self.state.lock_or_panic();
            self.check_token_identity_locked(&st, env)?;

            if !force {
                if let Some(token) = &st.env(env).services_token {
                    let fresh = token
                        .expires_at
                        .checked_sub(self.config.auth.token_refresh_leeway)
                        .is_some_and(|limit| limit >= self.now());
                    if fresh {
                        return Ok((token.access_token.clone(), false));
                    }
                }
            }
        }

        let msa = self.msa_token(env).await?;
        match self.sign_in_identity(env, &msa).await {
            Ok(token) => Ok((token, false)),
            Err(RefreshError::Failed(AuthError::UnlinkedDevice)) => {
                // The service unlinked this device: auto re-link it once per run.
                let token = self.recover_unlinked(env, &msa).await?;
                Ok((token, false))
            }
            Err(RefreshError::Failed(e)) => Err(e),
            Err(RefreshError::IdentityLost) => self.recreate_identity(env, &msa).await.map(|token| (token, true)),
        }
    }

    /// Auto re-links an unlinked device once per run: creates a new identity for the signed-in
    /// account and enrolls the account under it, so a de-provisioned device heals on its own. The
    /// recovery is bounded to one attempt per run; a server refusal (or no signed-in account) keeps
    /// the unlinked state, which the caller reports.
    async fn recover_unlinked(&self, env: Environment, msa: &str) -> Result<String, AuthError> {
        if self
            .recovery_attempted
            .compare_exchange(false, true, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst)
            .is_err()
        {
            return Err(AuthError::UnlinkedDevice);
        }

        let account = self.state.lock_or_panic().account.clone().ok_or(AuthError::UnlinkedDevice)?;

        let new = match self.request_new_identity(env, msa).await? {
            Ok(new) => new,
            Err(e) if e.is_unlinked() => return Err(self.unlinked(env)),
            Err(e) => return Err(self.msa_call_failure(env, e)),
        };

        let token = new.token.access_token.clone();
        self.store_new_identity(env, new)?;
        {
            let mut st = self.state.lock_or_panic();
            let device_id = st.env(env).identity.as_ref().map(|i| i.device_id.clone());
            st.env_mut(env).account_id = Some(account.account_id.clone());
            let info = st.account_info.entry(account.account_id.clone()).or_insert_with(|| AccountInfo::new(&account));
            info.state = AccountState::Enrolled;
            info.environment = Some(env);
            info.dcg_client_id = device_id;
            info.last_update_time = Some(self.now());
            self.persist_account_info(&st)?;
        }

        tracing::warn!(%env, "device was unlinked; a new identity was created and enrolled automatically");
        Ok(token)
    }

    /// The identity a token request needs: a stored one with an account id, expired or not — its key
    /// may be missing.
    fn check_token_identity_locked(&self, st: &State, env: Environment) -> Result<(), AuthError> {
        let slot = st.env(env);
        if slot.identity.is_none() {
            return Err(AuthError::NotProvisioned(format!("no identity in {env}")));
        }
        if slot.account_id.is_none() {
            return Err(AuthError::NotProvisioned(format!("no account id for the identity of {env}")));
        }
        Ok(())
    }

    /// The refresh policy's `GenerateNonce` + `SignIn` with the existing identity certificate. When
    /// `SignIn` rejects it (`InvalidCertificate`) or the identity key is missing while a key rotation
    /// is pending — the server took `RotateKeys` but the promotion never happened (the persisted state
    /// keeps, briefly, the outgoing key during rotation) — `SignIn` is retried with the pending key,
    /// which is promoted on success; only when that key is rejected too is the identity lost.
    pub(super) async fn sign_in_identity(&self, env: Environment, msa: &str) -> Result<String, RefreshError> {
        let (device_id, current, pending) = {
            let st = self.state.lock_or_panic();
            let slot = st.env(env);
            match &slot.identity {
                Some(r) => (r.device_id.clone(), slot.identity_keys.clone(), slot.pending_identity_keys.clone()),
                None => return Err(AuthError::NotProvisioned(format!("no identity in {env}")).into()),
            }
        };

        let first = self.sign_in_with(env, msa, &device_id, current.as_ref()).await;
        let (token, promote) = match (first, pending) {
            (Ok(token), _) => (token, None),
            (Err(failure), Some(pending)) if is_key_rejected(&failure) => {
                tracing::warn!(%env, "SignIn found no usable identity key during a key rotation; signing in with the incoming key");
                match self.sign_in_with(env, msa, &device_id, Some(&pending)).await {
                    Ok(token) => (token, Some(pending)),
                    Err(failure) => return Err(self.refresh_failure(env, failure)),
                }
            }
            (Err(failure), _) => return Err(self.refresh_failure(env, failure)),
        };

        if !token.device_id.eq_ignore_ascii_case(&device_id) {
            return Err(AuthError::Protocol("SignIn deviceId echo differs from the stored deviceId".into()).into());
        }

        let (access, pending_key_outcome) = self.store_signed_in(env, &device_id, &token, promote)?;
        tracing::info!(%env, ?pending_key_outcome, "services token refreshed with SignIn");

        self.rotate_identity_key_if_due(env, msa).await?;
        Ok(access)
    }

    /// What a failed `SignIn` or `RotateKeys` exchange means for the refresh: a rejected certificate or
    /// a missing key loses the identity; a 403 + 13 unlinks the device; the rest maps generically.
    pub(super) fn refresh_failure(&self, env: Environment, failure: AuthCallFailure) -> RefreshError {
        match failure {
            AuthCallFailure::Rest(e) if e.is_invalid_certificate() => RefreshError::IdentityLost,
            AuthCallFailure::KeyMissing => RefreshError::IdentityLost,
            AuthCallFailure::Rest(e) if e.is_unlinked() => RefreshError::Failed(self.unlinked(env)),
            AuthCallFailure::Rest(e) => RefreshError::Failed(self.msa_call_failure(env, e)),
            AuthCallFailure::Local(e) => RefreshError::Failed(e),
        }
    }

    /// `GenerateNonce(deviceId)` → nonce JWT signed with `pair` → `SignIn`; `InvalidNonce` is retried
    /// with a fresh nonce up to `auth_recover_retries` times. Without a `pair` the nonce is requested
    /// and nothing can sign it ([`AuthCallFailure::KeyMissing`]).
    pub(super) async fn sign_in_with(
        &self,
        env: Environment,
        msa: &str,
        device_id: &str,
        pair: Option<&KeyPair>,
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

            let pair = pair.ok_or(AuthCallFailure::KeyMissing)?;
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

/// The service rejected the key `SignIn` proved, or there was no key to prove.
fn is_key_rejected(failure: &AuthCallFailure) -> bool {
    match failure {
        AuthCallFailure::Rest(e) => e.is_invalid_certificate(),
        AuthCallFailure::KeyMissing => true,
        AuthCallFailure::Local(_) => false,
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
