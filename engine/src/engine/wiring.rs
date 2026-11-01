//! How one identity's layers are built and wired, and the adapters between the core and the layers it joins.

use async_trait::async_trait;

use super::{EngineCore, EngineError, PRIMARY_STORE_PREFIX, auth_err, setup_err};
use crate::app::AccountGate;
use crate::auth::AuthService;
use crate::auth::live::broker::{HttpTransport, LiveBroker};
use crate::auth::pairing::PhoneLookup;
use crate::auth::store::AccountState;
use crate::config::EngineConfig;
use crate::dcg::{DcgIdentifiers, DcgTransport, HubOpener};
use crate::edge::{Edges, PrefixedStore, SecureStore};
use crate::features::PhoneSwitches;
use crate::identification::ClientIdentification;
use crate::platform::{PartnerSessions, PlatformDeps, PlatformRoutes, Router};
use crate::relay::RelayManager;
use crate::seams::{
    ConnectOutcome, ConnectRequest, HubControl, HubLink, MessageTransport, PartnerSessionControl, SessionEvent,
    TokenSource, TrustGate, WakeHub, Waker,
};
use crate::tap::{LabeledTap, Tap};
use crate::types::DcgClient;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use tracing::{debug, error};

/// The App layer's account gate, checked first when an App request is sent (the selected phone left
/// the account's linked devices, the enrolled account is missing, or its token cannot be
/// refreshed). The engine has one account, and its phones come from that account's device list.
pub(super) struct EnrolledAccount {
    pub(super) auth: Arc<AuthService>,
    /// The selected phone left the account's linked devices; set until a start selects a phone
    /// again ([`EngineCore::phone_unlinked`]).
    pub(super) phone_unlinked: Arc<AtomicBool>,
}

#[async_trait]
impl AccountGate for EnrolledAccount {
    async fn is_ready(&self, _phone: &DcgClient) -> bool {
        !self.phone_unlinked.load(Ordering::SeqCst)
            && self.auth.account_info().is_some_and(|a| a.state == AccountState::Enrolled)
            && !self.auth.needs_user_attention()
    }
}

/// One PC identity's stack: its client identification, auth, relay, fragment layer, router and
/// per-partner connections, and — once provisioned — its platform routes.
///
/// The engine builds one for its primary identity and, when the configuration has one, one for the
/// CDEH identity beside it.
pub struct IdentityStack {
    pub config: Arc<EngineConfig>,
    pub ident: Arc<ClientIdentification>,
    pub auth: Arc<AuthService>,
    pub relay: Arc<RelayManager>,
    pub dcg: Arc<DcgTransport>,
    pub router: Arc<Router>,
    pub sessions: Arc<PartnerSessions>,
    pub(super) waker: Arc<dyn Waker>,
    pub(super) routes: OnceLock<Arc<PlatformRoutes>>,
}

impl IdentityStack {
    /// Wires one identity's layers: its keys in `edges.store`, the machine's logical device id in
    /// `shared`. Contacts nothing.
    pub(super) fn build(
        config: Arc<EngineConfig>,
        edges: &Edges,
        shared: &dyn SecureStore,
        tap: &Tap,
        broker: &Arc<LiveBroker>,
    ) -> Result<Self, EngineError> {
        let tap = &LabeledTap::wrap(tap.clone(), config.identity.profile.client_type);
        let ident = Arc::new(ClientIdentification::load_or_create(&config, shared)?);
        let auth = AuthService::new(config.clone(), edges.clone(), ident.clone(), tap.clone(), broker.clone())
            .map_err(auth_err)?;

        let relay = RelayManager::new(
            config.clone(),
            ident.clone(),
            auth.clone() as Arc<dyn TokenSource>,
            edges.reachability.clone(),
            tap.clone(),
        );

        // The fragment layer's identifiers belong to the process and the identity: a core rebuilt in
        // this process continues the `MessageId` and `SequenceNumber` counters and keeps each
        // `SessionId`.
        let dcg = DcgTransport::new(
            config.clone(),
            relay.clone() as Arc<dyn HubLink>,
            relay.clone() as Arc<dyn HubOpener>,
            auth.clone() as Arc<dyn TrustGate>,
            tap.clone(),
            DcgIdentifiers::process(config.identity.profile.client_type),
        );
        relay.set_packet_sink(dcg.clone());

        let router = Router::new(config.clone(), dcg.clone() as Arc<dyn MessageTransport>, tap.clone());
        dcg.set_message_sink(router.clone());

        let waker = auth.waker(relay.clone() as Arc<dyn WakeHub>);
        let sessions = PartnerSessions::new(
            config.clone(),
            relay.clone() as Arc<dyn HubControl>,
            waker.clone(),
            router.clone(),
            tap.clone(),
        );
        dcg.set_partner_connector(sessions.clone());

        Ok(Self { config, ident, auth, relay, dcg, router, sessions, waker, routes: OnceLock::new() })
    }

