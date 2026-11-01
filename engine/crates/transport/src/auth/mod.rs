//! Identity, enrollment, trust and wake — the control plane of the DCG authentication and enrollment
//! protocol.
//!
//! [`AuthService`] is the facade: it signs the user in to the Microsoft account, provisions and enrolls the
//! `WEA` identity (the startup and provisioning sequence), keeps the services token
//! ([`phonelink_core::seams::TokenSource`]), synchronizes and answers trust
//! ([`phonelink_core::seams::TrustGate`]), and builds the [`phonelink_core::seams::Waker`] over a hub. The
//! pieces are public for the CLI and the tests:
//!
//! | module | covers | |---|---| | [`crypto`] | crypto parameters; the nonce JWT, inbound JWT validation | |
//! [`dcg_rest`] | the DCG REST calls, error responses and status mapping | | [`rest_policy`] | the resilience
//! policy of the DCG REST calls: timeouts, retries, circuit breaker | | [`msa`] | MSA tokens and account
//! identification | | [`network`] | transport failures with an internet cause (relay `OpenStatus`
//! `InternetError`) | | [`pairing`] | QR/PIN account pairing of a phone (`DevicePairingProxy`) | | [`store`]
//! | what must survive a restart | | [`identity`] | startup and provisioning, identity lifecycle, services
//! token refresh, key and cert rotation | | [`trust`] | the trust relationship record, trust validation rules
//! | | [`sync`] | trust synchronization (peers → `AsyncD2D`) | | [`wake`] | the wake sequence,
//! `/Dispatcher/Wake` |

pub mod crypto;
pub mod dcg_rest;
pub mod identity;
pub mod live;
pub mod msa;
pub mod pairing;
pub mod rest_policy;
pub mod store;
pub mod sync;
pub mod trust;
pub mod wake;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex, Weak};
use std::time::SystemTime;

use tokio::sync::broadcast;

use phonelink_core::config::EngineConfig;
use phonelink_core::constants::ScopeFamily;
use phonelink_core::edge::Edges;
use phonelink_core::identification::ClientIdentification;
use phonelink_core::seams::{TokenError, WakeHub, Waker};
use phonelink_core::tap::Tap;
use phonelink_core::types::{DcgClient, DeviceInfoItem, Environment, new_guid_lower};

use self::crypto::{CryptoError, KeyPair};
use self::dcg_rest::{DcgRest, RestError};
use self::live::broker::LiveBroker;
use self::msa::{MsaClient, MsaError};
use self::store::{AccountInfo, IdentityRecord, LocalAccount, PeerCache, Persist, PersistError, StoredPair, keys};
use self::trust::{InboundJwt, InboundJwtError, TrustPolicy, TrustRelationship, TrustStore, TrustStoreError};

use self::identity::Deprovision;
#[cfg(feature = "test-support")]
pub use self::identity::metadata_hash;
pub use self::wake::WakeService;
use phonelink_core::sync::MutexExt;

