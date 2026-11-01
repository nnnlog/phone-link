//! `/SessionValidation`, its sequence, and the per-partner context source.
//!
//! The PC is the requester only (the route has no PC subscriber, so the router rejects an inbound one).
//! Validation is gated by `PlatformConfig::session_validation_enabled` (on by default), runs once per
//! partner connect, is shared by concurrent callers, is cached per partner on success, and is never
//! retried inside the handshake; it never tears down the session.
//!
//! A context source exists for a partner from a successful validation, or from
//! [`SessionValidator::context_source`], which creates (and caches) one without validation whenever the
//! gate is off or validation does not succeed — "not connected" included. While a source exists, its
//! partner's `/Context/Message/{partnerId}` route is registered ([`super::context_message`]); it is
//! removed with the source when the partner connects anew or disconnects.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use futures_util::future::{BoxFuture, FutureExt, Shared};
use prost::Message;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::context_message::{ContextHandle, ContextHub, ContextMessageRoute, partner_route};
use super::router::Router;
use super::session::PartnerSessions;
use super::{priority, route};
use phonelink_core::config::EngineConfig;
use phonelink_core::seams::{
    PartnerSessionControl, PlatformMessenger, PlatformRequest, PlatformRequestStatus, PlatformResponse, SessionEvent,
};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::{DcgClient, HeaderMap, MessageType, SendOptions, TraceContext, TransportMessage, header};
use phonelink_proto::platform_sdk::v1::{
    ProtoPlatformCapabilities, ProtoPlatformSessionValidationRequest, ProtoPlatformSessionValidationResponse,
};

/// `nano_transport_preference_version` must be at least this when `EnableNanoTransportPreference` is on.
pub const MIN_NANO_TRANSPORT_PREFERENCE_VERSION: i32 = 3;

/// The partner capabilities a validation accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedCapabilities {
    /// Always contains `SESSION_VALIDATION`; `NANO_TRANSPORT_PREFERENCE` while
    /// `EnableNanoTransportPreference` is on. `PERSISTENT_MESSAGE_CHANNEL` is recorded only under
    /// `EnableNanoOffloadPhase1`, which this build never sets.
    pub capabilities: Vec<ProtoPlatformCapabilities>,
    /// The accepted `nano_transport_preference_version` (read only while `EnableNanoTransportPreference`).
    pub nano_transport_preference_version: Option<i32>,
}

/// A partner's context source: what MSAEP needs to publish to and accept publications from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextSource {
    pub partner: DcgClient,
    /// `None` when the source was created without validation (gate off, or validation did not
    /// succeed — the partner not connected included).
    pub capabilities: Option<AcceptedCapabilities>,
}

/// The result of one `/SessionValidation`: the gate check before sending, and the response mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationOutcome {
    Validated(AcceptedCapabilities),
    /// "Validation not supported": gate off, `NoResponse` / `RemoteVersionTooLow` /
    /// `RemoteNotSubscribedToRoute`, an empty or unparsable payload, or a response without a required
    /// capability.
    NotSupported,
    /// The partner's connection is not connected; nothing was sent.
    NotConnected,
    /// `EnableNanoTransportPreference` is on and the phone's version is below 3 (hard failure).
    NanoTransportPreferenceVersionTooLow,
    /// The other request statuses, same-named (`RemoteAllSyncDisabled`, `RemoteMeteredSyncDisabled`,
    /// `Canceled`, `ResponseTimedOut`, …), and a failed send with its cause
    /// (`RequestSendFailed(DcgSendStatus)`).
    Failed(PlatformRequestStatus),
}

/// The local capability set: `SESSION_VALIDATION` always, `NANO_TRANSPORT_PREFERENCE` iff
/// `EnableNanoTransportPreference` (baseline `[SESSION_VALIDATION]`).
pub fn local_capabilities(config: &EngineConfig) -> Vec<ProtoPlatformCapabilities> {
    let mut capabilities = vec![ProtoPlatformCapabilities::SessionValidation];
    if config.platform.enable_nano_transport_preference {
        capabilities.push(ProtoPlatformCapabilities::NanoTransportPreference);
    }
    capabilities
}

