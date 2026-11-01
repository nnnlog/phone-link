//! Registers every platform route the engine answers, and hands back the APIs the feature work uses
//! (MSAEP, DRM, the context-source cache).
//!
//! - `/internal/response`: the router itself.
//! - `/PlatformDisconnectRequest` (inbound): [`super::session_control::PlatformDisconnectRoute`].
//! - `/CryptoTrustKeyRotationCheck`, `/CryptoTrustKeyRotation` (inbound): [`super::key_rotation`].
//! - `/Context/Publish`: [`super::msaep`].
//! - `/DeviceResourceManager`: [`super::drm`], registered with the first receiver.
//! - `/DeviceProxyClient/TransportMiddleware` (App): [`super::device_proxy`].
//! - `/Context/Message/{partnerId}`: [`super::context_message`], registered by
//!   [`super::session_validation`] while the partner has a context source.
//! - `/SessionValidation`, `/PlatformActivateSessionRequest`, the bare `/Context/Message` (inbound): not
//!   registered, so rejected `NotSubscribedToRoute`.

use std::sync::Arc;

use super::device_proxy::DeviceProxyReceiver;
use super::drm::Drm;
use super::key_rotation::{KeyRotationCheckRoute, KeyRotationRoute};
use super::msaep::{ContextPublishRoute, Msaep};
use super::route;
use super::router::Router;
use super::session::PartnerSessions;
use super::session_control::PlatformDisconnectRoute;
use super::session_validation::SessionValidator;
use phonelink_core::config::EngineConfig;
use phonelink_core::edge::{Clock, SessionLock};
use phonelink_core::seams::TrustGate;
use phonelink_core::types::DcgClient;

/// What [`PlatformRoutes::install`] needs.
pub struct PlatformDeps {
    pub config: Arc<EngineConfig>,
    pub router: Arc<Router>,
    pub sessions: Arc<PartnerSessions>,
    pub trust: Arc<dyn TrustGate>,
    /// The PC's own enrolled identity (`MsaepMessage.dcg_client_id`).
    pub own: DcgClient,
    /// The macOS lock test of the Cross Device Experience Host request wrapper.
    pub lock: Arc<dyn SessionLock>,
    /// Wall clock for `TimeToExpireInSeconds` and the expiry of a rotated partner certificate.
    pub clock: Arc<dyn Clock>,
}

/// The installed platform routes and the handles the feature layers use.
pub struct PlatformRoutes {
    /// `/SessionValidation` and the per-partner context sources.
    pub validator: Arc<SessionValidator>,
    /// MSAEP `subscribe` / `publish`.
    pub msaep: Arc<Msaep>,
    /// DRM `register_receiver` / `send` (the request wrapper).
    pub drm: Arc<Drm>,
}

impl PlatformRoutes {
    /// Registers the routes on `deps.router` and starts validating partners on connect. Call it once,
    /// inside the tokio runtime; a second call panics (each route has a single handler).
    pub fn install(deps: PlatformDeps) -> Arc<PlatformRoutes> {
        let PlatformDeps { config, router, sessions, trust, own, lock, clock } = deps;

        let validator = SessionValidator::new(config.clone(), router.clone(), sessions.clone(), own.clone());
        validator.spawn_session_listener();
        let msaep = Msaep::new(config.clone(), router.clone(), sessions.clone(), validator.clone(), own);
        let drm = Drm::new(config, router.clone(), sessions.clone(), lock);

        let register = |route: &str, handler: Arc<dyn super::router::PlatformRouteHandler>| {
            router.register_route(route, handler).unwrap_or_else(|e| panic!("installing the platform routes: {e}"));
        };

        register(route::PLATFORM_DISCONNECT_REQUEST, Arc::new(PlatformDisconnectRoute::new(sessions)));
        register(
            route::CRYPTO_TRUST_KEY_ROTATION_CHECK,
            Arc::new(KeyRotationCheckRoute::new(router.clone(), trust.clone(), clock.clone())),
        );
        register(route::CRYPTO_TRUST_KEY_ROTATION, Arc::new(KeyRotationRoute::new(router.clone(), trust, clock)));
        register(route::CONTEXT_PUBLISH, Arc::new(ContextPublishRoute(msaep.clone())));
        router.set_device_proxy_receiver(Arc::new(DeviceProxyReceiver::new(router.clone(), drm.clone())));

        Arc::new(PlatformRoutes { validator, msaep, drm })
    }
}