/// Failures of the control plane.
/// Why the account's trust could not be synchronized.
#[derive(Debug, thiserror::Error)]
pub enum AccountTrustCause {
    #[error("no accountCert")]
    NoAccountCert,
    #[error("the account certificate has no CN")]
    NoCommonName,
    #[error("{0}")]
    Certificate(#[source] CryptoError),
    /// Recording the account's trust relationship failed.
    #[error("{0}")]
    Record(#[source] Box<AuthError>),
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// The MSA token needs the user: run [`AuthService::sign_in`].
    #[error("MSA sign-in needs the user: {0}")]
    UserInteractionRequired(String),
    /// The configured scope family cannot be minted by the device-code flow (RPS `MBI_SSL`).
    #[error("the {0:?} scope family cannot be minted by the engine's OAuth device-code flow")]
    UnsupportedScopeFamily(ScopeFamily),
    /// Any other failure of the MSA token provider, including
    /// [`MsaError::Internet`] — a token request that could not reach the endpoint (no connectivity,
    /// or no HTTP response because of the network).
    #[error(transparent)]
    Msa(MsaError),
    /// A DCG REST call without connectivity.
    #[error("no internet connectivity")]
    InternetUnavailable,
    /// A 401 status of the DCG status mapping, or "enrollment expired".
    #[error("DCG answered Unauthorized: {0}")]
    Unauthorized(String),
    /// 403 + `categoryCode 13`: identity cleared, accounts deprovisioned.
    #[error("the device was unlinked: identity cleared and every account deprovisioned")]
    UnlinkedDevice,
    /// 400 + `InvalidCertificate`: the identity was cleared.
    #[error("the identity certificate was rejected; the identity was cleared")]
    InvalidIdentity,
    #[error("MultipleAccountNotSupported: {0}")]
    MultipleAccountNotSupported(String),
    #[error("AccountTrustSyncFailure: {0}")]
    AccountTrustSyncFailure(#[source] AccountTrustCause),
    /// No account provisioned or no identity for the environment.
    #[error("not provisioned: {0}")]
    NotProvisioned(String),
    /// Any other REST failure (status in the error).
    #[error("{0}")]
    Rest(Box<RestError>),
    /// The server broke the contract (e.g. a `SignIn` `deviceId` echo that differs).
    #[error("unexpected server behaviour: {0}")]
    Protocol(String),
    #[error("crypto: {0}")]
    Crypto(#[from] CryptoError),
    #[error("trust store: {0}")]
    Trust(#[from] TrustStoreError),
    #[error("{0}")]
    Store(String),
    #[error("invalid local configuration: {0}")]
    Config(String),
}

impl From<PersistError> for AuthError {
    fn from(e: PersistError) -> Self {
        AuthError::Store(e.to_string())
    }
}

impl From<MsaError> for AuthError {
    fn from(e: MsaError) -> Self {
        match e {
            MsaError::InteractionRequired(m) => AuthError::UserInteractionRequired(m),
            MsaError::UnsupportedScopeFamily(f) => AuthError::UnsupportedScopeFamily(f),
            // An unreachable token endpoint stays an MSA failure here (`InternetUnavailable` is a DCG
            // REST call without connectivity); the relay's view is [`AuthError::to_token_error`].
            other => AuthError::Msa(other),
        }
    }
}

impl AuthError {
    /// Generic mapping of a REST failure the caller has no specific rule for. A local
    /// precondition failure (nothing was sent) is a configuration error.
    pub fn from_rest(e: RestError) -> Self {
        if let RestError::Local { .. } = e {
            AuthError::Config(e.to_string())
        } else if e.is_internet() {
            AuthError::InternetUnavailable
        } else if e.is_unauthorized() {
            AuthError::Unauthorized(e.to_string())
        } else {
            AuthError::Rest(Box::new(e))
        }
    }

    /// The relay's view (`TokenError::open_status` gives the relay's `OpenStatus`). An unreachable
    /// MSA token endpoint is [`TokenError::Internet`], which the relay's start retries and opens as
    /// `InternetError`. Every other failure of the MSA token provider is [`TokenError::Msa`], which
    /// the relay's start does not retry and which opens as `NonTransientAuthFailure`; a DCG REST
    /// failure stays `Http` / `Internet` / `Unauthorized`.
    pub fn to_token_error(&self) -> TokenError {
        match self {
            AuthError::UserInteractionRequired(_) => TokenError::UserInteractionRequired,
            AuthError::Msa(MsaError::Internet) => TokenError::Internet,
            AuthError::Msa(e) => TokenError::Msa(e.to_string()),
            // The engine's token provider cannot mint the configured scope family.
            AuthError::UnsupportedScopeFamily(_) => TokenError::Msa(self.to_string()),
            AuthError::InternetUnavailable => TokenError::Internet,
            AuthError::Unauthorized(m) => TokenError::Unauthorized(m.clone()),
            AuthError::Rest(e) => TokenError::Http(e.to_string()),
            AuthError::Crypto(e) => TokenError::Crypto(e.to_string()),
            AuthError::Store(m) => TokenError::KeyStoreTransient(m.clone()),
            AuthError::UnlinkedDevice | AuthError::InvalidIdentity | AuthError::NotProvisioned(_) => {
                TokenError::NotProvisioned
            }
            AuthError::MultipleAccountNotSupported(_)
            | AuthError::AccountTrustSyncFailure(_)
            | AuthError::Protocol(_)
            | AuthError::Trust(_)
            | AuthError::Config(_) => TokenError::Other(self.to_string()),
        }
    }

    /// No connectivity: a DCG REST call (`InternetUnavailable`) or the MSA token provider failed
    /// for lack of it.
    pub fn is_internet_unavailable(&self) -> bool {
        matches!(self, AuthError::InternetUnavailable | AuthError::Msa(MsaError::Internet))
    }
}

/// Trust synchronization fetch modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FetchMode {
    /// The cached peers that still have a `trust_<id>` relationship; no network.
    LocalOnly,
    /// As `LocalOnly`, refreshing from the cloud when the last refresh is older than 1 d.
    Default,
    /// Always refreshes from the cloud.
    ForceCloud,
}

/// A linked peer with a trust relationship: its `DcgClient` and its device-list entry.
#[derive(Debug, Clone, PartialEq)]
pub struct PeerDevice {
    pub client: DcgClient,
    pub info: DeviceInfoItem,
}

/// `Added` / `Removed` / `Updated` peer events of a cloud refresh (by id,
/// `distinguishedDeviceId`, metadata hash).
#[derive(Debug, Clone, PartialEq)]
pub enum PeerEvent {
    Added(PeerDevice),
    Removed(DcgClient),
    Updated(PeerDevice),
}

/// A cloud refresh applied the DCG device list — every refresh, whether or not an entry changed
/// (each application rewrites the Android phone record from the device list).
///
/// `peers` are the linked peers with a trust relationship after it.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceListApplied {
    pub peers: Vec<PeerDevice>,
}

/// An account was deprovisioned: its relationships, trust certificate and peer cache are gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRemoved {
    pub account_id: String,
}

/// The result of [`AuthService::ensure_provisioned`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provisioned {
    pub env: Environment,
    /// The PC's own `DcgClient` (`dcgClientId`, environment).
    pub own: DcgClient,
    pub account_id: String,
}

/// The cached services token of one environment (memory only, local scope key `general`).
#[derive(Debug, Clone)]
pub(crate) struct ServicesToken {
    pub access_token: String,
    /// The `deviceId` echo of the response.
    pub device_id: String,
    pub expires_at: SystemTime,
}

/// Per-environment identity state.
#[derive(Debug, Default)]
pub(crate) struct EnvState {
    pub identity: Option<IdentityRecord>,
    pub identity_keys: Option<KeyPair>,
    /// The incoming key of an identity key rotation the server may already hold (persisted before
    /// `RotateKeys`; promoted or dropped once a reply or a `SignIn` shows which key it holds).
    pub pending_identity_keys: Option<KeyPair>,
    pub account_id: Option<String>,
    pub trust_keys: Option<KeyPair>,
    pub services_token: Option<ServicesToken>,
}

/// Everything the control plane keeps, guarded by one lock that is never held across an `.await`.
#[derive(Debug)]
pub(crate) struct State {
    pub saved_env: Option<Environment>,
    pub account: Option<LocalAccount>,
    pub envs: BTreeMap<Environment, EnvState>,
    pub trust: TrustStore,
    pub account_info: BTreeMap<String, AccountInfo>,
    pub peer_cache: PeerCache,
    pub capabilities: Vec<String>,
    /// This identity's own entry in the last device list applied (the peer cache excludes it).
    /// Carries the `distinguishedDeviceId` the service assigned this record.
    pub own_entry: Option<DeviceInfoItem>,
}

/// An identity key + certificate entry of the identity `device_id`, or why it cannot be used (torn,
/// undecodable, or a certificate that is not `CN=<dcgClientId>`); one that cannot be used counts as
/// missing.
fn identity_pair(stored: StoredPair, device_id: &str) -> Result<Option<KeyPair>, String> {
    match stored {
        StoredPair::Absent => Ok(None),
        StoredPair::Present(pair) if pair.cn().eq_ignore_ascii_case(device_id) => Ok(Some(*pair)),
        StoredPair::Present(_) => Err("the certificate is not CN=<dcgClientId>".to_owned()),
        StoredPair::Unusable(reason) => Err(reason),
    }
}

impl State {
    /// The persisted state. A key + certificate entry that cannot be used counts as missing: an
    /// identity without its key is cleared, a trust certificate is rotated as missing. A failing
    /// store is an error.
    fn load(store: &Persist<'_>, capabilities: Vec<String>) -> Result<Self, AuthError> {
        let saved_env = match store.string(keys::ENVIRONMENT)? {
            Some(name) => Some(
                Environment::from_name(&name)
                    .ok_or_else(|| AuthError::Store(format!("{}: unknown environment {name:?}", keys::ENVIRONMENT)))?,
            ),
            None => None,
        };

        let account = store.json::<LocalAccount>(keys::ACCOUNT)?;

        let mut envs = BTreeMap::new();
        for env in Environment::ALL {
            let identity = store.json::<IdentityRecord>(&keys::identity(env))?;
            let stored_identity = store.key_pair(&keys::identity_pair(env))?;
            let stored_pending = store.key_pair(&keys::identity_pending_pair(env))?;

            let (identity_keys, pending_identity_keys) = match &identity {
                Some(record) => {
                    // `AuthService::new` clears an identity whose key this leaves out.
                    let identity_keys = identity_pair(stored_identity, &record.device_id)
                        .and_then(|pair| pair.ok_or_else(|| "missing".to_owned()))
                        .map_err(|reason| {
                            tracing::warn!(%env, %reason, "identity key + certificate unusable; the identity is cleared");
                        })
                        .ok();
                    let pending = identity_pair(stored_pending, &record.device_id).unwrap_or_else(|reason| {
                        tracing::warn!(%env, %reason, "pending rotation key + certificate unusable; dropped");
                        None
                    });
                    (identity_keys, pending)
                }
                // Keys without an identity record are left over from an interrupted write; the next
                // identity overwrites them.
                None => (None, None),
            };

            let trust_keys = match store.key_pair(&keys::trust_pair(env))? {
                StoredPair::Absent => None,
                StoredPair::Present(pair) => Some(*pair),
                StoredPair::Unusable(reason) => {
                    tracing::warn!(
                        %env,
                        %reason,
                        "trust key + certificate unusable; the next EnrollDevice or SetDeviceInfo sends a new one"
                    );
                    None
                }
            };
            let account_id = store.string(&keys::account_id(env))?;

            envs.insert(
                env,
                EnvState {
                    identity,
                    identity_keys,
                    pending_identity_keys,
                    account_id,
                    trust_keys,
                    services_token: None,
                },
            );
        }

        let trust = store
            .json::<Vec<TrustRelationship>>(keys::TRUST_RELATIONSHIPS)?
            .map(TrustStore::from_records)
            .unwrap_or_default();
        let account_info = store.json(keys::ACCOUNT_INFO)?.unwrap_or_default();
        let peer_cache = store.json(keys::PEER_CACHE)?.unwrap_or_default();
        let own_entry = store.json(keys::OWN_ENTRY)?;

        Ok(Self { saved_env, account, envs, trust, account_info, peer_cache, capabilities, own_entry })
    }

