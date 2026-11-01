//! Trust synchronization (peers → `AsyncD2D`).
//!
//! It covers fetch modes, the cloud refresh (device list, own-entry check and repair, linked peers,
//! `AsyncD2D` add / rotate / remove, the account certificate), the peer cache and events, the
//! user's `UnlinkDevice`, and [`AuthService`]'s [`TrustGate`] answers.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use phonelink_core::seams::{TrustGate, TrustRotationError};
use phonelink_core::types::{DcgClient, DeviceInfoItem, Environment, TraceContext};

use super::crypto::{self, ParsedCertificate};
use super::identity::Deprovision;
use super::store::{AccountState, LocalAccount};
use super::trust::{CryptoTrustType, new_relationship, trust_client_id};
use super::{AccountTrustCause, AuthError, AuthService, DeviceListApplied, FetchMode, PeerDevice, PeerEvent, State};
use phonelink_core::sync::MutexExt;

/// `yppCapabilities` bit `AsyncTrust`.
pub const YPP_CAPABILITY_ASYNC_TRUST: u64 = 0x20;

/// `customData.yppCapabilities` (`0` when absent or unparsable).
pub fn ypp_capabilities(item: &DeviceInfoItem) -> u64 {
    item.custom_data
        .as_deref()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
        .and_then(|v| v.get("yppCapabilities").and_then(serde_json::Value::as_u64))
        .unwrap_or(0)
}

/// Whether a pairing-era record may be replaced by an `AsyncD2D` record: `AsyncTrust` set or an
/// `iOS` peer.
pub fn is_async_trust_capable(item: &DeviceInfoItem) -> bool {
    ypp_capabilities(item) & YPP_CAPABILITY_ASYNC_TRUST != 0
        || item.os_name.as_deref().is_some_and(|os| os.eq_ignore_ascii_case("ios"))
}

/// The peer's certificate: `PKI[0]`, else `SelfSigned[0]` (`CTSelfSigned` is never selected).
pub fn peer_certificate_b64(item: &DeviceInfoItem) -> Option<&str> {
    let certs = item.certificates.as_ref()?;
    certs
        .pki
        .as_ref()
        .and_then(|v| v.first())
        .or_else(|| certs.self_signed.as_ref().and_then(|v| v.first()))
        .map(String::as_str)
}

/// The metadata hash of a peer for `Updated` events: SHA-256 of the entry without the volatile
/// `lastSeenTime` (local only).
fn peer_hash(item: &DeviceInfoItem) -> String {
    let mut stable = item.clone();
    stable.last_seen_time = None;
    hex::encode(Sha256::digest(serde_json::to_vec(&stable).unwrap_or_default()))
}

fn since(now: SystemTime, then: Option<SystemTime>) -> Option<Duration> {
    then.map(|t| now.duration_since(t).unwrap_or(Duration::ZERO))
}

fn parse_b64_certificate(b64: &str) -> Result<ParsedCertificate, crypto::CryptoError> {
    crypto::parse_certificate(&crypto::b64_std_decode(b64)?)
}

/// What applying one linked peer of a device list did to its relationship.
enum PeerTrust {
    /// The peer has no PKI or `SelfSigned` certificate.
    NoCertificate,
    /// A pairing-era relationship kept: the peer is not `AsyncTrust`-capable.
    PairingEraKept,
    /// The relationship that names the peer is kept, its certificate rotated when it changed.
    Kept,
    /// A new `AsyncD2D` relationship, `replaced` a pairing-era one.
    Added { replaced: bool },
    /// The peer's relationship could not be added or its certificate rotated.
    Failed(String),
}

/// What one application of the device list did to the linked peers' relationships, for its line.
#[derive(Default)]
struct AppliedPeers {
    removed_count: usize,
    added_count: usize,
    replaced_count: usize,
    failed_count: usize,
    /// The last failure's text.
    last_failure: Option<String>,
}

