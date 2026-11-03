//! The engine: builds every layer, wires them, runs the start sequence, and exports the handle
//! `gui/` holds through `uniffi`.
//!
//! Wiring: the relay hands packets to the fragment layer; the fragment layer opens the relay's hub
//! connection before each fragment try and ack, allocates from the process's identifiers, hands
//! reassembled messages to the router and asks the per-partner connection to wake the phone for sends
//! that may wake; the router hands App messages to the App layer; the App layer sends through the router and
//! waits for the connection through the per-partner connection; the auth layer supplies the services
//! token to the relay, the trust gate to the fragment layer and the routes, and the wake. The
//! features ([`crate::features`]) are installed once the platform routes exist and before the App
//! layer starts, so the engine-start connect carries their providers.
//!
//! While it runs, the engine keeps the phone record current (every device-list refresh rewrites it),
//! bridges the clipboard's global switch to the advertised capabilities, and reacts to the removal
//! of the account: the features stop and the core must be rebuilt ([`EngineCore::needs_rebuild`]).

use crate::app::ConnectTrigger;
use crate::features::clipboard::{global_switch_persisted, with_clipboard_capability};
use crate::features::file_transfer::with_file_transfer_capability;
use crate::features::remote_control::with_remote_control_capabilities;
use crate::seams::{PartnerSessionControl, PlatformMessenger};
use crate::tap::LabeledTap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tracing::{info, warn};

use crate::app::AppLayer;
use crate::app::phone_record::PHONE_RECORD_KEY_PREFIX;
use crate::auth::live::broker::LiveBroker;
use crate::auth::pairing::PairingManager;
use crate::auth::{AccountRemoved, AuthService, PeerDevice};
use crate::config::EngineConfig;
use crate::dcg::DcgTransport;
use crate::edge::{EdgeError, Edges};
use crate::features::shared::peers::{DisplayedPeers, is_phone};
use crate::features::{Features, NotificationRouter, PhoneSwitches, delete_phone_stores};
use crate::identification::ClientIdentification;
use crate::platform::{ClientVersion, PartnerSessions, PlatformRoutes, Router};
use crate::relay::RelayManager;
use crate::seams::Waker;

mod bridges;
mod cdeh;
mod devices;
mod install;
mod phones;
mod system;

mod events;
mod wiring;

use cdeh::{CdehTrigger, SessionChanges};
pub use wiring::IdentityStack;
pub(crate) use wiring::build_broker;
use wiring::{CorePhoneLookup, EnrolledAccount, SharedConnection, install_routes, primary_edges};

use crate::tap::{Tap, null_tap};
use crate::types::DcgClient;
pub use events::{CoreEvent, CoreEvents};

// What an embedder (the CLI) needs from the layers beneath to build and drive a core.
pub use crate::auth::FetchMode;
pub use crate::features::installed::FeatureEdges;
pub use crate::features::shared::db::StoreLocation;
pub use crate::features::shared::notifier::{SystemNotification, SystemNotifier};

/// Stand-ins for the features' OS edges that the headless CLI runs with.
pub mod standins {
    pub use crate::features::device_status::edge::standins::{FixedPcStatus, PcState};

    pub use crate::features::remote_control::edge::RefusingPowerControl;
}

/// Why the engine could not start or carry out a request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum EngineError {
    /// Sign-in, enrollment or trust synchronization failed.
    #[error("auth: {message}")]
    Auth { message: String },
    /// The account's device list has no linked phone (or not the one asked for).
    #[error("no linked phone in the account's device list")]
    NoPhone,
    /// A local precondition broke (store, configuration, runtime).
    #[error("setup: {message}")]
    Setup { message: String },
    #[error("the engine is not started")]
    NotStarted,
}

fn auth_err(e: impl std::fmt::Display) -> EngineError {
    EngineError::Auth { message: e.to_string() }
}

fn setup_err(e: impl std::fmt::Display) -> EngineError {
    EngineError::Setup { message: e.to_string() }
}

impl From<EdgeError> for EngineError {
    fn from(e: EdgeError) -> Self {
        setup_err(e)
    }
}