    pub(crate) fn env(&self, env: Environment) -> &EnvState {
        self.envs.get(&env).expect("every environment has a slot")
    }

    pub(crate) fn env_mut(&mut self, env: Environment) -> &mut EnvState {
        self.envs.get_mut(&env).expect("every environment has a slot")
    }
}

/// The control-plane facade (`Send + Sync`, shared as `Arc<AuthService>`).
pub struct AuthService {
    config: Arc<EngineConfig>,
    edges: Edges,
    rest: DcgRest,
    msa: MsaClient,
    policy: TrustPolicy,
    state: Mutex<State>,
    /// Serializes startup and provisioning sequence runs.
    provision_lock: tokio::sync::Mutex<()>,
    /// Serializes services-token refreshes (and the key rotation that follows a `SignIn`).
    token_lock: tokio::sync::Mutex<()>,
    /// Serializes trust synchronizations.
    sync_lock: tokio::sync::Mutex<()>,
    /// Serializes `SetDeviceInfo` (the client-info refresh and the own-entry repair), each of which
    /// may mint a trust certificate.
    device_info_lock: tokio::sync::Mutex<()>,
    peer_events: broadcast::Sender<PeerEvent>,
    device_list_events: broadcast::Sender<DeviceListApplied>,
    account_events: broadcast::Sender<AccountRemoved>,
    /// Forced (`ForceCloud`) refreshes triggered by untrusted sources, per source, in the last hour.
    forced_refreshes: Mutex<HashMap<String, VecDeque<tokio::time::Instant>>>,
    weak: Weak<AuthService>,
}

impl AuthService {
    /// Loads the persisted state (the first step of startup and provisioning). An identity whose
    /// key + certificate entry is missing or unusable is cleared; an unusable trust entry is
    /// missing, so the next `EnrollDevice` / `SetDeviceInfo` sends a new trust certificate.
    pub fn new(
        config: Arc<EngineConfig>,
        edges: Edges,
        ident: Arc<ClientIdentification>,
        tap: Tap,
        broker: Arc<LiveBroker>,
    ) -> Result<Arc<Self>, AuthError> {
        let http = DcgRest::http_client().map_err(|e| AuthError::Config(format!("HTTP client: {e}")))?;
        let rest = DcgRest::new(
            http.clone(),
            config.endpoints.clone(),
            config.identity.scope_family.authorization_portal(),
            ident,
            edges.reachability.clone(),
            tap.clone(),
        )
        .with_policy(config.auth.rest.clone());

        let msa = MsaClient::new(broker, config.identity.profile.msa_client_id, config.identity.scope_family);
        let state = State::load(&Persist(&*edges.store), config.auth.capabilities.clone())?;
        let policy = TrustPolicy::from_config(&config.auth);

        let (peer_events, _) = broadcast::channel(64);
        let (device_list_events, _) = broadcast::channel(16);
        let (account_events, _) = broadcast::channel(16);

        let service = Arc::new_cyclic(|weak| Self {
            config,
            edges,
            rest,
            msa,
            policy,
            state: Mutex::new(state),
            provision_lock: tokio::sync::Mutex::new(()),
            token_lock: tokio::sync::Mutex::new(()),
            sync_lock: tokio::sync::Mutex::new(()),
            device_info_lock: tokio::sync::Mutex::new(()),
            peer_events,
            device_list_events,
            account_events,
            forced_refreshes: Mutex::new(HashMap::new()),
            weak: weak.clone(),
        });

        {
            let mut st = service.state.lock_or_panic();
            for env in Environment::ALL {
                let slot = st.env(env);
                // `State::load` logged why the key is missing.
                if slot.identity.is_some() && slot.identity_keys.is_none() {
                    service.clear_identity_locked(&mut st, env)?;
                }
            }
        }

        Ok(service)
    }

    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// The REST client (for the CLI and tests).
    pub fn rest(&self) -> &DcgRest {
        &self.rest
    }