    /// Sign-in (the device code only when no refresh token works), provisioning, and — the first
    /// time — the platform routes: the startup and provisioning sequence. Returns this identity's own
    /// client.
    pub(super) async fn provision(&self, edges: &Edges) -> Result<DcgClient, EngineError> {
        self.auth.sign_in().await.map_err(auth_err)?;
        let provisioned = self.auth.ensure_provisioned().await.map_err(auth_err)?;
        install_routes(&self.routes, &self.config, &self.router, &self.sessions, &self.auth, &provisioned.own, edges);
        Ok(provisioned.own)
    }

    /// The platform routes of the primary identity's connection, once installed.
    pub fn routes(&self) -> Option<Arc<PlatformRoutes>> {
        self.routes.get().cloned()
    }
}

/// Builds the shared device-bound sign-in broker from the base edges. Its `InlineLogin` runs under
/// the `PL` app id; it reads and writes the base (unprefixed) store.
pub(crate) fn build_broker(edges: &Edges) -> Result<Arc<LiveBroker>, EngineError> {
    let transport: Arc<dyn crate::edge::TokenTransport> = match &edges.token_transport {
        Some(t) => t.clone(),
        None => {
            let http =
                crate::auth::dcg_rest::DcgRest::http_client().map_err(|e| setup_err(format!("HTTP client: {e}")))?;
            Arc::new(HttpTransport::new(http, edges.reachability.clone()))
        }
    };

    Ok(Arc::new(LiveBroker::new(
        transport,
        edges.store.clone(),
        edges.clock.clone(),
        edges.web_sign_in.clone(),
        crate::constants::PROFILE_PL.msa_client_id.to_string(),
    )))
}

/// The primary identity's edges: with a CDEH identity beside it, its keys are prefixed; the App
/// layer's and the machine's keys are not.
pub(super) fn primary_edges(edges: &Edges, with_cdeh: bool) -> Edges {
    if with_cdeh {
        Edges { store: Arc::new(PrefixedStore::new(edges.store.clone(), PRIMARY_STORE_PREFIX)), ..edges.clone() }
    } else {
        edges.clone()
    }
}

/// Installs an identity's platform routes once.
pub(super) fn install_routes(
    slot: &OnceLock<Arc<PlatformRoutes>>,
    config: &Arc<EngineConfig>,
    router: &Arc<Router>,
    sessions: &Arc<PartnerSessions>,
    auth: &Arc<AuthService>,
    own: &DcgClient,
    edges: &Edges,
) {
    if slot.get().is_none() {
        let routes = PlatformRoutes::install(PlatformDeps {
            config: config.clone(),
            router: router.clone(),
            sessions: sessions.clone(),
            trust: auth.clone() as Arc<dyn TrustGate>,
            own: own.clone(),
            lock: edges.lock.clone(),
            clock: edges.clock.clone(),
        });
        let _ = slot.set(routes);
    }
}

/// The pairing session's view of the core that runs it.
pub(super) struct CorePhoneLookup(pub(super) std::sync::Weak<EngineCore>);

#[async_trait]
impl PhoneLookup for CorePhoneLookup {
    async fn learn_phone(&self, phone_id: &str) -> bool {
        match self.0.upgrade() {
            Some(core) => core.learn_paired_phone(phone_id).await,
            None => false,
        }
    }
}

/// The App layer's view of the CDEH identity's connection it rides: a connect to a phone whose
/// per-phone switch is off starts nothing and reports the connection as it is, *connected* or a
/// window that ended with no failure — the switch keeps the CDEH session down, and the App layer
/// cannot start it.
pub(super) struct SharedConnection {
    pub(super) sessions: Arc<PartnerSessions>,
    pub(super) switches: Arc<PhoneSwitches>,
}

#[async_trait]
impl PartnerSessionControl for SharedConnection {
    async fn connect(&self, partner: &DcgClient, request: ConnectRequest) -> ConnectOutcome {
        match self.switches.is_enabled(partner) {
            Ok(true) => PartnerSessionControl::connect(&*self.sessions, partner, request).await,
            Ok(false) => {
                debug!(%partner, "the phone is disabled: the App layer's connect starts nothing");
                if self.sessions.is_connected(partner) {
                    ConnectOutcome::Connected
                } else {
                    ConnectOutcome::Pending { last_failure: None }
                }
            }
            Err(e) => {
                error!(%partner, error = %e, "the per-phone switch could not be read; the connect starts nothing");
                ConnectOutcome::Pending { last_failure: None }
            }
        }
    }

    fn is_connected(&self, partner: &DcgClient) -> bool {
        self.sessions.is_connected(partner)
    }

    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<SessionEvent> {
        PartnerSessionControl::subscribe(&*self.sessions)
    }
}