/// What a start established.
#[derive(Debug, Clone)]
pub struct Started {
    pub own: DcgClient,
    /// The selected phone; `None` while the user has not selected one.
    pub phone: Option<PeerDevice>,
    pub peers: Vec<PeerDevice>,
}

/// Store prefix of the primary identity's keys when a CDEH identity is enrolled beside it; the CDEH
/// identity keeps the unprefixed keys (those of an install that enrolled `WEA` alone).
const PRIMARY_STORE_PREFIX: &str = "pl/";

/// Store key of the selected phone's `dcgClientId`.
pub const SELECTED_PHONE_KEY: &str = "app/selected-phone";

/// The user's sign-out on the stored state (differs from Phone Link on Windows).
///
/// Every identity of `config` signs out ([`AuthService::sign_out`]), then the selected phone, the
/// App layer's phone records, every phone's feature stores and the per-phone switches are deleted.
///
/// The machine's
/// logical device id, the App layer's installation id and the settings that belong to no phone
/// stay. Contacts nothing; whatever served the account must be stopped first.
pub fn sign_out_stored(config: &EngineConfig, edges: &Edges, stores: &StoreLocation) -> Result<(), EngineError> {
    let primary = Arc::new(config.clone());
    let mut identities = vec![(primary.clone(), primary_edges(edges, config.identity.cdeh.is_some()))];
    if let Some(cdeh) = config.cdeh_config() {
        identities.push((Arc::new(cdeh), edges.clone()));
    }

    let broker = build_broker(edges)?;
    for (config, identity_edges) in identities {
        let ident = Arc::new(ClientIdentification::load_or_create(&config, edges.store.as_ref())?);
        let auth = AuthService::new(config, identity_edges, ident, null_tap(), broker.clone()).map_err(auth_err)?;
        auth.sign_out().map_err(auth_err)?;
    }

    edges.store.delete(SELECTED_PHONE_KEY.into())?;
    for key in edges.store.keys_with_prefix(PHONE_RECORD_KEY_PREFIX.into())? {
        edges.store.delete(key)?;
    }
    delete_phone_stores(stores).map_err(setup_err)?;
    PhoneSwitches::forget_all(stores).map_err(setup_err)?;

    info!("signed out; the account's state and phone data are deleted");
    Ok(())
}

/// Every layer, wired. Build it inside a tokio runtime (the fragment layer starts its receive task);
/// building contacts nothing — [`EngineCore::start`] does.
///
/// `ident` and `auth` are the primary identity's, whose App layer and App-layer features `app` runs;
/// `relay` … `sessions` and `waker` are the connection's. With a CDEH identity configured
/// (`config.identity.cdeh`), [`EngineCore::cdeh`] is that identity's stack and the connection is its
/// own: it carries the CDEH features (clipboard, device status's CDEH contract) and the App layer's
/// messages, while the primary identity opens no connection. Without one the primary identity's stack
/// is the connection and carries everything.
pub struct EngineCore {
    pub config: Arc<EngineConfig>,
    pub edges: Edges,
    /// Where the core reports what changed.
    events: Arc<dyn CoreEvents>,
    pub tap: Tap,
    pub ident: Arc<ClientIdentification>,
    pub auth: Arc<AuthService>,
    pub relay: Arc<RelayManager>,
    pub dcg: Arc<DcgTransport>,
    pub router: Arc<Router>,
    pub sessions: Arc<PartnerSessions>,
    pub app: Arc<AppLayer>,
    /// The CDEH identity's stack, when one is configured.
    pub cdeh: Option<Arc<IdentityStack>>,
    /// The account's QR/PIN pairing sessions, over the primary identity's account.
    pub pairing: Arc<PairingManager>,
    /// The wake sequence the per-partner connection uses; the engine resets its breakers on unlock
    /// and wake.
    waker: Arc<dyn Waker>,
    /// The per-phone "enabled" switch of the CDEH features.
    pub switches: Arc<PhoneSwitches>,
    /// Where the responses to the features' system notifications go.
    pub responses: Arc<NotificationRouter>,
    feature_edges: FeatureEdges,
    /// The CDEH identity's `config.auth.capabilities` as configured (the primary's without a CDEH
    /// identity), without the feature-switched ones.
    base_capabilities: Vec<String>,
    /// The runtime the engine was built in; entry points the foreign side calls synchronously (on
    /// its own threads) enter it, so what they start runs there.
    runtime: tokio::runtime::Handle,
    routes: OnceLock<Arc<PlatformRoutes>>,
    features: OnceLock<Arc<Features>>,
    /// A feature install failed part-way: some features are registered on this core's App layer and
    /// platform routes, so it cannot install them again — the core must be rebuilt.
    install_failed: AtomicBool,
    /// The account was removed while the features ran: they are stopped, so the core must be
    /// rebuilt (a new core installs them for the account the next start provisions).
    account_removed: AtomicBool,
    /// The selected phone left the account's linked devices (a device-list refresh no longer lists
    /// it): App requests fail at the account gate until a start selects a phone again.
    phone_unlinked: Arc<AtomicBool>,
    phone: Mutex<Option<DcgClient>>,
    /// The CDEH features' view of the account's peers: the selected phone only.
    displayed_peers: OnceLock<Arc<DisplayedPeers>>,
    /// The starts and stops of the CDEH identity's session, in order.
    cdeh_changes: Arc<SessionChanges>,
    /// The phone's oldest Link to Windows version a CDEH session start accepts.
    minimum_client_version: ClientVersion,
    started: AtomicBool,
}