    /// The MSA token provider.
    pub fn msa(&self) -> &MsaClient {
        &self.msa
    }

    fn now(&self) -> SystemTime {
        self.edges.clock.now()
    }

    /// Signs the user in through the device-bound broker: ensures a signed-in user session, running the
    /// interactive web-view sign-in when there is none, and learns the account PUID. A PUID equal to the
    /// stored local account's keeps it, with its identity, trust and phone; any other — a different PUID, or
    /// no stored account (including an old store that recorded a `sub` and no PUID) — is an account switch
    /// (`switch_account`).
    pub async fn sign_in(&self) -> Result<(), AuthError> {
        let puid = self.msa.ensure_signed_in().await?;
        let stored = self.state.lock_or_panic().account.as_ref().map(|a| a.puid.as_deref() == Some(puid.as_str()));

        let sign_in_kind = match stored {
            Some(true) => "stored_account",
            Some(false) => "account_switch",
            None => "first_account",
        };

        if stored != Some(true) {
            self.switch_account(&puid)?;
        }

        tracing::info!(sign_in_kind, "signed in");
        Ok(())
    }

    /// An account switch: every account record is deprovisioned (its relationships, trust
    /// certificate and peer cache) and every identity cleared with its relationships, and a new
    /// local account with fresh ids is minted for `puid`. The broker's user session is already the
    /// new account's; only the previous account's DCG state is cleared here.
    fn switch_account(&self, puid: &str) -> Result<(), AuthError> {
        let mut st = self.state.lock_or_panic();
        self.forget_account_locked(&mut st, Deprovision::AccountSwitch)?;
        let account =
            LocalAccount { account_id: new_guid_lower(), cid: new_guid_lower(), puid: Some(puid.to_string()) };
        Persist(&*self.edges.store).put_json(keys::ACCOUNT, &account)?;
        st.account = Some(account);
        Ok(())
    }