/// The request message: Platform, `_route: /SessionValidation`, `ms-content-type:
/// application/x-binary`, body `ProtoPlatformSessionValidationRequest`.
pub fn request_message(config: &EngineConfig) -> TransportMessage {
    let body = ProtoPlatformSessionValidationRequest {
        platform_capabilities: local_capabilities(config).into_iter().map(|c| c as i32).collect(),
    }
    .encode_to_vec();

    let headers = HeaderMap::from_pairs([
        (header::ROUTE, route::SESSION_VALIDATION),
        (header::MS_CONTENT_TYPE, header::CONTENT_TYPE_BINARY),
    ])
    .expect("distinct header keys");
    TransportMessage::new(MessageType::Platform, headers, body)
}

/// The acceptance rules of the response table.
pub fn accept_response(config: &EngineConfig, payload: &[u8]) -> ValidationOutcome {
    if payload.is_empty() {
        return ValidationOutcome::NotSupported;
    }
    let Ok(response) = ProtoPlatformSessionValidationResponse::decode(payload) else {
        return ValidationOutcome::NotSupported;
    };

    let has = |c: ProtoPlatformCapabilities| response.platform_capabilities.contains(&(c as i32));
    if !has(ProtoPlatformCapabilities::SessionValidation) {
        return ValidationOutcome::NotSupported;
    }

    let mut accepted = AcceptedCapabilities {
        capabilities: vec![ProtoPlatformCapabilities::SessionValidation],
        nano_transport_preference_version: None,
    };

    if config.platform.enable_nano_transport_preference {
        if !has(ProtoPlatformCapabilities::NanoTransportPreference) {
            return ValidationOutcome::NotSupported;
        }
        if response.nano_transport_preference_version < MIN_NANO_TRANSPORT_PREFERENCE_VERSION {
            return ValidationOutcome::NanoTransportPreferenceVersionTooLow;
        }

        accepted.capabilities.push(ProtoPlatformCapabilities::NanoTransportPreference);
        accepted.nano_transport_preference_version = Some(response.nano_transport_preference_version);
    }
    ValidationOutcome::Validated(accepted)
}

/// The failure mapping of a `/SessionValidation` response.
pub fn map_response(config: &EngineConfig, response: PlatformResponse) -> ValidationOutcome {
    match response.status {
        PlatformRequestStatus::Success => match response.message {
            Some(message) => accept_response(config, &message.payload),
            None => ValidationOutcome::NotSupported,
        },
        PlatformRequestStatus::NoResponse
        | PlatformRequestStatus::RemoteVersionTooLow
        | PlatformRequestStatus::RemoteNotSubscribedToRoute => ValidationOutcome::NotSupported,
        other => ValidationOutcome::Failed(other),
    }
}

#[derive(Default)]
struct PartnerValidation {
    /// Bumped on every connect and disconnect: a validation started before it does not cache.
    generation: u64,
    /// The partner's context source, validated or not.
    cached: Option<ContextSource>,
    inflight: Option<Shared<BoxFuture<'static, ValidationOutcome>>>,
    /// The `/Context/Message/{partnerId}` route registered for `cached`.
    route: Option<String>,
}

/// The `/SessionValidation` requester and the per-partner context-source cache.
pub struct SessionValidator {
    me: Weak<SessionValidator>,
    config: Arc<EngineConfig>,
    router: Arc<Router>,
    sessions: Arc<PartnerSessions>,
    state: Mutex<HashMap<DcgClient, PartnerValidation>>,
    hub: Arc<ContextHub>,
}

