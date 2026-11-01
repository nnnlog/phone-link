//! The startup and provisioning sequence: sign-in, enrollment, client info.

use super::Deprovision;
use super::{Discovery, metadata_hash, since};
use crate::auth::crypto::{KeyPair, ParsedCertificate};
use crate::auth::dcg_rest::{DeviceInfoBody, DeviceMetadata, EnrollDeviceResponse, RestError, TokenResponse};
use crate::auth::store::{AccountInfo, AccountState, IdentityRecord, LocalAccount};
use crate::auth::trust::{CryptoTrustType, new_relationship, trust_client_id};
use crate::auth::{AccountTrustCause, AuthError, AuthService, Provisioned, ServicesToken, State, crypto};
use phonelink_core::config::display_name_from_host;
use phonelink_core::sync::MutexExt;
use phonelink_core::types::{DcgClient, DeviceCertificates, Environment, TraceContext, new_guid_lower};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

impl AuthService {
    /// Runs the startup and provisioning sequence, steps 1–7: account enumeration over the loaded
    /// state (1); an `Enrolled` account then skips to step 7 without a network call; otherwise
    /// environment discovery and snap (2), the MSA token (3), the account id (4), the identity (5)
    /// and the enrollment (6), then the client-info refresh (7). Peers (step 8) are
    /// [`AuthService::sync_trust`].
    pub async fn ensure_provisioned(&self) -> Result<Provisioned, AuthError> {
        let _op = self.provision_lock.lock().await;
        let account = self.local_account().ok_or_else(|| AuthError::UserInteractionRequired("not signed in".into()))?;
        self.enumerate_accounts(&account)?;

        let enrolled_env = {
            let st = self.state.lock_or_panic();
            st.account_info
                .get(&account.account_id)
                .filter(|i| i.state == AccountState::Enrolled)
                .and_then(|i| i.environment)
        };

        let env = match enrolled_env {
            Some(env) => {
                let mut st = self.state.lock_or_panic();
                if st.saved_env != Some(env) {
                    st.saved_env = Some(env);
                    self.persist_saved_env(&st)?;
                }
                env
            }
            None => {
                let env = self.discover_and_snap(&account).await?;
                // Step 3: PopToken = (account, token, pairingToken = "") on the silent path.
                let msa = self.msa_token(env).await?;
                self.store_account_id(env, &account)?;
                self.ensure_identity(env, &account, &msa).await?;
                self.enroll(env, &account, &msa, "").await?;
                env
            }
        };

        self.client_info_refresh(env, &account).await?;
        self.provisioned(env, &account)
    }

    /// Step 7 on an options change (capabilities, display name): `SetDeviceInfo` when due.
    pub async fn refresh_client_info(&self) -> Result<(), AuthError> {
        let _op = self.provision_lock.lock().await;
        let (account, env) = self.enrolled()?;
        self.client_info_refresh(env, &account).await
    }

    pub(super) fn provisioned(&self, env: Environment, account: &LocalAccount) -> Result<Provisioned, AuthError> {
        let st = self.state.lock_or_panic();
        let own =
            st.env(env).identity.as_ref().ok_or_else(|| AuthError::NotProvisioned(format!("no identity in {env}")))?;
        Ok(Provisioned { env, own: DcgClient::new(own.device_id.clone(), env), account_id: account.account_id.clone() })
    }

    /// The signed-in account and its environment when it is `Enrolled`.
    pub(crate) fn enrolled(&self) -> Result<(LocalAccount, Environment), AuthError> {
        let st = self.state.lock_or_panic();
        let account = st.account.clone().ok_or_else(|| AuthError::NotProvisioned("not signed in".into()))?;
        let info = st
            .account_info
            .get(&account.account_id)
            .filter(|i| i.state == AccountState::Enrolled)
            .ok_or_else(|| AuthError::NotProvisioned("the account is not enrolled".into()))?;
        let env = info
            .environment
            .ok_or_else(|| AuthError::NotProvisioned("the enrolled account has no environment".into()))?;
        Ok((account, env))
    }