    /// The user's sign-out, local only (differs from Phone Link on Windows): the refresh token and
    /// every cached access token are forgotten, every account record is deprovisioned and every
    /// identity cleared as at an account switch, and the local account, the per-environment account
    /// ids and the saved environment are deleted — the store keeps no key of this identity. The
    /// next start signs in as any Microsoft account. The caller stops whatever served the account
    /// first; an [`AccountRemoved`] broadcast reaches only live subscribers.
    pub fn sign_out(&self) -> Result<(), AuthError> {
        self.msa.sign_out()?;

        let mut st = self.state.lock_or_panic();
        self.forget_account_locked(&mut st, Deprovision::SignOut)?;

        let p = self.persist();
        st.account = None;
        p.delete(keys::ACCOUNT)?;

        for env in Environment::ALL {
            st.env_mut(env).account_id = None;
            self.persist_account_id(&st, env)?;
        }
        st.saved_env = None;
        self.persist_saved_env(&st)?;

        // Empty after the clearing; deleted rather than rewritten empty.
        for key in [keys::TRUST_RELATIONSHIPS, keys::ACCOUNT_INFO, keys::PEER_CACHE, keys::OWN_ENTRY] {
            p.delete(key)?;
        }

        tracing::info!("signed out; the Microsoft account and this identity's state are forgotten");
        Ok(())
    }