impl EngineCore {
    /// Builds the core from its configuration and edges without connecting anything.
    ///
    /// [`EngineCore::start`] connects.
    pub fn build(
        config: EngineConfig,
        edges: Edges,
        feature_edges: FeatureEdges,
        tap: Tap,
        events: Arc<dyn CoreEvents>,
    ) -> Result<Arc<Self>, EngineError> {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return Err(setup_err("the engine must be built inside a tokio runtime"));
        };

        let config = Arc::new(config);

        // One broker, shared by both identities: it owns the machine's device credential and the one
        // signed-in user session, keyed in the base (unprefixed) store.
        let broker = build_broker(&edges)?;
        let cdeh = match config.cdeh_config() {
            Some(cdeh_config) => Some(Arc::new(IdentityStack::build(
                Arc::new(cdeh_config),
                &edges,
                edges.store.as_ref(),
                &tap,
                &broker,
            )?)),
            None => None,
        };

        let primary_edges = primary_edges(&edges, cdeh.is_some());
        // With a CDEH identity the primary one keeps only its sign-in, enrollment, trust and device list, and
        // the App layer runs over the CDEH identity's connection. The phone then serves the primary
        // identity's features to the CDEH client, whose `connect` tells it that the PC shares one connection.
        let PrimaryLayers { ident, auth, relay, dcg, router, sessions, waker } =
            primary_layers(&config, &edges, &primary_edges, cdeh.as_deref(), &tap, &broker)?;

        // The PC advertises `CLIPBOARD` iff the clipboard's global switch is on, from the first
        // enrollment on — on the identity that carries the clipboard.
        let cdeh_auth = cdeh.as_ref().map_or(&auth, |s| &s.auth);
        let base_capabilities = advertise_capabilities(cdeh_auth, &feature_edges)?;

        let switches = PhoneSwitches::open(&feature_edges.stores).map_err(setup_err)?;
        let phone_unlinked = Arc::new(AtomicBool::new(false));

        let app_sessions: Arc<dyn PartnerSessionControl> = match &cdeh {
            Some(_) => Arc::new(SharedConnection { sessions: sessions.clone(), switches: switches.clone() }),
            None => sessions.clone(),
        };
        let app = AppLayer::new(
            config.clone(),
            edges.clone(),
            router.clone() as Arc<dyn PlatformMessenger>,
            app_sessions,
            Arc::new(EnrolledAccount { auth: auth.clone(), phone_unlinked: phone_unlinked.clone() }),
            LabeledTap::wrap(tap.clone(), config.identity.profile.client_type),
        )
        .map_err(setup_err)?;
        router.set_app_inbound(app.clone());