    /// Account enumeration: an account record of another account is deprovisioned first (one
    /// account at a time), and so is any account whose identity is no longer active.
    pub(super) fn enumerate_accounts(&self, account: &LocalAccount) -> Result<(), AuthError> {
        let now = self.now();
        let mut st = self.state.lock_or_panic();
        let ids: Vec<String> =
            st.account_info.iter().filter(|(_, i)| i.is_provisioned()).map(|(id, _)| id.clone()).collect();

        for id in ids {
            if id != account.account_id {
                self.deprovision_locked(&mut st, &id, Deprovision::OtherAccount)?;
                continue;
            }

            let env = st.account_info[&id].environment;
            let active = env.is_some_and(|env| self.identity_active_locked(&st, env, account, now));
            if !active {
                self.deprovision_locked(&mut st, &id, Deprovision::IdentityInactive)?;
            }
        }

        Ok(())
    }

    /// An active identity: stored with its key and certificate, `last_accessed` within the identity
    /// expiration, account id stored and matching.
    pub(super) fn identity_active_locked(
        &self,
        st: &State,
        env: Environment,
        account: &LocalAccount,
        now: SystemTime,
    ) -> bool {
        let slot = st.env(env);
        match (&slot.identity, &slot.identity_keys, &slot.account_id) {
            (Some(record), Some(_), Some(account_id)) => {
                *account_id == account.account_id && record.last_accessed + self.config.auth.identity_expiration > now
            }
            _ => false,
        }
    }

    /// Step 2: `GET /EnvironmentDiscovery/UserServiceEnvironment` with the prod-scope token and the
    /// snap rules.
    pub(super) async fn discover_and_snap(&self, account: &LocalAccount) -> Result<Environment, AuthError> {
        let msa_prod = self.msa_token(Environment::Prod).await?;
        let trace = TraceContext::new_root(Vec::new());
        let discovery = self.discovery_outcome(self.rest.user_service_environment(&msa_prod, &trace).await)?;

        let now = self.now();
        let mut st = self.state.lock_or_panic();
        let env = self.snapped_environment_locked(&st, discovery, account, now);
        check_account_environment(&st, account, env)?;

        if st.saved_env != Some(env) {
            st.saved_env = Some(env);
            self.persist_saved_env(&st)?;
        }
        Ok(env)
    }

    /// The discovery result of the `UserServiceEnvironment` reply: an unknown or missing name, or a
    /// remote failure, is `Defaulted`; an unauthorized reply invalidates the prod-scope token.
    fn discovery_outcome(&self, reply: Result<Option<String>, RestError>) -> Result<Discovery, AuthError> {
        Ok(match reply {
            Ok(Some(name)) => match name.as_str() {
                "Production" => Discovery::Success(Environment::Prod),
                "Beta" => Discovery::Success(Environment::Beta),
                other => {
                    tracing::warn!(
                        raw_service_environment = other,
                        "environment discovery Defaulted; unknown serviceEnvironment"
                    );
                    Discovery::Defaulted
                }
            },
            Ok(None) => {
                tracing::warn!("environment discovery Defaulted; no serviceEnvironment");
                Discovery::Defaulted
            }
            Err(e) if e.is_unauthorized() => {
                self.msa.invalidate(Environment::Prod);
                return Err(AuthError::Unauthorized(e.to_string()));
            }
            Err(e @ RestError::Local { .. }) => return Err(AuthError::from_rest(e)),
            Err(e) => {
                tracing::warn!(error = %e, "environment discovery failed; Defaulted");
                Discovery::Defaulted
            }
        })
    }

    /// The snap rule: a discovered environment wins; a defaulted one keeps the saved environment
    /// while its identity is active and a trust is unexpired, else Prod.
    fn snapped_environment_locked(
        &self,
        st: &State,
        discovery: Discovery,
        account: &LocalAccount,
        now: SystemTime,
    ) -> Environment {
        match discovery {
            Discovery::Success(env) => env,
            Discovery::Defaulted => match st.saved_env {
                Some(saved)
                    if self.identity_active_locked(st, saved, account, now)
                        && st.trust.has_unexpired(now, &self.policy) =>
                {
                    saved
                }
                _ => Environment::Prod,
            },
        }
    }