impl AppliedPeers {
    fn count(&mut self, outcome: PeerTrust) {
        match outcome {
            PeerTrust::NoCertificate | PeerTrust::PairingEraKept | PeerTrust::Kept => {}
            PeerTrust::Added { replaced } => {
                self.added_count += 1;
                self.replaced_count += usize::from(replaced);
            }
            PeerTrust::Failed(error) => {
                self.failed_count += 1;
                self.last_failure = Some(error);
            }
        }
    }
}

impl AuthService {
    /// Trust synchronization with `mode`; returns the cached peers that still have a `trust_<id>`
    /// relationship. A failed `GetDeviceInfoList` fails only `ForceCloud`; otherwise the cache is
    /// returned.
    pub async fn sync_trust(&self, mode: FetchMode) -> Result<Vec<PeerDevice>, AuthError> {
        let _guard = self.sync_lock.lock().await;
        let (account, env) = self.enrolled()?;

        let refresh = match mode {
            FetchMode::LocalOnly => false,
            FetchMode::ForceCloud => true,
            FetchMode::Default => {
                let st = self.state.lock_or_panic();
                let last = st.account_info.get(&account.account_id).and_then(|i| i.last_get_trusted_peer_clients_time);
                // A missing own entry (a store from before it was kept) is fetched once regardless.
                st.own_entry.is_none() || since(self.now(), last).is_none_or(|d| d > self.config.auth.peer_refresh)
            }
        };

        if refresh {
            self.cloud_refresh(env, &account, mode).await?;
        }

        Ok(self.peers())
    }

    /// This identity's own entry in the last device list applied (no network, kept across restarts),
    /// or `None` before one was applied.
    pub fn own_entry(&self) -> Option<DeviceInfoItem> {
        self.state.lock_or_panic().own_entry.clone()
    }

    /// The cached linked peers that still have a valid `trust_<id>` relationship (no network).
    pub fn peers(&self) -> Vec<PeerDevice> {
        let mut st = self.state.lock_or_panic();
        self.prune_trust_locked(&mut st);
        Self::peers_locked(&st)
    }

    fn peers_locked(st: &State) -> Vec<PeerDevice> {
        let Some(account) = &st.account else { return Vec::new() };
        let Some(env) = st
            .account_info
            .get(&account.account_id)
            .filter(|i| i.state == AccountState::Enrolled)
            .and_then(|i| i.environment)
        else {
            return Vec::new();
        };

        st.peer_cache
            .get(&account.account_id)
            .map(|items| {
                items
                    .iter()
                    .filter(|d| st.trust.index_by_partner_client_id(&trust_client_id(&d.id)).is_some())
                    .map(|d| PeerDevice { client: DcgClient::new(d.id.clone(), env), info: d.clone() })
                    .collect()
            })
            .unwrap_or_default()
    }

    async fn fetch_device_list(&self, env: Environment) -> Result<Vec<DeviceInfoItem>, AuthError> {
        let msa = self.msa_token(env).await?;
        let trace = TraceContext::new_root(Vec::new());

        match self.rest.device_info_list(env, &msa, &trace).await {
            Ok(list) => Ok(list),
            Err(e) if e.is_unlinked() => Err(self.unlinked(env)),
            Err(e) => Err(self.msa_call_failure(env, e)),
        }
    }

    /// Cloud refresh steps 1–6.
    async fn cloud_refresh(&self, env: Environment, account: &LocalAccount, mode: FetchMode) -> Result<(), AuthError> {
        // 1. The device list.
        let Some(list) = self.cloud_device_list(env, mode).await? else {
            return Ok(());
        };

        // 2. Own-entry check: our id with a `SelfSigned` certificate equal to a stored one.
        let (own_id, stored) = self.own_id_and_certificates(env)?;
        self.check_own_entry(env, account, &list, &own_id, &stored).await?;

        // 3–6 under the state lock.
        let before = self.peers();
        let applied = self.apply_device_list_locked(env, account, &own_id, list)?;
        let after = self.peers();
        self.raise_peer_events(&before, &after);

        tracing::info!(
            peer_count = after.len(),
            removed_count = applied.removed_count,
            added_count = applied.added_count,
            replaced_count = applied.replaced_count,
            "device list applied"
        );
        if let Some(error) = &applied.last_failure {
            tracing::warn!(
                failed_count = applied.failed_count,
                %error,
                "linked peers without a usable relationship; they stay untrusted until the next refresh"
            );
        }

        // Every application of the device list, changed or not (no receiver is not an error).
        let _ = self.device_list_events.send(DeviceListApplied { peers: after });

        // 6. The account certificate → A2D.
        self.sync_account_certificate(env, account).await
    }