        let cdeh_changes = SessionChanges::new(cdeh.as_ref().map_or("", |s| s.config.identity.profile.client_type));
        let minimum_client_version = minimum_client_version(&config)?;
        let pairing = PairingManager::new(
            auth.clone(),
            config.auth.pairing.clone(),
            edges.clock.clone(),
            edges.reachability.clone(),
            edges.host.clone(),
        );

        Ok(Arc::new(Self {
            config,
            edges,
            events,
            tap,
            ident,
            auth,
            pairing,
            relay,
            dcg,
            router,
            sessions,
            app,
            cdeh,
            waker,
            switches,
            responses: NotificationRouter::new(),
            feature_edges,
            base_capabilities,
            runtime,
            routes: OnceLock::new(),
            features: OnceLock::new(),
            install_failed: AtomicBool::new(false),
            account_removed: AtomicBool::new(false),
            phone_unlinked,
            phone: Mutex::new(None),
            displayed_peers: OnceLock::new(),
            cdeh_changes,
            minimum_client_version,
            started: AtomicBool::new(false),
        }))
    }

    /// The start sequence (startup and provisioning, then the relay and the phone session): sign-in
    /// (the device code only when no refresh token works), provisioning, the platform routes, trust
    /// synchronization, phone selection, and the per-partner connection and App layer — whose
    /// engine-start connect brings the phone up.
    /// `phone_id` picks a phone when the account has several. A core that needs a rebuild
    /// ([`EngineCore::needs_rebuild`]) does not start; neither does a restarted core whose account
    /// this start's provisioning or trust synchronization removed (it then needs a rebuild).
    pub async fn start(self: &Arc<Self>, phone_id: Option<&str>) -> Result<Started, EngineError> {
        if self.needs_rebuild() {
            return Err(setup_err("this engine core cannot start again; it must be rebuilt"));
        }

        log_start(self.started.load(Ordering::SeqCst));
        let mut removed = self.auth.subscribe_account_removed();
        self.auth.sign_in().await.map_err(auth_err)?;
        let provisioned = self.auth.ensure_provisioned().await.map_err(auth_err)?;

        // The connection's platform routes: the CDEH identity installs its own at its provisioning.
        if self.cdeh.is_none() {
            install_routes(
                &self.routes,
                &self.config,
                &self.router,
                &self.sessions,
                &self.auth,
                &provisioned.own,
                &self.edges,
            );
        }

        let peers = self.auth.sync_trust(FetchMode::Default).await.map_err(auth_err)?;

        // The installed features served the account this start removed (deprovisioned while it was
        // provisioned again): they stop, and this core cannot go on.
        if self.features.get().is_some() && account_removed(&mut removed) {
            self.on_account_removed();
            return Err(setup_err("the account was removed; the engine must be rebuilt"));
        }

        if let Some(cdeh) = &self.cdeh {
            self.start_cdeh(cdeh).await?;
        }

        // The phone is the user's selection, never chosen automatically; `phone_id` (the CLI) selects
        // that phone.
        let wanted = match phone_id {
            Some(id) => Some(id.to_string()),
            None => self.persisted_phone()?,
        };
        let Some(wanted) = wanted else {
            info!("no phone selected yet; waiting for the user's selection");
            return Ok(Started { own: provisioned.own, phone: None, peers });
        };

        let phone = match linked_phone(&peers, &wanted) {
            Ok(phone) => phone,
            Err(_) => {
                warn!("the selected phone is no longer a linked phone of the account; the user selects again");
                return Ok(Started { own: provisioned.own, phone: None, peers });
            }
        };

        self.persist_phone(&phone.client)?;
        self.begin_phone(&phone)?;
        Ok(Started { own: provisioned.own, phone: Some(phone), peers })
    }

    /// Starts serving `phone` (the first time) or serves it again after a stop: it becomes the App
    /// layer's phone, the features are installed, the sessions start, and its sessions open.
    fn begin_phone(self: &Arc<Self>, phone: &PeerDevice) -> Result<(), EngineError> {
        info!(phone = %phone.client, "phone selected");
        self.select(&phone.client, &phone.info)?;
        self.install_features()?;

        if !self.started.swap(true, Ordering::SeqCst) {
            self.sessions.start();
            self.spawn_event_bridge();
            self.app.start().map_err(setup_err)?;
        } else {
            // A start after `stop` is an app start with a linked phone, a connect trigger: the
            // background connect brings the session back.
            self.app.trigger(ConnectTrigger::EngineStart);
        }

        self.connect_cdeh(CdehTrigger::ProcessSessionChanges);

        if crate::nano::probe::enabled() {
            let (auth, sessions, routes) = self.cdeh_parts();
            if let Some(routes) = routes {
                let (config, client) = (self.config.nano.clone(), phone.client.clone());
                tokio::spawn(crate::nano::probe::run(config, auth, sessions, routes, client));
            }
        }
        Ok(())
    }

    // --------------------------------------------------------------------------------------------
    // State
    // --------------------------------------------------------------------------------------------

    /// The installed features (after the first [`EngineCore::start`]).
    pub fn features(&self) -> Option<Arc<Features>> {
        self.features.get().cloned()
    }

    /// This core cannot start again; build a new one: a feature install failed part-way, or the
    /// account was removed while the features ran.
    pub fn needs_rebuild(&self) -> bool {
        self.install_failed.load(Ordering::SeqCst) || self.account_removed.load(Ordering::SeqCst)
    }

    /// The app came to the foreground (`true`) or left it (`false`) — macOS: the application became or
    /// resigned active ([`AppLayer::foreground_changed`]: `isForeground` of the trace state, and the
    /// immediate connect of "app came to the foreground" — sent by the App layer's start when the
    /// change comes while this core is starting; and call control's reconnect). The foreign side calls this on its own thread; it
    /// runs inside the engine's runtime.
    pub fn set_foreground(&self, foreground: bool) {
        let _runtime = self.runtime.enter();
        self.app.foreground_changed(foreground);
        // Call control sets up a failed or lost Bluetooth connection again.
        if let Some(features) = self.features().filter(|_| foreground) {
            features.calling.foreground();
        }
    }

    /// Enters the runtime the engine was built in, for calls the foreign side makes on its own
    /// threads.
    pub fn enter_runtime(&self) -> tokio::runtime::EnterGuard<'_> {
        self.runtime.enter()
    }

    /// The runtime the engine was built in, for work a foreign caller awaits.
    pub fn runtime_handle(&self) -> tokio::runtime::Handle {
        self.runtime.clone()
    }

    // --------------------------------------------------------------------------------------------
    // State
    // --------------------------------------------------------------------------------------------

    /// The connection's platform routes: the CDEH identity's when there is one.
    pub fn routes(&self) -> Option<Arc<PlatformRoutes>> {
        match &self.cdeh {
            Some(cdeh) => cdeh.routes(),
            None => self.routes.get().cloned(),
        }
    }

    /// Whether the selected phone's session is connected now.
    pub fn phone_connected(&self) -> bool {
        self.phone().is_some_and(|p| self.sessions.is_connected(&p))
    }

    /// Ends the phone session: cancels in-flight connects, as the disconnect trigger does, then
    /// `/PlatformDisconnectRequest` and the hub connection.
    pub async fn stop(&self) {
        info!("engine stopping; in-flight connects are canceled and the phone disconnected");
        self.app.cancel_connects();
        if let Some(phone) = self.phone() {
            self.sessions.disconnect(&phone).await;
        }
    }

    /// The user signed out: the phone session ends as at [`EngineCore::stop`], the clipboard stops
    /// and sends nothing, and this core cannot start again — the next start builds a new one.
    pub async fn retire(&self) {
        self.account_removed.store(true, Ordering::SeqCst);
        self.stop().await;
        if let Some(features) = self.features() {
            features.clipboard.shutdown();
        }
    }
}