    /// Step 4: the account id of the environment. A different stored account is an account switch,
    /// which clears that environment's identity first.
    pub(super) fn store_account_id(&self, env: Environment, account: &LocalAccount) -> Result<(), AuthError> {
        let mut st = self.state.lock_or_panic();
        let stored = st.env(env).account_id.clone();

        if stored.as_deref() == Some(account.account_id.as_str()) {
            return Ok(());
        }

        if stored.is_some() {
            if st.env(env).identity.is_some() {
                tracing::info!(%env, "identity cleared; the environment's stored account id is another account's");
            }
            self.clear_identity_locked(&mut st, env)?;
        }

        st.env_mut(env).account_id = Some(account.account_id.clone());
        self.persist_account_id(&st, env)
    }

    /// Step 5: reuse an active identity, else clear it and run `CreateIdentity`; the account record
    /// becomes `Provisioned`.
    pub(super) async fn ensure_identity(
        &self,
        env: Environment,
        account: &LocalAccount,
        msa: &str,
    ) -> Result<(), AuthError> {
        let active = {
            let st = self.state.lock_or_panic();
            self.identity_active_locked(&st, env, account, self.now())
        };

        if !active {
            {
                let mut st = self.state.lock_or_panic();
                if st.env(env).identity.is_some() {
                    self.clear_identity_locked(&mut st, env)?;
                }
            }
            self.create_identity(env, msa).await?;
        }

        let mut st = self.state.lock_or_panic();
        let device_id = st
            .env(env)
            .identity
            .as_ref()
            .map(|i| i.device_id.clone())
            .ok_or_else(|| AuthError::NotProvisioned(format!("no identity in {env}")))?;

        let scope = self.msa.scope(env)?;
        let info = st.account_info.entry(account.account_id.clone()).or_insert_with(|| AccountInfo::new(account));
        if info.state != AccountState::Enrolled {
            info.state = AccountState::Provisioned;
        }
        info.environment = Some(env);
        info.dcg_client_id = Some(device_id);
        info.msa_scope = Some(scope);
        self.persist_account_info(&st)
    }

    /// `GenerateNonce(newGuid)` → new P-384 key + `CN=<newGuid>` → nonce JWT → `CreateIdentity`,
    /// retried up to `auth_recover_retries` times on a device-id collision (uncategorized 403, new
    /// GUID) or `InvalidNonce` (new nonce).
    pub(super) async fn create_identity(&self, env: Environment, msa: &str) -> Result<(), AuthError> {
        let retries = self.config.auth.auth_recover_retries;
        let mut attempt = 0;
        let mut device_id = new_guid_lower();

        loop {
            let scenario = TraceContext::new_root(Vec::new());
            let nonce = self
                .rest
                .generate_nonce(env, msa, &device_id, &scenario.child())
                .await
                .map_err(|e| self.auth_failure(env, e))?;

            let now = self.now();
            let pair = KeyPair::generate(&device_id, now)?;
            let jwt = crypto::nonce_jwt(&pair, &nonce, now)?;

            match self.rest.create_identity(env, msa, &jwt, &scenario.child()).await {
                Ok(token) => return self.store_new_identity(env, &device_id, pair, token),
                Err(e) if attempt < retries && e.is_forbidden_uncategorized() => {
                    attempt += 1;
                    tracing::warn!(%env, attempt, error = %e, "CreateIdentity device-id collision; retrying with a new GUID");
                    device_id = new_guid_lower();
                }
                Err(e) if attempt < retries && e.is_invalid_nonce() => {
                    attempt += 1;
                    tracing::warn!(%env, attempt, "CreateIdentity InvalidNonce; retrying with a new nonce");
                }
                Err(e) => return Err(self.auth_failure(env, e)),
            }
        }
    }

    pub(super) fn check_token_expiry(&self, token: &TokenResponse, now: SystemTime) {
        let exp = UNIX_EPOCH + Duration::from_secs(token.epoch_expiration_time.max(0) as u64);
        let ceiling = now + phonelink_core::constants::lifetime::SERVICES_TOKEN_EXPECTED_LIFETIME;

        if exp <= now || exp > ceiling {
            tracing::warn!(
                epoch = token.epoch_expiration_time,
                "services token expiry outside (now, now + 24 h]; kept"
            );
        }
    }