    /// Step 1: the device list; `None` when the fetch failed and the cached peers are kept (only an
    /// unforced refresh keeps them, and never past an unlinked device or a configuration error).
    async fn cloud_device_list(
        &self,
        env: Environment,
        mode: FetchMode,
    ) -> Result<Option<Vec<DeviceInfoItem>>, AuthError> {
        match self.fetch_device_list(env).await {
            Ok(list) => Ok(Some(list)),
            Err(e @ (AuthError::UnlinkedDevice | AuthError::Config(_))) => Err(e),
            Err(e) if mode == FetchMode::ForceCloud => Err(e),
            Err(e) => {
                tracing::warn!(error = %e, "GetDeviceInfoList failed; the cached peers are kept");
                Ok(None)
            }
        }
    }

    /// The identity's device id and the DER of every stored certificate of `env`.
    fn own_id_and_certificates(&self, env: Environment) -> Result<(String, Vec<Vec<u8>>), AuthError> {
        let st = self.state.lock_or_panic();
        let slot = st.env(env);
        let own_id = slot
            .identity
            .as_ref()
            .map(|i| i.device_id.clone())
            .ok_or_else(|| AuthError::NotProvisioned(format!("no identity in {env}")))?;

        let stored: Vec<Vec<u8>> =
            [&slot.trust_keys, &slot.identity_keys].into_iter().flatten().map(|p| p.cert.der.clone()).collect();
        Ok((own_id, stored))
    }

    /// Step 2: repairs an own device-list entry that is absent or carries no stored certificate
    /// (`SetDeviceInfo`); an absent entry that cannot be repaired deprovisions the account.
    async fn check_own_entry(
        &self,
        env: Environment,
        account: &LocalAccount,
        list: &[DeviceInfoItem],
        own_id: &str,
        stored: &[Vec<u8>],
    ) -> Result<(), AuthError> {
        let own_self_signed = list
            .iter()
            .find(|d| d.id.eq_ignore_ascii_case(own_id))
            .and_then(|d| d.certificates.as_ref())
            .and_then(|c| c.self_signed.as_ref())
            .filter(|certs| !certs.is_empty());
        let own_ok = own_self_signed.is_some_and(|certs| {
            certs.iter().any(|c| crypto::b64_std_decode(c).is_ok_and(|der| stored.contains(&der)))
        });

        if own_ok {
            return Ok(());
        }

        let missing = own_self_signed.is_none();
        match self.set_device_info(env, account).await {
            Ok(()) => tracing::info!(missing, "own device-list entry repaired with SetDeviceInfo"),
            // The identity is gone (unlinked, or cleared as invalid): nothing to bind peers to.
            Err(e @ (AuthError::UnlinkedDevice | AuthError::InvalidIdentity)) => return Err(e),
            Err(e) if missing => {
                let mut st = self.state.lock_or_panic();
                self.deprovision_locked(&mut st, &account.account_id, Deprovision::EnrollmentExpired)?;
                return Err(AuthError::Unauthorized(format!("enrollment expired: {e}")));
            }
            Err(e) => {
                tracing::warn!(error = %e, "stale own device-list entry not repaired; tried again at the next refresh")
            }
        }
        Ok(())
    }