/// The primary identity's layers: with a CDEH identity, its own identification and auth over the
/// CDEH identity's connection layers; without one, a full stack of its own.
struct PrimaryLayers {
    ident: Arc<ClientIdentification>,
    auth: Arc<AuthService>,
    relay: Arc<RelayManager>,
    dcg: Arc<DcgTransport>,
    router: Arc<Router>,
    sessions: Arc<PartnerSessions>,
    waker: Arc<dyn Waker>,
}

/// Builds the primary identity's layers (see [`PrimaryLayers`]).
fn primary_layers(
    config: &Arc<EngineConfig>,
    edges: &Edges,
    primary_edges: &Edges,
    cdeh: Option<&IdentityStack>,
    tap: &Tap,
    broker: &Arc<LiveBroker>,
) -> Result<PrimaryLayers, EngineError> {
    match cdeh {
        Some(stack) => {
            let ident = Arc::new(ClientIdentification::load_or_create(config, edges.store.as_ref())?);
            let auth = AuthService::new(
                config.clone(),
                primary_edges.clone(),
                ident.clone(),
                LabeledTap::wrap(tap.clone(), config.identity.profile.client_type),
                broker.clone(),
            )
            .map_err(auth_err)?;

            Ok(PrimaryLayers {
                ident,
                auth,
                relay: stack.relay.clone(),
                dcg: stack.dcg.clone(),
                router: stack.router.clone(),
                sessions: stack.sessions.clone(),
                waker: stack.waker.clone(),
            })
        }
        None => {
            let IdentityStack { config: _, ident, auth, relay, dcg, router, sessions, waker, routes: _ } =
                IdentityStack::build(config.clone(), primary_edges, edges.store.as_ref(), tap, broker)?;
            Ok(PrimaryLayers { ident, auth, relay, dcg, router, sessions, waker })
        }
    }
}