    pub(super) fn services_token(token: &TokenResponse) -> ServicesToken {
        ServicesToken {
            access_token: token.access_token.clone(),
            device_id: token.device_id.clone(),
            expires_at: UNIX_EPOCH + Duration::from_secs(token.epoch_expiration_time.max(0) as u64),
        }
    }

    pub(super) fn key_validity(token: &TokenResponse) -> i64 {
        token.key_valid_remaining_days.filter(|d| *d >= 0).unwrap_or(-1)
    }

    /// Persists a new identity: the GUID the PC generated (the response's `deviceId` is kept only
    /// on the cached token), `NextKeyRotation = now + 182.5 d`, the cached token.
    pub(super) fn store_new_identity(
        &self,
        env: Environment,
        device_id: &str,
        pair: KeyPair,
        token: TokenResponse,
    ) -> Result<(), AuthError> {
        let now = self.now();
        self.check_token_expiry(&token, now);

        let mut st = self.state.lock_or_panic();
        let record = IdentityRecord {
            device_id: device_id.to_string(),
            last_accessed: now,
            next_key_rotation: now + self.config.auth.identity_key_rotation,
            key_validity_remaining_days: Self::key_validity(&token),
            endpoint: self.config.endpoints.base(env).to_string(),
        };

        let slot = st.env_mut(env);
        slot.identity = Some(record);
        slot.identity_keys = Some(pair);
        slot.pending_identity_keys = None;
        slot.services_token = Some(Self::services_token(&token));

        self.persist_identity(&st, env)?;
        tracing::info!(%env, device_id, "identity created");
        Ok(())
    }

    /// Step 6: services token, trust certificate (reused or new), `EnrollDevice`; success persists a
    /// new trust certificate, turns `accountCert` into the `A2D` relationship and marks the account
    /// `Enrolled`. `Unauthorized` (any 401 status) is the repair path: `EnrollDevice` is retried
    /// once with a strong-auth (SAPI-scope) MSA token as `Authorization: Bearer`, `Dcg-Token`
    /// unchanged; a second 401 is `Unauthorized`.
    pub(super) async fn enroll(
        &self,
        env: Environment,
        account: &LocalAccount,
        msa: &str,
        pop_device_key: &str,
    ) -> Result<(), AuthError> {
        let services = self.services_token_for(env, false).await?;

        let (trust, is_new, body) = {
            let st = self.state.lock_or_panic();
            let (trust, is_new) = self.trust_pair_for_send(&st, env)?;
            let body = self.device_body(&st, &trust)?;
            (trust, is_new, body)
        };

        let response = match self.enroll_device(env, msa, &services, pop_device_key, &body).await {
            Err(e) if e.is_unauthorized() => {
                tracing::warn!(%env, error = %e, "EnrollDevice Unauthorized; retrying with a strong-auth MSA token");
                // The DCG-scope token was refused as the enroll bearer: later calls fetch a fresh one.
                self.msa.invalidate(env);
                let strong = self.strong_auth_token(env).await?;
                match self.enroll_device(env, &strong, &services, pop_device_key, &body).await {
                    Err(e) if e.is_unauthorized() => {
                        self.msa.invalidate_strong_auth(env);
                        return Err(AuthError::Unauthorized(e.to_string()));
                    }
                    second => second,
                }
            }
            first => first,
        };

        let response = match response {
            Ok(r) => r,
            Err(e) if e.is_unlinked() => return Err(self.unlinked(env)),
            Err(e) => return Err(self.msa_call_failure(env, e)),
        };

        let now = self.now();
        let mut st = self.state.lock_or_panic();
        if is_new {
            st.env_mut(env).trust_keys = Some(trust);
            self.persist_trust_keys(&st, env)?;
        }

        let cert = response
            .account_cert
            .as_deref()
            .ok_or(AccountTrustCause::NoAccountCert)
            .and_then(|b64| crypto::b64_std_decode(b64).map_err(AccountTrustCause::Certificate))
            .and_then(|der| crypto::parse_certificate(&der).map_err(AccountTrustCause::Certificate))
            .map_err(AuthError::AccountTrustSyncFailure)?;
        self.upsert_a2d_locked(&mut st, env, account, &cert, now)
            .map_err(|e| AuthError::AccountTrustSyncFailure(AccountTrustCause::Record(Box::new(e))))?;

        let device_id = st.env(env).identity.as_ref().map(|i| i.device_id.clone());
        let info = st.account_info.entry(account.account_id.clone()).or_insert_with(|| AccountInfo::new(account));
        info.state = AccountState::Enrolled;
        info.environment = Some(env);
        info.dcg_client_id = device_id;
        info.last_update_time = Some(now);
        info.last_attempt_update_time = Some(now);
        info.metadata_hash = Some(metadata_hash(&body.metadata));

        self.persist_account_info(&st)?;
        tracing::info!(%env, is_new_trust_certificate = is_new, "device enrolled");
        Ok(())
    }