    /// Steps 3–6 under the state lock: the own entry, the relationships of unlinked and linked
    /// peers, the peer cache and its timestamp, persisted.
    fn apply_device_list_locked(
        &self,
        env: Environment,
        account: &LocalAccount,
        own_id: &str,
        list: Vec<DeviceInfoItem>,
    ) -> Result<AppliedPeers, AuthError> {
        let now = self.now();
        let mut st = self.state.lock_or_panic();
        st.trust.prune(now, &self.policy);

        let self_client_id = trust_client_id(own_id);
        st.own_entry = list.iter().find(|d| d.id.eq_ignore_ascii_case(own_id)).cloned();
        tracing::debug!(
            listed = st.own_entry.is_some(),
            distinguished_device_id = st.own_entry.as_ref().and_then(|e| e.distinguished_device_id.as_deref()),
            "own device-list entry applied"
        );

        let peers: Vec<DeviceInfoItem> =
            list.into_iter().filter(|d| d.is_linked == Some(true) && !d.id.eq_ignore_ascii_case(own_id)).collect();
        let linked: HashSet<String> = peers.iter().map(|d| d.id.to_ascii_lowercase()).collect();

        // 4. AsyncD2D relationships of peers that are no longer linked.
        let mut applied = AppliedPeers {
            removed_count: st.trust.remove_where(|r| {
                r.trust_type == CryptoTrustType::AsyncD2D
                    && r.dcg_client_id(env).is_some_and(|id| !linked.contains(&id.to_ascii_lowercase()))
            }),
            ..AppliedPeers::default()
        };

        // 5. One relationship per linked peer.
        for peer in &peers {
            applied.count(self.trust_peer_locked(&mut st, env, account, &self_client_id, peer, now));
        }

        // 6. Cache, timestamp.
        st.peer_cache.insert(account.account_id.clone(), peers);
        if let Some(info) = st.account_info.get_mut(&account.account_id) {
            info.last_get_trusted_peer_clients_time = Some(now);
        }

        self.persist_trust(&st)?;
        self.persist_peer_cache(&st)?;
        self.persist_account_info(&st)?;

        Ok(applied)
    }

    /// Step 6, last part: the account certificate as the A2D relationship.
    async fn sync_account_certificate(&self, env: Environment, account: &LocalAccount) -> Result<(), AuthError> {
        let msa = self.msa_token(env).await?;
        let trace = TraceContext::new_root(Vec::new());

        // GetAccountCert is not among the calls whose 403 + 13 means "unlinked".
        let account_cert =
            self.rest.account_cert(env, &msa, &trace).await.map_err(|e| self.msa_call_failure(env, e))?;
        let cert = account_cert
            .ok_or(AccountTrustCause::NoAccountCert)
            .and_then(|b64| parse_b64_certificate(&b64).map_err(AccountTrustCause::Certificate))
            .map_err(AuthError::AccountTrustSyncFailure)?;

        let mut st = self.state.lock_or_panic();
        self.upsert_a2d_locked(&mut st, env, account, &cert, self.now())
            .map_err(|e| AuthError::AccountTrustSyncFailure(AccountTrustCause::Record(Box::new(e))))
    }

    /// Step 5 for one linked peer; a failure skips the peer (the device list's line counts it).
    fn trust_peer_locked(
        &self,
        st: &mut State,
        env: Environment,
        account: &LocalAccount,
        self_client_id: &str,
        peer: &DeviceInfoItem,
        now: SystemTime,
    ) -> PeerTrust {
        let Some(b64) = peer_certificate_b64(peer) else {
            tracing::debug!(peer = %peer.id, "linked peer without a PKI or SelfSigned certificate; skipped");
            return PeerTrust::NoCertificate;
        };
        let cert = match parse_b64_certificate(b64) {
            Ok(cert) => cert,
            Err(e) => return PeerTrust::Failed(format!("the peer certificate: {e}")),
        };

        let partner_client_id = trust_client_id(&peer.id);
        let client = DcgClient::new(peer.id.clone(), env);
        if let Some(index) = st.trust.index_by_partner_client_id(&partner_client_id) {
            return self.refresh_partner_relationship(st, index, account, &cert, now);
        }

        let mut replaced = false;
        if let Some(index) = st.trust.index_by_dcg_client(&client) {
            if !is_async_trust_capable(peer) {
                tracing::debug!(peer = %peer.id, "pairing-era relationship kept; the peer is not AsyncTrust-capable");
                return PeerTrust::PairingEraKept;
            }
            st.trust.remove(index);
            replaced = true;
        }

        let added = new_relationship(
            self_client_id,
            &partner_client_id,
            &account.cid,
            CryptoTrustType::AsyncD2D,
            &cert,
            env,
            &peer.id,
            now,
            &self.policy,
        )
        .and_then(|record| st.trust.add(record, now, &self.policy));
        match added {
            Ok(_) => PeerTrust::Added { replaced },
            Err(e) => PeerTrust::Failed(format!("the AsyncD2D relationship: {e}")),
        }
    }

