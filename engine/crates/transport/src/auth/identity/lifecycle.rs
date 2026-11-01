//! The identity's lifecycle: storing, clearing and deprovisioning it.

use crate::auth::dcg_rest::RestError;
use crate::auth::trust::trust_client_id;
use crate::auth::{AccountRemoved, AuthError, AuthService, State};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::Environment;

/// What deprovisions an account (the field of its `account deprovisioned` line).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Deprovision {
    /// The user signed out.
    SignOut,
    /// Another Microsoft account signed in.
    AccountSwitch,
    /// Account enumeration found the record of another account (one account at a time).
    OtherAccount,
    /// Account enumeration found the account's identity no longer active.
    IdentityInactive,
    /// The service reported the device unlinked.
    Unlinked,
    /// The client-info refresh's `SetDeviceInfo` was refused as unauthorized.
    Unauthorized,
    /// The trust synchronization found the own device-list entry gone and could not restore it.
    EnrollmentExpired,
}

impl AuthService {
    /// Clears the identity of `env` (with a pending rotation key) and every relationship whose
    /// `self_client_id == trust_<deviceId>`, with its trust certificate and cached token.
    pub(crate) fn clear_identity_locked(&self, st: &mut State, env: Environment) -> Result<(), AuthError> {
        if let Some(record) = st.env_mut(env).identity.take() {
            let self_id = trust_client_id(&record.device_id);
            st.trust.remove_where(|r| r.self_client_id.eq_ignore_ascii_case(&self_id));
        }

        let slot = st.env_mut(env);
        slot.identity_keys = None;
        slot.pending_identity_keys = None;
        slot.trust_keys = None;
        slot.services_token = None;

        self.persist_identity(st, env)?;
        self.persist_trust_keys(st, env)?;
        self.persist_trust(st)
    }

    /// Deprovisioning an account: its relationships (by CID), its trust certificate and peer cache
    /// go; when no partner remains in the environment the identity is cleared too.
    pub(crate) fn deprovision_locked(
        &self,
        st: &mut State,
        account_id: &str,
        trigger: Deprovision,
    ) -> Result<(), AuthError> {
        let Some(info) = st.account_info.remove(account_id) else {
            return Ok(());
        };

        let cid = info.msa_cid.clone();
        st.trust.remove_where(|r| r.msa_cid.as_deref() == Some(cid.as_str()));
        st.peer_cache.remove(account_id);
        st.own_entry = None;

        if let Some(env) = info.environment {
            st.env_mut(env).trust_keys = None;
            self.persist_trust_keys(st, env)?;
            let partner_remains = st.trust.records().iter().any(|r| r.dcg_client_id(env).is_some());
            if !partner_remains {
                self.clear_identity_locked(st, env)?;
            }
        }

        self.persist_trust(st)?;
        self.persist_peer_cache(st)?;
        self.persist_account_info(st)?;

        tracing::info!(account = account_id, trigger = ?trigger, "account deprovisioned");
        // No receiver is not an error.
        let _ = self.account_events.send(AccountRemoved { account_id: account_id.to_string() });
        Ok(())
    }

    /// The device was unlinked: the identity of `env` is cleared and every account deprovisioned.
    /// Returns the error to report.
    pub(crate) fn unlinked(&self, env: Environment) -> AuthError {
        let mut st = self.state.lock_or_panic();
        let outcome = self.clear_identity_locked(&mut st, env).and_then(|()| {
            let ids: Vec<String> = st.account_info.keys().cloned().collect();
            ids.iter().try_for_each(|id| self.deprovision_locked(&mut st, id, Deprovision::Unlinked))
        });

        if let Err(e) = outcome {
            tracing::error!(error = %e, "unlinked state not persisted");
        }
        AuthError::UnlinkedDevice
    }

    /// The `/Auth` error rules: `InvalidCertificate` → identity cleared (`InvalidIdentity`);
    /// 403 + 13 → unlinked; a 401 drops the cached MSA token; the rest maps generically.
    pub(super) fn auth_failure(&self, env: Environment, e: RestError) -> AuthError {
        if e.is_invalid_certificate() {
            let mut st = self.state.lock_or_panic();
            if let Err(err) = self.clear_identity_locked(&mut st, env) {
                tracing::error!(error = %err, "cleared invalid identity not persisted");
            }
            return AuthError::InvalidIdentity;
        }

        if e.is_unlinked() {
            return self.unlinked(env);
        }

        self.msa_call_failure(env, e)
    }
}