    pub(super) async fn enroll_device(
        &self,
        env: Environment,
        bearer: &str,
        services: &str,
        pop_device_key: &str,
        body: &DeviceInfoBody,
    ) -> Result<EnrollDeviceResponse, RestError> {
        let trace = TraceContext::new_root(Vec::new());
        self.rest.enroll_device(env, bearer, services, pop_device_key, body, &trace).await
    }

    /// The strong-auth (SAPI-scope) MSA token of `env` for the enrollment's repair path, minted by the
    /// device-bound broker. The broker exchanges against the one signed-in session, so there is no separate
    /// account match; a declined or unavailable sign-in surfaces as [`MsaError::InteractionRequired`].
    pub(super) async fn strong_auth_token(&self, env: Environment) -> Result<String, AuthError> {
        Ok(self.msa.strong_auth_token(env).await?)
    }

    /// Creates or rotates the `A2D` relationship from the account certificate: `self_client_id =
    /// trust_<own id>`, `partner_client_id = trust_<CN>`, `msa_cid = CID`,
    /// `attributes[dcg_<env>_client_id] = CN`.
    pub(crate) fn upsert_a2d_locked(
        &self,
        st: &mut State,
        env: Environment,
        account: &LocalAccount,
        cert: &ParsedCertificate,
        now: SystemTime,
    ) -> Result<(), AuthError> {
        let account_dcg_id = cert
            .subject_cn
            .clone()
            .filter(|cn| !cn.is_empty())
            .ok_or(AuthError::AccountTrustSyncFailure(AccountTrustCause::NoCommonName))?;
        let own = st
            .env(env)
            .identity
            .as_ref()
            .map(|i| i.device_id.clone())
            .ok_or_else(|| AuthError::NotProvisioned(format!("no identity in {env}")))?;

        let partner_client_id = trust_client_id(&account_dcg_id);
        st.trust.prune(now, &self.policy);
        match st.trust.index_by_partner_client_id(&partner_client_id) {
            Some(index) => {
                st.trust.rotate_partner_certificate(index, cert, now, &self.policy)?;
                let r = st.trust.get_mut(index);
                if r.msa_cid.as_deref().is_none_or(str::is_empty) || r.trust_type == CryptoTrustType::Unknown {
                    r.msa_cid = Some(account.cid.clone());
                    r.trust_type = CryptoTrustType::A2D;
                }
            }
            None => {
                let record = new_relationship(
                    &trust_client_id(&own),
                    &partner_client_id,
                    &account.cid,
                    CryptoTrustType::A2D,
                    cert,
                    env,
                    &account_dcg_id,
                    now,
                    &self.policy,
                )?;
                st.trust.add(record, now, &self.policy)?;
            }
        }

        self.persist_trust(st)
    }

