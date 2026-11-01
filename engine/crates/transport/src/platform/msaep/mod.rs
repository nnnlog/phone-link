//! MSAEP publish/subscribe over `/Context/Publish`: the MSAEP envelope and its sequence.
//!
//! The body is a serialized `MsaepMessage` wrapping a `PubSubPayload`, in the clear inside the hub
//! connection. Inbound: parse, acknowledge with `{}`, obtain the partner's context source (validated,
//! or created without validation), deliver to the subscribers whose device filter, tag and
//! last-delivered id let it through. Outbound: one `OneWay` request per connected partner, each with
//! its context source obtained first. The engine is the relay (`Cloud`) provider with trust filter
//! `MyDevices`; publication TTL, frequency and connectability have no wire effect and are not modelled.

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use async_trait::async_trait;
use prost::Message;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::kvp::Kvp;
use super::router::{InboundMessage, PlatformRouteHandler, Router, panic_text};
use super::session::PartnerSessions;
use super::session_validation::{ContextSource, SessionValidator};
use super::{priority, route};
use phonelink_core::config::EngineConfig;
use phonelink_core::seams::{PlatformMessenger, PlatformRequest, PlatformRequestStatus};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::{
    DcgClient, DcgSendStatus, HeaderMap, MessageType, SendOptions, TraceContext, TransportMessage, header,
    new_guid_lower,
};
use phonelink_proto::platform_sdk::v1::{MsaepMessage, PubSubPayload};

/// `MsaepMessage.platform_major_version` / `platform_minor_version`.
pub const PLATFORM_MAJOR_VERSION: i32 = 1;
pub const PLATFORM_MINOR_VERSION: i32 = 1;

/// Which publishers a subscriber accepts, by `MsaepMessage.dcg_client_id` (exact match).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceFilter {
    /// The empty list: any publisher.
    Any,
    /// Only these `dcgClientId`s; an empty list is the empty filter, i.e. any publisher.
    Only(Vec<String>),
    /// The `null` filter: reject every publication.
    RejectAll,
}

impl DeviceFilter {
    pub fn accepts(&self, dcg_client_id: &str) -> bool {
        match self {
            DeviceFilter::Any => true,
            DeviceFilter::Only(ids) => ids.is_empty() || ids.iter().any(|id| id == dcg_client_id),
            DeviceFilter::RejectAll => false,
        }
    }
}

/// One delivered publication.
#[derive(Debug, Clone, PartialEq)]
pub struct Publication {
    /// The partner the message arrived from (identifies the phone; `message.dcg_client_id` is the
    /// publisher's self-declared id).
    pub source: DcgClient,
    pub context: ContextSource,
    pub message: MsaepMessage,
    /// `message.payload` parsed. The phone puts its scenario message in `additional`; which field a
    /// tag uses is the feature page's rule.
    pub payload: PubSubPayload,
}

/// A subscriber's callback. Called on the router's task for the publication: return quickly (spawn
/// any longer work). A panic is caught and logged.
pub trait PublicationObserver: Send + Sync {
    fn on_publication(&self, publication: &Publication);
}

impl<F: Fn(&Publication) + Send + Sync> PublicationObserver for F {
    fn on_publication(&self, publication: &Publication) {
        self(publication)
    }
}

struct Subscriber {
    id: u64,
    tag: i16,
    filter: DeviceFilter,
    observer: Arc<dyn PublicationObserver>,
    /// One id per observer, not a set.
    last_message_id: Mutex<Option<String>>,
}

/// A live subscription; dropping it unsubscribes.
pub struct Subscription {
    owner: Weak<Msaep>,
    id: u64,
}

impl Subscription {
    /// Unsubscribes now (same as dropping it).
    pub fn unsubscribe(self) {}
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            owner.subscribers.lock_or_panic().retain(|s| s.id != self.id);
        }
    }
}

/// The outcome over all the sends of one publication (only logged).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishOutcome {
    Success,
    PartialSuccess,
    Failure,
    /// No connected partner with a context source.
    NoResults,
}

/// A publication whose sends have been started.
pub struct PublishHandle {
    pub message_id: String,
    pub targets: Vec<DcgClient>,
    outcome: tokio::task::JoinHandle<PublishOutcome>,
}