    /// The relationship at `index` already names the peer: its certificate rotated, and an unset
    /// account or type completed as this account's `AsyncD2D`.
    fn refresh_partner_relationship(
        &self,
        st: &mut State,
        index: usize,
        account: &LocalAccount,
        cert: &ParsedCertificate,
        now: SystemTime,
    ) -> PeerTrust {
        let rotated = st.trust.rotate_partner_certificate(index, cert, now, &self.policy);
        let r = st.trust.get_mut(index);
        if r.msa_cid.as_deref().is_none_or(str::is_empty) || r.trust_type == CryptoTrustType::Unknown {
            r.msa_cid = Some(account.cid.clone());
            r.trust_type = CryptoTrustType::AsyncD2D;
        }

        match rotated {
            Ok(_) => PeerTrust::Kept,
            Err(e) => PeerTrust::Failed(format!("the peer certificate not rotated: {e}")),
        }
    }

    fn raise_peer_events(&self, before: &[PeerDevice], after: &[PeerDevice]) {
        let old: HashMap<String, &PeerDevice> = before.iter().map(|p| (p.client.id.to_ascii_lowercase(), p)).collect();
        let new: HashMap<String, &PeerDevice> = after.iter().map(|p| (p.client.id.to_ascii_lowercase(), p)).collect();

        for p in after {
            match old.get(&p.client.id.to_ascii_lowercase()) {
                None => {
                    let _ = self.peer_events.send(PeerEvent::Added(p.clone()));
                }
                Some(o) => {
                    if o.info.distinguished_device_id != p.info.distinguished_device_id
                        || peer_hash(&o.info) != peer_hash(&p.info)
                    {
                        let _ = self.peer_events.send(PeerEvent::Updated(p.clone()));
                    }
                }
            }
        }

        for p in before {
            if !new.contains_key(&p.client.id.to_ascii_lowercase()) {
                let _ = self.peer_events.send(PeerEvent::Removed(p.client.clone()));
            }
        }
    }

    /// The user's unlink of `peer`: `UnlinkDevice` followed by a forced peer refresh. The app's UI
    /// unlinks the account's other PCs with it (Settings); it offers no unlink of a phone.
    pub async fn unlink_device(&self, peer: &DcgClient) -> Result<Vec<PeerDevice>, AuthError> {
        let (_, env) = self.enrolled()?;
        if peer.env != env {
            return Err(AuthError::Config(format!("a peer is not in the account's environment {env}")));
        }

        let msa = self.msa_token(env).await?;
        let services = self.services_token_for(env, false).await?;
        let trace = TraceContext::new_root(Vec::new());

        match self.rest.unlink_device(env, &msa, &services, &peer.id, &trace).await {
            Ok(()) => tracing::info!(%peer, "peer unlinked from the account with UnlinkDevice"),
            Err(e) if e.is_unlinked() => return Err(self.unlinked(env)),
            Err(e) => return Err(self.msa_call_failure(env, e)),
        }

        self.sync_trust(FetchMode::ForceCloud).await
    }