    /// Step 7: `SetDeviceInfo` when `(now − last_update_time > 7 d && now − last_attempt_update_time >
    /// 1 d) || metadata_hash ≠ hash(current metadata)`. Success updates both times and the hash; other
    /// failures only `last_attempt_update_time`; `Unauthorized` deprovisions the account.
    pub(super) async fn client_info_refresh(&self, env: Environment, account: &LocalAccount) -> Result<(), AuthError> {
        let due = {
            let st = self.state.lock_or_panic();
            let Some(info) = st.account_info.get(&account.account_id).filter(|i| i.state == AccountState::Enrolled)
            else {
                return Err(AuthError::NotProvisioned("the account is not enrolled".into()));
            };

            let hash = metadata_hash(&self.metadata_locked(&st)?);
            let now = self.now();
            let stale_update =
                since(now, info.last_update_time).is_none_or(|d| d > self.config.auth.client_info_refresh);
            let stale_attempt = since(now, info.last_attempt_update_time)
                .is_none_or(|d| d > self.config.auth.client_info_attempt_interval);
            (stale_update && stale_attempt) || info.metadata_hash.as_deref() != Some(hash.as_str())
        };

        if !due {
            return Ok(());
        }

        // Tokens first: their failures are not a `SetDeviceInfo` result, so nothing is
        // deprovisioned or recorded for them (an unlinked or invalid identity was already handled).
        let tokens = match self.msa_token(env).await {
            Ok(msa) => self.services_token_for(env, false).await.map(|services| (msa, services)),
            Err(e) => Err(e),
        };

        let (msa, services) = match tokens {
            Ok(t) => t,
            Err(e @ (AuthError::UnlinkedDevice | AuthError::InvalidIdentity | AuthError::Config(_))) => {
                return Err(e);
            }
            Err(e) => {
                tracing::warn!(error = %e, "client-info refresh skipped; no token");
                return Ok(());
            }
        };

        self.send_client_info(env, account, &msa, &services).await
    }

    /// Step 7's `SetDeviceInfo` and what its result does: `Unauthorized` deprovisions the account,
    /// an unlinked device or a local failure is returned, any other failure records the attempt.
    async fn send_client_info(
        &self,
        env: Environment,
        account: &LocalAccount,
        msa: &str,
        services: &str,
    ) -> Result<(), AuthError> {
        match self.send_device_info(env, account, msa, services).await {
            Ok(()) => {
                tracing::info!(%env, "client info refreshed");
                Ok(())
            }
            Err(AuthError::Unauthorized(m)) => {
                let mut st = self.state.lock_or_panic();
                self.deprovision_locked(&mut st, &account.account_id, Deprovision::Unauthorized)?;
                Err(AuthError::Unauthorized(m))
            }
            Err(e @ (AuthError::UnlinkedDevice | AuthError::Config(_))) => Err(e),
            Err(e) => {
                tracing::warn!(error = %e, "client-info refresh failed; tried again after the attempt interval");
                let mut st = self.state.lock_or_panic();
                if let Some(info) = st.account_info.get_mut(&account.account_id) {
                    info.last_attempt_update_time = Some(self.now());
                }
                self.persist_account_info(&st)?;
                Ok(())
            }
        }
    }

    /// `SetDeviceInfo` with the current metadata and trust certificate (tokens included).
    pub(crate) async fn set_device_info(&self, env: Environment, account: &LocalAccount) -> Result<(), AuthError> {
        let msa = self.msa_token(env).await?;
        let services = self.services_token_for(env, false).await?;
        self.send_device_info(env, account, &msa, &services).await
    }

    /// The `SetDeviceInfo` call itself; success persists a new trust certificate and sets
    /// `last_update_time = last_attempt_update_time = now` and the hash.
    pub(super) async fn send_device_info(
        &self,
        env: Environment,
        account: &LocalAccount,
        msa: &str,
        services: &str,
    ) -> Result<(), AuthError> {
        let _guard = self.device_info_lock.lock().await;

        let (trust, is_new, body) = {
            let st = self.state.lock_or_panic();
            let (trust, is_new) = self.trust_pair_for_send(&st, env)?;
            let body = self.device_body(&st, &trust)?;
            (trust, is_new, body)
        };

        let trace = TraceContext::new_root(Vec::new());
        match self.rest.set_device_info(env, msa, services, &body, &trace).await {
            Ok(()) => {}
            Err(e) if e.is_unlinked() => return Err(self.unlinked(env)),
            Err(e) => return Err(self.msa_call_failure(env, e)),
        }

        let now = self.now();
        let mut st = self.state.lock_or_panic();
        if is_new {
            st.env_mut(env).trust_keys = Some(trust);
            self.persist_trust_keys(&st, env)?;
        }

        if let Some(info) = st.account_info.get_mut(&account.account_id) {
            info.last_update_time = Some(now);
            info.last_attempt_update_time = Some(now);
            info.metadata_hash = Some(metadata_hash(&body.metadata));
        }
        self.persist_account_info(&st)
    }