/// Sets the advertised capabilities on `cdeh_auth` (the base ones, with `CLIPBOARD` when the
/// clipboard's global switch is on) and returns the base ones.
fn advertise_capabilities(cdeh_auth: &AuthService, feature_edges: &FeatureEdges) -> Result<Vec<String>, EngineError> {
    // `REMOTE_CONTROL` and `REMOTE_CONTROL_V2` as the remote-control options allow.
    // `FILE_TRANSFER` when the Mac receives files from the phone.
    let base_capabilities = with_file_transfer_capability(
        &with_remote_control_capabilities(
            &cdeh_auth.config().auth.capabilities,
            &cdeh_auth.config().features.remote_control,
        ),
        &cdeh_auth.config().features.file_transfer,
    );

    let clipboard_on = global_switch_persisted(&feature_edges.stores).map_err(setup_err)?;
    cdeh_auth.set_capabilities(with_clipboard_capability(&base_capabilities, clipboard_on));
    Ok(base_capabilities)
}

/// The configured oldest Link to Windows version a CDEH session start accepts.
fn minimum_client_version(config: &EngineConfig) -> Result<ClientVersion, EngineError> {
    ClientVersion::parse(&config.platform.drm_minimum_client_version).ok_or_else(|| {
        setup_err(format!(
            "the minimum client version {:?} is not a version",
            config.platform.drm_minimum_client_version
        ))
    })
}

/// Whether `removed` saw an account removal (a lagged receiver may have lost one).
fn account_removed(removed: &mut tokio::sync::broadcast::Receiver<AccountRemoved>) -> bool {
    matches!(removed.try_recv(), Ok(_) | Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)))
}

/// The linked phone whose id is `id` (ASCII case-insensitive): a linked peer is a phone when its
/// `clientType` is `LTW` (Link to Windows) or its `osName` is Android. `NoPhone` otherwise.
fn linked_phone(peers: &[PeerDevice], id: &str) -> Result<PeerDevice, EngineError> {
    peers
        .iter()
        .find(|p| p.client.id.eq_ignore_ascii_case(id) && is_phone(&p.info))
        .cloned()
        .ok_or(EngineError::NoPhone)
}

// ------------------------------------------------------------------------------------------------
// uniffi export
// ------------------------------------------------------------------------------------------------

/// A change of the Mac's session or power state the engine reacts to ([`EngineCore::system_event`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum SystemEvent {
    ScreenLocked,
    ScreenUnlocked,
    WillSleep,
    DidWake,
}

fn log_start(is_restart: bool) {
    info!(is_restart, "engine starting");
}