    /// Every account record deprovisioned (its relationships, trust certificate and peer cache) and
    /// every identity cleared with its relationships, as an account switch clears them.
    fn forget_account_locked(&self, st: &mut State, trigger: Deprovision) -> Result<(), AuthError> {
        let ids: Vec<String> = st.account_info.keys().cloned().collect();
        for id in ids {
            self.deprovision_locked(st, &id, trigger)?;
        }

        for env in Environment::ALL {
            if st.env(env).identity.is_some() {
                self.clear_identity_locked(st, env)?;
            }
        }

        Ok(())
    }

    /// The signed-in local account.
    pub fn local_account(&self) -> Option<LocalAccount> {
        self.state.lock_or_panic().account.clone()
    }

    /// The machine's device PUID from the device-bound sign-in's stored credential.
    pub fn device_puid(&self) -> Option<String> {
        self.msa.broker().device_puid().ok().flatten()
    }

    /// The MSA access token of `env`'s DCG scope.
    async fn msa_token(&self, env: Environment) -> Result<String, AuthError> {
        Ok(self.msa.access_token(env).await?)
    }

    /// A failed MSA-bearing call without a call-specific rule: a 401 also drops the cached MSA
    /// token so the next call fetches a fresh one.
    fn msa_call_failure(&self, env: Environment, e: RestError) -> AuthError {
        if e.is_unauthorized() {
            self.msa.invalidate(env);
        }
        AuthError::from_rest(e)
    }

