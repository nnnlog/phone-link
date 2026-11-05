//! Identity key rotation.

use super::{AuthCallFailure, RefreshError};
use crate::auth::crypto::KeyPair;
use crate::auth::dcg_rest::RestError;
use crate::auth::store::IdentityRecord;
use crate::auth::{AuthError, AuthService, crypto};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::{Environment, TraceContext};
use std::time::SystemTime;

impl AuthService {
    /// Due when `NextKeyRotation < now`, or — once the server reported a non-negative
    /// `keyValidRemainingDays` — when that value is `≤ 182` instead.
    pub(super) fn key_rotation_due(&self, record: &IdentityRecord, now: SystemTime) -> bool {
        if record.key_validity_remaining_days >= 0 {
            record.key_validity_remaining_days <= self.config.auth.key_valid_remaining_days_threshold
        } else {
            record.next_key_rotation < now
        }
    }

    /// The key-rotation check after a successful `SignIn`. A rejected identity
    /// ([`RefreshError::IdentityLost`]) or an unlinked device fails the refresh; any other failure
    /// is retried after the retry interval.
    pub(super) async fn rotate_identity_key_if_due(&self, env: Environment, msa: &str) -> Result<(), RefreshError> {
        let due = {
            let st = self.state.lock_or_panic();
            st.env(env).identity.as_ref().is_some_and(|r| self.key_rotation_due(r, self.now()))
        };

        if !due {
            return Ok(());
        }

        match self.rotate_identity_key(env, msa).await {
            Ok(()) => {
                tracing::info!(%env, "identity key rotated");
                Ok(())
            }
            Err(e @ (RefreshError::IdentityLost | RefreshError::Failed(AuthError::UnlinkedDevice))) => Err(e),
            Err(RefreshError::Failed(e)) => {
                // A rotation that got no reply keeps its incoming key until a `SignIn` shows which key
                // the server holds.
                let is_incoming_key_pending = self.state.lock_or_panic().env(env).pending_identity_keys.is_some();
                tracing::warn!(
                    %env,
                    error = %e,
                    is_incoming_key_pending,
                    retry_secs = self.config.auth.identity_key_rotation_retry.as_secs(),
                    "identity key rotation failed; tried again after the retry interval"
                );
                Ok(())
            }
        }
    }

    /// `GenerateNonce(deviceId)`; a new key + `CN=<deviceId>`; the nonce signed with the current and
    /// with the new certificate; `RotateKeys`; `InvalidNonce` is retried. The new key + certificate is
    /// persisted as the pending rotation **before** `RotateKeys`, so a crash after the server took it
    /// leaves the key the server now expects on disk. Success promotes it (the old key is deleted,
    /// `NextKeyRotation = now + 182.5 d`); a failure the server answered drops it
    /// (`NextKeyRotation = now + 1 d`). A request that got no reply may have been applied, so its key
    /// stays pending until the next `SignIn` shows which key the server holds.
    pub(super) async fn rotate_identity_key(&self, env: Environment, msa: &str) -> Result<(), RefreshError> {
        let (device_id, current) = {
            let st = self.state.lock_or_panic();
            let slot = st.env(env);
            match (&slot.identity, &slot.identity_keys) {
                (Some(r), Some(p)) => (r.device_id.clone(), p.clone()),
                _ => return Err(AuthError::NotProvisioned(format!("no identity in {env}")).into()),
            }
        };

        let incoming = KeyPair::generate(&device_id, self.now()).map_err(AuthError::from)?;
        {
            let mut st = self.state.lock_or_panic();
            st.env_mut(env).pending_identity_keys = Some(incoming.clone());
            self.persist_pending_identity(&st, env)?;
        }

        let outcome = self.rotate_keys_with(env, msa, &device_id, &current, &incoming).await;
        let now = self.now();

        let failure = {
            let mut st = self.state.lock_or_panic();
            let slot = st.env_mut(env);
            let Some(record) = slot.identity.as_mut().filter(|r| r.device_id == device_id) else {
                return Err(
                    AuthError::NotProvisioned(format!("the identity of {env} changed during RotateKeys")).into()
                );
            };
            record.last_accessed = now;
            match outcome {
                Ok(()) => {
                    record.next_key_rotation = now + self.config.auth.identity_key_rotation;
                    slot.identity_keys = Some(incoming);
                    slot.pending_identity_keys = None;
                    self.persist_identity(&st, env)?;
                    return Ok(());
                }
                Err(failure) => {
                    record.next_key_rotation = now + self.config.auth.identity_key_rotation_retry;
                    // No reply: the incoming key stays pending until a `SignIn` shows which key the
                    // server holds.
                    if !matches!(failure, AuthCallFailure::Rest(RestError::Transport { .. })) {
                        slot.pending_identity_keys = None;
                    }
                    self.persist_identity_record(&st, env)?;
                    self.persist_pending_identity(&st, env)?;
                    failure
                }
            }
        };

        Err(self.refresh_failure(env, failure))
    }

    /// `GenerateNonce` + `RotateKeys` with the current and the incoming key; `InvalidNonce` is
    /// retried with a fresh nonce up to `auth_recover_retries` times.
    pub(super) async fn rotate_keys_with(
        &self,
        env: Environment,
        msa: &str,
        device_id: &str,
        current: &KeyPair,
        incoming: &KeyPair,
    ) -> Result<(), AuthCallFailure> {
        let retries = self.config.auth.auth_recover_retries;
        let mut attempt = 0;

        loop {
            let scenario = TraceContext::new_root(Vec::new());
            let nonce = self
                .rest
                .generate_nonce(env, msa, device_id, &scenario.child())
                .await
                .map_err(AuthCallFailure::Rest)?;

            let now = self.now();
            let old_jwt = crypto::nonce_jwt(current, &nonce, now)?;
            let new_jwt = crypto::nonce_jwt(incoming, &nonce, now)?;

            match self.rest.rotate_keys(env, msa, &old_jwt, &new_jwt, &scenario.child()).await {
                Ok(()) => return Ok(()),
                Err(e) if attempt < retries && e.is_invalid_nonce() => {
                    attempt += 1;
                    tracing::warn!(%env, attempt, "RotateKeys InvalidNonce; retrying with a new nonce");
                }
                Err(e) => return Err(AuthCallFailure::Rest(e)),
            }
        }
    }
}