    /// The `EnrollDevice` / `SetDeviceInfo` `metadata`, from `config.identity` and the current
    /// capabilities.
    pub(crate) fn metadata_locked(&self, st: &State) -> Result<DeviceMetadata, AuthError> {
        let identity = &self.config.identity;
        let display_name =
            identity.display_name.clone().unwrap_or_else(|| display_name_from_host(&self.edges.host.host_name()));

        for (field, value) in [
            ("displayName", display_name.as_str()),
            ("clientVersion", identity.profile.package_version),
            ("osVersion", identity.os_version.as_str()),
            ("clientType", identity.profile.client_type),
            ("osName", identity.os_name.as_str()),
        ] {
            if value.is_empty() {
                return Err(AuthError::Config(format!("metadata.{field} must be non-empty")));
            }
        }

        Ok(DeviceMetadata {
            capabilities: st.capabilities.clone(),
            client_type: identity.profile.client_type.to_string(),
            client_version: identity.profile.package_version.to_string(),
            consent_version: 0,
            custom_data: String::new(),
            display_name,
            is_enabled: true,
            manufacture: "Unknown".into(),
            model_name: "Unknown".into(),
            model_version: "Unknown".into(),
            os_name: identity.os_name.clone(),
            os_version: identity.os_version.clone(),
        })
    }

    pub(crate) fn device_body(&self, st: &State, trust: &KeyPair) -> Result<DeviceInfoBody, AuthError> {
        Ok(DeviceInfoBody {
            metadata: self.metadata_locked(st)?,
            certificates: DeviceCertificates {
                ct_self_signed: None,
                self_signed: Some(vec![trust.cert.base64()]),
                pki: None,
            },
        })
    }

    /// Trust certificate rotation: the stored `CN=trust_<dcgClientId>` unless missing, keyless, or
    /// `now > notAfter − 182 d` — then a new pair, persisted by the caller only after the server
    /// accepted it. Returns `(pair, is_new)`.
    pub(crate) fn trust_pair_for_send(&self, st: &State, env: Environment) -> Result<(KeyPair, bool), AuthError> {
        let device_id = st
            .env(env)
            .identity
            .as_ref()
            .map(|i| i.device_id.clone())
            .ok_or_else(|| AuthError::NotProvisioned(format!("no identity in {env}")))?;
        let cn = trust_client_id(&device_id);
        let now = self.now();

        if let Some(pair) = &st.env(env).trust_keys {
            let due = pair
                .cert
                .not_after
                .checked_sub(self.config.auth.trust_rotate_before_expiry)
                .is_none_or(|limit| now > limit);

            if pair.cn().eq_ignore_ascii_case(&cn) && !due {
                return Ok((pair.clone(), false));
            }
        }

        Ok((KeyPair::generate(&cn, now)?, true))
    }
}

/// Refuses an environment other than the one the account is provisioned in.
fn check_account_environment(st: &State, account: &LocalAccount, env: Environment) -> Result<(), AuthError> {
    if let Some(info) = st.account_info.get(&account.account_id) {
        let provisioned = matches!(info.state, AccountState::Provisioned | AccountState::EnrolledEnvironmentMismatch);
        if provisioned && info.environment.is_some_and(|e| e != env) {
            return Err(AuthError::MultipleAccountNotSupported(format!(
                "the account is provisioned in {:?} but the service environment is {env}",
                info.environment
            )));
        }
    }
    Ok(())
}