impl SessionValidator {
    /// `own` is the PC's enrolled identity: the route its context messages go to.
    pub fn new(
        config: Arc<EngineConfig>,
        router: Arc<Router>,
        sessions: Arc<PartnerSessions>,
        own: DcgClient,
    ) -> Arc<Self> {
        let hub = ContextHub::new(own, Arc::downgrade(&router), config.platform.request_timeout);

        Arc::new_cyclic(|me| SessionValidator {
            me: me.clone(),
            config,
            router,
            sessions,
            state: Mutex::default(),
            hub,
        })
    }

    /// The partner's context handle: its context source (created as [`Self::context_source`] does)
    /// and the messages exchanged on it. `None` only when the source was dropped meanwhile.
    pub async fn context_handle(&self, partner: &DcgClient) -> Option<ContextHandle> {
        self.context_source(partner).await;
        self.hub.handle(partner)
    }

    /// The partner's context handle while its context source exists, without creating one.
    pub fn current_handle(&self, partner: &DcgClient) -> Option<ContextHandle> {
        self.hub.handle(partner)
    }

    /// Validates `partner`: gate off ⇒ `NotSupported` without sending; not connected ⇒ `NotConnected`;
    /// cached with capabilities ⇒ the cached result; a validation in flight ⇒ its result; else one new
    /// request.
    pub async fn validate(&self, partner: &DcgClient) -> ValidationOutcome {
        if !self.config.platform.session_validation_enabled {
            return ValidationOutcome::NotSupported;
        }
        if !self.sessions.is_connected(partner) {
            return ValidationOutcome::NotConnected;
        }

        let shared = {
            let mut state = self.state.lock_or_panic();
            let entry = state.entry(partner.clone()).or_default();
            if let Some(ContextSource { capabilities: Some(accepted), .. }) = &entry.cached {
                return ValidationOutcome::Validated(accepted.clone());
            }

            match &entry.inflight {
                Some(inflight) => inflight.clone(),
                None => {
                    let Some(me) = self.me.upgrade() else {
                        return ValidationOutcome::NotSupported;
                    };

                    let generation = entry.generation;
                    let target = partner.clone();
                    let task = tokio::spawn(async move { me.run(target, generation).await });
                    let joined: BoxFuture<'static, ValidationOutcome> = async move {
                        task.await.unwrap_or_else(|e| {
                            error!(error = %e, "/SessionValidation task failed");
                            ValidationOutcome::NotSupported
                        })
                    }
                    .boxed();

                    let shared = joined.shared();
                    entry.inflight = Some(shared.clone());
                    shared
                }
            }
        };