impl PublishHandle {
    /// Waits for every send of the publication.
    pub async fn outcome(self) -> PublishOutcome {
        self.outcome.await.unwrap_or(PublishOutcome::Failure)
    }
}

/// MSAEP over the relay: the subscriber registry and the publisher.
pub struct Msaep {
    me: Weak<Msaep>,
    config: Arc<EngineConfig>,
    router: Arc<Router>,
    sessions: Arc<PartnerSessions>,
    validator: Arc<SessionValidator>,
    own: DcgClient,
    subscribers: Mutex<Vec<Arc<Subscriber>>>,
    next_subscription: AtomicU64,
}

impl Msaep {
    /// `own` is the PC's enrolled `DcgClient`; its id is every publication's `dcg_client_id`.
    pub fn new(
        config: Arc<EngineConfig>,
        router: Arc<Router>,
        sessions: Arc<PartnerSessions>,
        validator: Arc<SessionValidator>,
        own: DcgClient,
    ) -> Arc<Self> {
        Arc::new_cyclic(|me| Msaep {
            me: me.clone(),
            config,
            router,
            sessions,
            validator,
            own,
            subscribers: Mutex::default(),
            next_subscription: AtomicU64::new(1),
        })
    }

    /// Subscribes `observer` to tag `tag` (a 16-bit value widened with sign extension; a publication
    /// matches only when its `int32` `message_tag` equals it).
    pub fn subscribe(&self, tag: i16, filter: DeviceFilter, observer: Arc<dyn PublicationObserver>) -> Subscription {
        let id = self.next_subscription.fetch_add(1, Ordering::Relaxed);
        let subscriber = Arc::new(Subscriber { id, tag, filter, observer, last_message_id: Mutex::new(None) });
        self.subscribers.lock_or_panic().push(subscriber);
        Subscription { owner: self.me.clone(), id }
    }

    /// Publishes `payload` under `tag` (the outbound `/Context/Publish` sequence): `MsaepMessage {
    /// message_tag, dcg_client_id = own id, message_id = fresh GUID, payload, 1, 1 }`, sent `OneWay`
    /// (`application/x-binary`, default priority) to every connected partner, after obtaining its
    /// context source (which validates it when the gate is on). Returns once the sends have started.
    pub async fn publish(&self, tag: i16, payload: PubSubPayload) -> PublishHandle {
        let message = MsaepMessage {
            message_tag: i32::from(tag),
            dcg_client_id: self.own.id.clone(),
            message_id: new_guid_lower(),
            payload: payload.encode_to_vec(),
            platform_major_version: PLATFORM_MAJOR_VERSION,
            platform_minor_version: PLATFORM_MINOR_VERSION,
        };
        let body = message.encode_to_vec();

        let mut targets = Vec::new();
        for partner in self.sessions.connected_partners() {
            let context = self.validator.context_source(&partner).await;
            debug!(partner = %partner, validated = context.capabilities.is_some(), "publication target");
            targets.push(partner);
        }

        let trace = TraceContext::new_root(Vec::new());
        let mut sends = Vec::with_capacity(targets.len());
        for target in &targets {
            let request = PlatformRequest {
                target: target.clone(),
                message: TransportMessage::new(MessageType::Platform, publish_headers(), body.clone()),
                priority: priority::PROTOBUF_DEFAULT,
                options: SendOptions { one_way: true, ..SendOptions::NONE },
                timeout: self.config.platform.request_timeout,
                trace: trace.child(),
                cancel: CancellationToken::new(),
            };

            let router = self.router.clone();
            sends.push(tokio::spawn(async move { router.request(request).await.status }));
        }

        let message_id = message.message_id.clone();
        let logged_id = message_id.clone();
        let outcome = tokio::spawn(async move {
            let total = sends.len();
            let mut succeeded = 0;
            for send in sends {
                if matches!(send.await, Ok(PlatformRequestStatus::Success)) {
                    succeeded += 1;
                }
            }

            let outcome = match (total, succeeded) {
                (0, _) => PublishOutcome::NoResults,
                (t, s) if s == t => PublishOutcome::Success,
                (_, 0) => PublishOutcome::Failure,
                _ => PublishOutcome::PartialSuccess,
            };

            info!(tag_kind = tag, message_id = %logged_id, ?outcome, target_count = total, succeeded_count = succeeded, "publication finished");
            outcome
        });

        PublishHandle { message_id, targets, outcome }
    }