    /// Deletes this identity's own record from the account (`DeleteDeviceInfo`). The local state is
    /// left as it is; the identity is gone on the service, so it is not used again.
    pub async fn delete_own_device_info(&self) -> Result<(), AuthError> {
        let (_, env) = self.enrolled()?;
        let msa = self.msa_token(env).await?;
        let services = self.services_token_for(env, false).await?;
        let trace = TraceContext::new_root(Vec::new());

        match self.rest.delete_device_info(env, &msa, &services, &trace).await {
            Ok(()) => {
                tracing::info!(%env, "this identity's record deleted from the account with DeleteDeviceInfo");
                Ok(())
            }
            Err(e) if e.is_unlinked() => Err(self.unlinked(env)),
            Err(e) => Err(self.msa_call_failure(env, e)),
        }
    }

    /// Whether another forced refresh for `source` fits the per-hour budget (and records it).
    fn take_forced_refresh_budget(&self, source: &DcgClient) -> bool {
        let now = tokio::time::Instant::now();
        let hour = Duration::from_secs(3600);
        let mut map = self.forced_refreshes.lock_or_panic();
        let queue = map.entry(source.to_string().to_ascii_lowercase()).or_default();

        while queue.front().is_some_and(|t| now.duration_since(*t) >= hour) {
            queue.pop_front();
        }

        if queue.len() >= self.config.auth.forced_peer_refresh_per_hour as usize {
            return false;
        }

        queue.push_back(now);
        true
    }
}

impl TrustGate for AuthService {
    /// Prunes expired records, then the record naming `partner` must be trusted (its `A2D`
    /// anchor, while one exists, unexpired); a hit refreshes its last access.
    fn is_trusted(&self, partner: &DcgClient) -> bool {
        let now = self.now();
        let mut st = self.state.lock_or_panic();
        let mut changed = st.trust.prune(now, &self.policy);

        let trusted = match st.trust.trusted_index_by_dcg_client(partner, now, &self.policy) {
            Some(index) => {
                changed |= st.trust.touch(index, now, &self.policy);
                true
            }
            None => false,
        };

        if changed {
            if let Err(e) = self.persist_trust(&st) {
                tracing::error!(error = %e, "persisting the trust store failed");
            }
        }
        trusted
    }

    fn partner_key_expiration(&self, partner: &DcgClient) -> Option<SystemTime> {
        let now = self.now();
        let mut st = self.state.lock_or_panic();
        self.prune_trust_locked(&mut st);
        st.trust
            .trusted_index_by_dcg_client(partner, now, &self.policy)
            .map(|i| st.trust.get(i).partner_key_expiration_time)
    }

    fn rotate_partner_certificate(
        &self,
        partner: &DcgClient,
        certificate_der: &[u8],
    ) -> Result<(), TrustRotationError> {
        let now = self.now();
        let mut st = self.state.lock_or_panic();
        self.prune_trust_locked(&mut st);

        let index =
            st.trust.trusted_index_by_dcg_client(partner, now, &self.policy).ok_or(TrustRotationError::NotFound)?;
        let cert = crypto::parse_certificate(certificate_der).map_err(|_| TrustRotationError::InvalidCertificate)?;
        st.trust
            .rotate_partner_certificate(index, &cert, now, &self.policy)
            .map_err(|_| TrustRotationError::InvalidCertificate)?;

        self.persist_trust(&st).map_err(|e| TrustRotationError::Store(e.to_string()))
    }

    /// Runs a `ForceCloud` refresh in the background, at most `forced_peer_refresh_per_hour` times
    /// per source per hour.
    fn on_untrusted_source(&self, partner: &DcgClient) {
        if !self.take_forced_refresh_budget(partner) {
            tracing::debug!(%partner, "forced peer refresh budget exhausted");
            return;
        }

        let Some(this) = self.weak.upgrade() else { return };

        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                let partner = partner.clone();
                handle.spawn(async move {
                    if let Err(e) = this.sync_trust(FetchMode::ForceCloud).await {
                        tracing::warn!(%partner, error = %e, "forced peer refresh failed");
                    }
                });
            }
            Err(_) => tracing::warn!(%partner, "forced peer refresh skipped; no async runtime"),
        }
    }
}

#[cfg(test)]
mod tests;