        shared.await
    }

    /// The partner's context source: with the gate on, the result of [`SessionValidator::validate`]
    /// when it succeeds (a cached validated source, or a cached source without capabilities validated
    /// again); otherwise — the gate off, or a validation that did not succeed for any reason, "not
    /// connected" included — the cached source, else one created without validation and cached, which
    /// registers the partner's `/Context/Message/{partnerId}`.
    pub async fn context_source(&self, partner: &DcgClient) -> ContextSource {
        if self.config.platform.session_validation_enabled {
            match self.validate(partner).await {
                ValidationOutcome::Validated(accepted) => {
                    return ContextSource { partner: partner.clone(), capabilities: Some(accepted) };
                }
                other => {
                    debug!(partner = %partner, outcome = ?other, "validation did not succeed; context source without validation")
                }
            }
        }

        let mut state = self.state.lock_or_panic();
        let entry = state.entry(partner.clone()).or_default();
        if let Some(cached) = &entry.cached {
            return cached.clone();
        }

        let source = ContextSource { partner: partner.clone(), capabilities: None };
        entry.cached = Some(source.clone());
        self.register_route_locked(partner, entry);
        source
    }

    /// Registers `/Context/Message/{partnerId}` for the partner's new context source (once).
    fn register_route_locked(&self, partner: &DcgClient, entry: &mut PartnerValidation) {
        if entry.route.is_some() {
            return;
        }

        let route = partner_route(partner);
        let handler = Arc::new(ContextMessageRoute::new(
            partner.clone(),
            Arc::downgrade(&self.router),
            Arc::downgrade(&self.hub),
        ));

        match self.router.register_route(&route, handler) {
            Ok(()) => {
                entry.route = Some(route);
                self.hub.created(partner);
            }
            // Another partner's source holds the route: two partners whose ids differ only by environment.
            Err(e) => error!(partner = %partner, error = %e, "context-source route not registered"),
        }
    }

    /// The cached (validated) context source of `partner`.
    pub fn cached(&self, partner: &DcgClient) -> Option<ContextSource> {
        self.state.lock_or_panic().get(partner).and_then(|e| e.cached.clone())
    }

    /// Trigger point "once per partner connect": forget what an earlier connection validated and
    /// validate anew (unless the gate is off).
    pub fn on_partner_connected(&self, partner: &DcgClient) {
        self.forget(partner);

        if !self.config.platform.session_validation_enabled {
            return;
        }
        let Some(me) = self.me.upgrade() else {
            return;
        };

        let partner = partner.clone();
        tokio::spawn(async move {
            me.validate(&partner).await;
        });
    }

    pub fn on_partner_disconnected(&self, partner: &DcgClient) {
        self.forget(partner);
    }

    /// Removes the partner's context source and its `/Context/Message/{partnerId}` route.
    fn forget(&self, partner: &DcgClient) {
        let mut state = self.state.lock_or_panic();
        let entry = state.entry(partner.clone()).or_default();
        entry.generation += 1;
        entry.cached = None;
        entry.inflight = None;

        if let Some(route) = entry.route.take() {
            self.router.unregister_route(&route);
        }
        self.hub.dropped(partner);
    }

    async fn run(self: Arc<Self>, partner: DcgClient, generation: u64) -> ValidationOutcome {
        let request = PlatformRequest {
            target: partner.clone(),
            message: request_message(&self.config),
            priority: priority::PROTOBUF_DEFAULT,
            options: SendOptions::NONE,
            timeout: self.config.platform.request_timeout,
            trace: TraceContext::new_root(Vec::new()),
            cancel: CancellationToken::new(),
        };

        let outcome = map_response(&self.config, self.router.request(request).await);

        {
            let mut state = self.state.lock_or_panic();
            let entry = state.entry(partner.clone()).or_default();
            if entry.generation == generation {
                entry.inflight = None;
                if let ValidationOutcome::Validated(accepted) = &outcome {
                    entry.cached =
                        Some(ContextSource { partner: partner.clone(), capabilities: Some(accepted.clone()) });
                    self.register_route_locked(&partner, entry);
                }
            }
        }

        match &outcome {
            ValidationOutcome::Validated(_) | ValidationOutcome::NotSupported => {
                info!(partner = %partner, ?outcome, "/SessionValidation finished")
            }
            _ => warn!(partner = %partner, ?outcome, "/SessionValidation failed; the session continues"),
        }
        outcome
    }

    /// Follows the sessions' events: validate on every connect, forget on every disconnect.
    pub(crate) fn spawn_session_listener(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let mut events = self.sessions.subscribe();

        tokio::spawn(async move {
            loop {
                let event = events.recv().await;
                let Some(validator) = weak.upgrade() else {
                    return;
                };

                match event {
                    Ok(SessionEvent::Connected(partner)) => validator.on_partner_connected(&partner),
                    Ok(SessionEvent::Disconnected { partner, .. }) => validator.on_partner_disconnected(&partner),
                    Err(RecvError::Lagged(missed)) => {
                        warn!(missed, "session events lagged; every cached context source is forgotten");
                        let partners: Vec<DcgClient> = validator.state.lock_or_panic().keys().cloned().collect();
                        for partner in partners {
                            validator.forget(&partner);
                        }
                    }
                    Err(RecvError::Closed) => return,
                }
            }
        });
    }
}

#[cfg(test)]
mod tests;
