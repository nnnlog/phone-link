//! An identity created anew in place, and its account enrolled again — the rule for a lost identity.
//!
//! When `SignIn` or the key rotation after it rejects the identity certificate, or the identity's key
//! and certificate are missing, the identity is not cleared: a new one (new `dcgClientId`, new key) is
//! created with `CreateIdentity` for the signed-in account, under the token lock, and replaces it. The
//! relationships bound to the old identity go with it. The account its record bound to the old
//! identity is then enrolled again under the new one (a full `EnrollDevice`, the silent path's token
//! and an empty `pop-device-key`), and the peers are synchronized for it.

use super::NewIdentity;
use crate::auth::trust::trust_client_id;
use crate::auth::{AuthError, AuthService, FetchMode};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::Environment;

impl AuthService {
    /// Creates the lost identity of `env` anew for the signed-in account and returns the new
    /// identity's services token. Without a signed-in account whose id the environment stores, or when
    /// `CreateIdentity` fails, the lost identity is kept and the request fails; an unlinked device
    /// clears it.
    pub(super) async fn recreate_identity(&self, env: Environment, msa: &str) -> Result<String, AuthError> {
        let old = {
            let st = self.state.lock_or_panic();
            let slot = st.env(env);
            let signed_in = st.account.as_ref().map(|a| a.account_id.as_str());

            match (&slot.identity, slot.account_id.as_deref()) {
                (Some(record), Some(stored)) if Some(stored) == signed_in => record.device_id.clone(),
                _ => {
                    return Err(AuthError::NotProvisioned(format!(
                        "the identity of {env} was lost and no signed-in account can create it anew"
                    )));
                }
            }
        };

        tracing::warn!(%env, device_id = old, "identity lost; creating it anew in place");
        let new = match self.request_new_identity(env, msa).await? {
            Ok(new) => new,
            Err(e) if e.is_unlinked() => return Err(self.unlinked(env)),
            Err(e) => return Err(self.msa_call_failure(env, e)),
        };

        let token = new.token.access_token.clone();
        self.replace_identity(env, &old, new)?;
        Ok(token)
    }

    /// The new identity replaces `old`: the relationships bound to `old`, its trust certificate and
    /// its device-list entry go first, then the new identity is stored. An interrupted replacement
    /// leaves the lost identity without them, which the next refresh replaces again.
    fn replace_identity(&self, env: Environment, old: &str, new: NewIdentity) -> Result<(), AuthError> {
        {
            let mut st = self.state.lock_or_panic();
            let self_id = trust_client_id(old);
            st.trust.remove_where(|r| r.self_client_id.eq_ignore_ascii_case(&self_id));
            st.env_mut(env).trust_keys = None;
            st.own_entry = None;

            self.persist_trust(&st)?;
            self.persist_trust_keys(&st, env)?;
            self.persist_peer_cache(&st)?;
        }

        self.store_new_identity(env, new)
    }

    /// After an identity of `env` was created anew: the account is enrolled again under it and, once
    /// enrolled, its peers are synchronized for it in the background. A failure is logged; the next
    /// provisioning or trust synchronization enrolls the account again.
    pub(super) async fn enroll_replaced_identity(&self, env: Environment) {
        match self.reenroll_replaced(env).await {
            Ok(true) => self.spawn_peer_refresh(),
            Ok(false) => tracing::debug!(%env, "no account is bound to the replaced identity"),
            Err(e) => tracing::warn!(
                %env,
                error = %e,
                "the account was not enrolled under the new identity; enrolled again at the next provisioning or trust synchronization"
            ),
        }
    }

    /// Enrolls the signed-in account again when its record in `env` is bound to another identity than
    /// the one `env` holds for it now (one created anew in place). Returns whether it enrolled; nothing
    /// is sent when the record is bound to the current identity or `env` holds none for the account.
    pub(crate) async fn reenroll_replaced(&self, env: Environment) -> Result<bool, AuthError> {
        let account = {
            let st = self.state.lock_or_panic();
            let Some(account) = st.account.clone() else { return Ok(false) };
            let slot = st.env(env);

            let current = match (&slot.identity, slot.account_id.as_deref()) {
                (Some(record), Some(stored)) if stored == account.account_id => record.device_id.clone(),
                _ => return Ok(false),
            };
            let replaced = st.account_info.get(&account.account_id).is_some_and(|info| {
                info.environment == Some(env)
                    && info.dcg_client_id.as_deref().is_some_and(|id| !id.eq_ignore_ascii_case(&current))
            });

            if !replaced {
                return Ok(false);
            }
            account
        };

        tracing::info!(%env, "enrolling the account again under its new identity");
        let msa = self.msa_token(env).await?;
        self.enroll(env, &account, &msa, "").await?;
        Ok(true)
    }

    /// A `Default` trust synchronization in the background: the enrollment reset the peers' refresh
    /// time, so it fetches the device list for the new identity.
    fn spawn_peer_refresh(&self) {
        let weak = self.weak.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    let Some(this) = weak.upgrade() else { return };
                    if let Err(e) = this.sync_trust(FetchMode::Default).await {
                        tracing::warn!(error = %e, "peer refresh after the re-enrollment failed");
                    }
                });
            }
            Err(_) => tracing::warn!("peer refresh after the re-enrollment skipped; no async runtime"),
        }
    }
}