    /// The inbound `/Context/Publish` sequence: parse, acknowledge, obtain the context source, deliver.
    async fn on_inbound(&self, inbound: InboundMessage) {
        let InboundMessage { source, trace, message, .. } = inbound;
        let msaep = match MsaepMessage::decode(message.payload.as_slice()) {
            Ok(m) => m,
            Err(e) => {
                warn!(partner = %source, error = %e, "/Context/Publish body is not an MsaepMessage; dropped, no response");
                return;
            }
        };

        if message.headers.contains(header::REQUEST_ID) {
            let router = self.router.clone();
            let (to, request) = (source.clone(), message.clone());
            tokio::spawn(async move {
                let ack = Kvp::new().platform_message(None);
                let status = router.respond(&to, &request, ack, priority::KVP, trace).await;
                if status != DcgSendStatus::Success {
                    warn!(partner = %to, ?status, error = status.cause(), "/Context/Publish acknowledgement not delivered");
                }
            });
        } else {
            warn!(partner = %source, "/Context/Publish without _requestId; no acknowledgement sent");
        }

        let context = self.validator.context_source(&source).await;
        let payload = match PubSubPayload::decode(msaep.payload.as_slice()) {
            Ok(p) => p,
            Err(e) => {
                warn!(partner = %source, tag_kind = msaep.message_tag, error = %e, "MsaepMessage.payload is not a PubSubPayload; dropped");
                return;
            }
        };

        self.deliver(&Publication { source, context, message: msaep, payload });
    }

    fn deliver(&self, publication: &Publication) {
        let subscribers = self.subscribers.lock_or_panic().clone();
        let message = &publication.message;
        let mut delivered = 0usize;

        for subscriber in subscribers {
            if !subscriber.filter.accepts(&message.dcg_client_id) || message.message_tag != i32::from(subscriber.tag) {
                continue;
            }
            if !claim_message_id(&subscriber, message) {
                continue;
            }

            delivered += 1;
            notify_observer(&subscriber, publication);
        }

        if delivered == 0 {
            debug!(partner = %publication.source, tag_kind = message.message_tag, "publication matched no subscriber");
        }
    }
}

/// Records `message` as the subscriber's last one; `false` (logged) when it is a duplicate of it.
fn claim_message_id(subscriber: &Subscriber, message: &MsaepMessage) -> bool {
    let mut last = subscriber.last_message_id.lock_or_panic();
    if last.as_deref() == Some(message.message_id.as_str()) {
        debug!(tag_kind = message.message_tag, message_id = %message.message_id, "duplicate publication for this observer; dropped");
        return false;
    }

    *last = Some(message.message_id.clone());
    true
}

/// Hands the publication to the subscriber's observer, isolating a panic in it.
fn notify_observer(subscriber: &Subscriber, publication: &Publication) {
    let observer = subscriber.observer.clone();
    if let Err(panic) = std::panic::catch_unwind(AssertUnwindSafe(|| observer.on_publication(publication))) {
        error!(tag_kind = publication.message.message_tag, panic = %panic_text(panic.as_ref()), "observer panicked; isolated");
    }
}

fn publish_headers() -> HeaderMap {
    HeaderMap::from_pairs([
        (header::ROUTE, route::CONTEXT_PUBLISH),
        (header::MS_CONTENT_TYPE, header::CONTENT_TYPE_BINARY),
    ])
    .expect("distinct header keys")
}

/// The `/Context/Publish` route handler.
pub struct ContextPublishRoute(pub Arc<Msaep>);

#[async_trait]
impl PlatformRouteHandler for ContextPublishRoute {
    async fn handle(&self, inbound: InboundMessage) {
        self.0.on_inbound(inbound).await;
    }
}

#[cfg(test)]
mod tests;