    /// Resolves a partner id (`<id>` or `trust_<id>`) to its `DcgClient`.
    pub fn resolve_partner(&self, id: &str) -> Option<DcgClient> {
        let mut st = self.state.lock_or_panic();
        self.prune_trust_locked(&mut st);
        st.trust.resolve_partner(id)
    }

    /// The PC's own `DcgClient` once an account is enrolled or provisioned.
    pub fn own_client(&self) -> Option<DcgClient> {
        let st = self.state.lock_or_panic();
        let account = st.account.as_ref()?;
        let info = st.account_info.get(&account.account_id).filter(|i| i.is_provisioned())?;
        let env = info.environment?;
        st.env(env).identity.as_ref().map(|i| DcgClient::new(i.device_id.clone(), env))
    }

    /// The account's environment (the saved, snapped environment).
    pub fn environment(&self) -> Option<Environment> {
        self.state.lock_or_panic().saved_env
    }

    /// Every stored trust relationship (diagnostics, tests).
    pub fn trust_relationships(&self) -> Vec<TrustRelationship> {
        self.state.lock_or_panic().trust.records().to_vec()
    }

    /// The `deviceId` echo stored on the cached services token of `env` (diagnostics, tests).
    pub fn cached_token_device_id(&self, env: Environment) -> Option<String> {
        self.state.lock_or_panic().env(env).services_token.as_ref().map(|t| t.device_id.clone())
    }

    /// The account records (diagnostics, tests).
    pub fn account_info(&self) -> Option<AccountInfo> {
        let st = self.state.lock_or_panic();
        let account = st.account.as_ref()?;
        st.account_info.get(&account.account_id).cloned()
    }

    /// The account needs the user's attention: its MSA token cannot be refreshed without the user
    /// ([`MsaClient::needs_attention`]) until a sign-in or a later grant succeeds.
    pub fn needs_user_attention(&self) -> bool {
        self.msa.needs_attention()
    }

    /// Peer `Added` / `Removed` / `Updated` events of cloud refreshes.
    pub fn subscribe_peer_events(&self) -> broadcast::Receiver<PeerEvent> {
        self.peer_events.subscribe()
    }

    /// One [`DeviceListApplied`] per cloud refresh that applied the device list, changed or not.
    pub fn subscribe_device_list(&self) -> broadcast::Receiver<DeviceListApplied> {
        self.device_list_events.subscribe()
    }

    /// One [`AccountRemoved`] per deprovisioned account.
    pub fn subscribe_account_removed(&self) -> broadcast::Receiver<AccountRemoved> {
        self.account_events.subscribe()
    }

    /// `metadata.capabilities` changed (an options change): the next client-info refresh sends them.
    pub fn set_capabilities(&self, capabilities: Vec<String>) {
        self.state.lock_or_panic().capabilities = capabilities;
    }

    /// The wake sequence over `hub`.
    pub fn waker(self: &Arc<Self>, hub: Arc<dyn WakeHub>) -> Arc<dyn Waker> {
        WakeService::new(self.clone(), hub)
    }

    /// Inbound JWT validation of a data JWT a partner signed.
    pub fn validate_inbound_jwt(
        &self,
        token: &str,
        expected_scope: Option<&str>,
    ) -> Result<InboundJwt, InboundJwtError> {
        let now = self.now();
        let mut st = self.state.lock_or_panic();
        let before = st.trust.clone();
        let result = st.trust.validate_inbound_jwt(token, expected_scope, now, &self.policy);

        if st.trust != before {
            if let Err(e) = self.persist_trust(&st) {
                tracing::error!(error = %e, "persisting the trust store failed");
            }
        }
        result
    }

    // --------------------------------------------------------------------------------------------
    // Persistence (the caller holds the state lock)
    // --------------------------------------------------------------------------------------------

    fn persist(&self) -> Persist<'_> {
        Persist(&*self.edges.store)
    }

    fn persist_saved_env(&self, st: &State) -> Result<(), AuthError> {
        match st.saved_env {
            Some(env) => self.persist().put_bytes(keys::ENVIRONMENT, env.name().as_bytes().to_vec())?,
            None => self.persist().delete(keys::ENVIRONMENT)?,
        }
        Ok(())
    }

    /// The identity of `env`: key + certificate first, then the record, then the pending rotation
    /// key — an interrupted sequence leaves keys without a record (ignored at load), never a record
    /// whose key is gone.
    fn persist_identity(&self, st: &State, env: Environment) -> Result<(), AuthError> {
        self.persist_identity_pair(st, env)?;
        self.persist_identity_record(st, env)?;
        self.persist_pending_identity(st, env)
    }

    fn persist_identity_record(&self, st: &State, env: Environment) -> Result<(), AuthError> {
        match &st.env(env).identity {
            Some(record) => self.persist().put_json(&keys::identity(env), record)?,
            None => self.persist().delete(&keys::identity(env))?,
        }
        Ok(())
    }

    fn persist_identity_pair(&self, st: &State, env: Environment) -> Result<(), AuthError> {
        Self::persist_pair(&self.persist(), &keys::identity_pair(env), st.env(env).identity_keys.as_ref())
    }

    fn persist_pending_identity(&self, st: &State, env: Environment) -> Result<(), AuthError> {
        Self::persist_pair(
            &self.persist(),
            &keys::identity_pending_pair(env),
            st.env(env).pending_identity_keys.as_ref(),
        )
    }

    /// One key + certificate entry (written as one value, so never torn), or its deletion.
    fn persist_pair(p: &Persist<'_>, key: &str, pair: Option<&KeyPair>) -> Result<(), AuthError> {
        match pair {
            Some(pair) => p.put_key_pair(key, pair)?,
            None => p.delete(key)?,
        }
        Ok(())
    }

    fn persist_account_id(&self, st: &State, env: Environment) -> Result<(), AuthError> {
        match &st.env(env).account_id {
            Some(id) => self.persist().put_bytes(&keys::account_id(env), id.as_bytes().to_vec())?,
            None => self.persist().delete(&keys::account_id(env))?,
        }
        Ok(())
    }

    fn persist_trust_keys(&self, st: &State, env: Environment) -> Result<(), AuthError> {
        Self::persist_pair(&self.persist(), &keys::trust_pair(env), st.env(env).trust_keys.as_ref())
    }

    fn persist_trust(&self, st: &State) -> Result<(), AuthError> {
        self.persist().put_json(keys::TRUST_RELATIONSHIPS, &st.trust)?;
        Ok(())
    }

    fn persist_account_info(&self, st: &State) -> Result<(), AuthError> {
        self.persist().put_json(keys::ACCOUNT_INFO, &st.account_info)?;
        Ok(())
    }

    fn persist_peer_cache(&self, st: &State) -> Result<(), AuthError> {
        let p = self.persist();
        p.put_json(keys::PEER_CACHE, &st.peer_cache)?;
        match &st.own_entry {
            Some(entry) => p.put_json(keys::OWN_ENTRY, entry)?,
            None => p.delete(keys::OWN_ENTRY)?,
        }
        Ok(())
    }

    /// Prunes the trust store and persists it when anything was removed.
    fn prune_trust_locked(&self, st: &mut State) {
        if st.trust.prune(self.now(), &self.policy) {
            if let Err(e) = self.persist_trust(st) {
                tracing::error!(error = %e, "persisting the pruned trust store failed");
            }
        }
    }
}
