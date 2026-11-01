//! `/Context/Message/{partnerId}`.
//!
//! It covers the route a partner's context source subscribes while it exists
//! ([`super::session_validation::SessionValidator`] registers and removes it), and the context
//! handle the Nano device connection exchanges its connection information over.
//!
//! Handling an inbound message is parsing it, answering `processStatus`, and handing a parsed message
//! to the partner's [`ContextHandle`] subscribers. An inbound `cs_initialize_message` is answered with
//! `cs_initialize_ack_message { status: ERROR }`: the PC never accepts a phone-initiated connection.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use async_trait::async_trait;
use prost::Message;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use super::kvp::Kvp;
use super::router::{InboundMessage, PlatformRouteHandler, Router};
use super::{priority, route};
use phonelink_core::seams::{PlatformMessenger, PlatformRequest, PlatformRequestStatus};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::{
    DcgClient, DcgSendStatus, HeaderMap, MessageType, SendOptions, TraceContext, TransportMessage, header,
};
use phonelink_proto::platform_sdk::v1::context_source_message::Type;
use phonelink_proto::platform_sdk::v1::{
    ContextSourceMessage, CsInitializeAckMessage, CsInitializeStatus, ProtoTraceContext,
};

/// The KVP key of the reply.
pub const PROCESS_STATUS_KEY: &str = "processStatus";

/// `ContextMessageReplyStatus` as the PC answers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyStatus {
    Success,
    Error,
}

impl ReplyStatus {
    pub fn wire(self) -> &'static str {
        match self {
            ReplyStatus::Success => "Success",
            ReplyStatus::Error => "Error",
        }
    }
}

/// The route of `partner`'s context source: `/Context/Message/{partner dcgClientId}`.
pub fn partner_route(partner: &DcgClient) -> String {
    format!("{}/{}", route::CONTEXT_MESSAGE, partner.id)
}

/// A W3C trace id: 32 UTF-16 units, each a letter or digit, not all `0`.
fn is_w3c_trace_id(id: &str) -> bool {
    id.chars().count() == 32
        && id.chars().all(|c| c.len_utf16() == 1 && c.is_alphanumeric())
        && id.chars().any(|c| c != '0')
}

fn trace_valid(trace: Option<&ProtoTraceContext>) -> bool {
    trace.is_some_and(|t| is_w3c_trace_id(&t.trace_id))
}

/// Whether handling `payload` succeeds.
///
/// It must parse as a `ContextSourceMessage`; a known case must carry a `trace_context` whose
/// `trace_id` is a W3C id; a `cs_connection_info` must carry a non-empty `connection_info` that is
/// not the JSON `null`.
///
/// A message with no case or an unknown
/// case (`cs_identity_challenge`, `cs_identity_response`) succeeds.
#[cfg(any(test, feature = "test-support"))]
pub fn handling_status(payload: &[u8]) -> ReplyStatus {
    match assess(payload) {
        Ok(()) | Err(Anomaly::UnhandledCase(_)) => ReplyStatus::Success,
        Err(_) => ReplyStatus::Error,
    }
}

/// What is wrong with a context-source message; every one but [`Anomaly::UnhandledCase`] answers
/// it `ERROR`.
#[derive(Debug, thiserror::Error)]
enum Anomaly {
    #[error("not a ContextSourceMessage: {0}")]
    Undecodable(#[source] prost::DecodeError),
    #[error("no trace_context with a W3C trace_id")]
    InvalidTrace,
    #[error("empty or null connection_info")]
    NoConnectionInfo,
    /// A case the PC does not handle (its field name, or `none`): answered as handled.
    #[error("case {0} is not handled")]
    UnhandledCase(&'static str),
}

/// [`handling_status`], with what is wrong with the message.
fn assess(payload: &[u8]) -> Result<(), Anomaly> {
    let message = ContextSourceMessage::decode(payload).map_err(Anomaly::Undecodable)?;
    let trace = |t: Option<&ProtoTraceContext>| if trace_valid(t) { Ok(()) } else { Err(Anomaly::InvalidTrace) };

    match &message.r#type {
        Some(Type::CsInitializeMessage(m)) => trace(m.trace_context.as_ref()),
        Some(Type::CsInitializeAckMessage(m)) => trace(m.trace_context.as_ref()),
        Some(Type::CsOpenDeviceConnectionRequest(m)) => trace(m.trace_context.as_ref()),
        Some(Type::CsConnectionInfo(m)) => {
            trace(m.trace_context.as_ref())?;
            let null =
                matches!(serde_json::from_str::<serde_json::Value>(&m.connection_info), Ok(serde_json::Value::Null));
            if m.connection_info.is_empty() || null { Err(Anomaly::NoConnectionInfo) } else { Ok(()) }
        }
        Some(Type::CsErrorInfo(m)) => trace(m.trace_context.as_ref()),
        Some(Type::CsWifiP2pInfo(m)) => trace(m.trace_context.as_ref()),
        Some(Type::CsIdentityChallenge(_)) => Err(Anomaly::UnhandledCase("cs_identity_challenge")),
        Some(Type::CsIdentityResponse(_)) => Err(Anomaly::UnhandledCase("cs_identity_response")),
        None => Err(Anomaly::UnhandledCase("none")),
    }
}

/// The `ProtoTraceContext` of `trace`.
pub fn proto_trace(trace: &TraceContext) -> ProtoTraceContext {
    ProtoTraceContext {
        trace_id: trace.trace_id.clone(),
        parent_id: trace.parent_id.clone().unwrap_or_default(),
        correlation_id: trace.correlation_id.clone(),
        trace_state: trace.trace_state.iter().cloned().collect(),
    }
}

/// Where a context message the PC sends goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextRoute {
    /// The bare `/Context/Message` (`cs_initialize_message`).
    Initialize,
    /// `/Context/Message/{PC dcgClientId}` (every other case).
    Own,
}

/// Why sending a context message failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContextSendError {
    #[error("the context handle was dropped")]
    Dropped,
    #[error("the context message was not delivered: {0:?}")]
    Request(PlatformRequestStatus),
    #[error("the phone answered processStatus {0:?}")]
    Status(Option<String>),
}

struct PartnerContext {
    dropped: CancellationToken,
    inbound: broadcast::Sender<ContextSourceMessage>,
}

/// The live context handles, one per partner while its context source exists.
pub struct ContextHub {
    own: DcgClient,
    router: Weak<Router>,
    request_timeout: std::time::Duration,
    partners: Mutex<HashMap<DcgClient, PartnerContext>>,
}

impl ContextHub {
    pub fn new(own: DcgClient, router: Weak<Router>, request_timeout: std::time::Duration) -> Arc<Self> {
        Arc::new(Self { own, router, request_timeout, partners: Mutex::default() })
    }

    /// The partner's context source now exists.
    pub fn created(&self, partner: &DcgClient) {
        let mut partners = self.partners.lock_or_panic();
        partners
            .entry(partner.clone())
            .or_insert_with(|| PartnerContext { dropped: CancellationToken::new(), inbound: broadcast::channel(64).0 });
    }

    /// The partner's context source is gone: every handle on it is dropped.
    pub fn dropped(&self, partner: &DcgClient) {
        if let Some(p) = self.partners.lock_or_panic().remove(partner) {
            debug!(partner = %partner, "context handle dropped");
            p.dropped.cancel();
        }
    }

    fn deliver(&self, partner: &DcgClient, message: ContextSourceMessage) {
        if let Some(p) = self.partners.lock_or_panic().get(partner) {
            let _ = p.inbound.send(message);
        }
    }

    /// The partner's handle, while its context source exists.
    pub fn handle(&self, partner: &DcgClient) -> Option<ContextHandle> {
        let partners = self.partners.lock_or_panic();
        let p = partners.get(partner)?;
        Some(ContextHandle {
            partner: partner.clone(),
            own: self.own.clone(),
            router: self.router.clone(),
            request_timeout: self.request_timeout,
            dropped: p.dropped.clone(),
            inbound: p.inbound.clone(),
        })
    }
}

/// A partner's context handle: the context messages it receives, the ones the PC sends, and whether
/// it was dropped.
#[derive(Clone)]
pub struct ContextHandle {
    pub partner: DcgClient,
    own: DcgClient,
    router: Weak<Router>,
    request_timeout: std::time::Duration,
    dropped: CancellationToken,
    inbound: broadcast::Sender<ContextSourceMessage>,
}

impl std::fmt::Debug for ContextHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContextHandle").field("partner", &self.partner).field("dropped", &self.is_dropped()).finish()
    }
}

impl ContextHandle {
    /// The partner's `dcgClientId`: the context source id.
    pub fn context_source_id(&self) -> &str {
        &self.partner.id
    }

    /// The PC's own `dcgClientId`.
    pub fn own_id(&self) -> &str {
        &self.own.id
    }

    pub fn is_dropped(&self) -> bool {
        self.dropped.is_cancelled()
    }

    /// Both are handles on the same context source of the same partner (a source created again after
    /// a drop is another one).
    pub fn same_source(&self, other: &ContextHandle) -> bool {
        self.partner == other.partner && self.inbound.same_channel(&other.inbound)
    }

    /// Cancelled when the handle is dropped.
    pub fn dropped(&self) -> CancellationToken {
        self.dropped.clone()
    }

    /// Context messages from the partner, from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<ContextSourceMessage> {
        self.inbound.subscribe()
    }

    /// Sends `message` as a platform message (protobuf body, priority 0, sent even when the device is
    /// disabled) and checks the phone's `processStatus` when the reply carries one.
    pub async fn send(
        &self,
        message: &ContextSourceMessage,
        to: ContextRoute,
        trace: TraceContext,
    ) -> Result<(), ContextSendError> {
        if self.is_dropped() {
            return Err(ContextSendError::Dropped);
        }

        let route = match to {
            ContextRoute::Initialize => route::CONTEXT_MESSAGE.to_string(),
            ContextRoute::Own => format!("{}/{}", route::CONTEXT_MESSAGE, self.own.id),
        };

        let headers = HeaderMap::from_pairs([
            (header::ROUTE, route.as_str()),
            (header::MS_CONTENT_TYPE, header::CONTENT_TYPE_BINARY),
        ])
        .expect("distinct header keys");
        let request = PlatformRequest {
            target: self.partner.clone(),
            message: TransportMessage::new(MessageType::Platform, headers, message.encode_to_vec()),
            priority: priority::CONTEXT_SOURCE,
            options: SendOptions { ignore_device_disabled: true, ..SendOptions::NONE },
            timeout: self.request_timeout,
            trace,
            cancel: self.dropped.child_token(),
        };

        let router = self.router.upgrade().ok_or(ContextSendError::Dropped)?;
        let response = router.request(request).await;
        if response.status != PlatformRequestStatus::Success {
            return Err(if self.is_dropped() {
                ContextSendError::Dropped
            } else {
                ContextSendError::Request(response.status)
            });
        }

        let status = response
            .message
            .and_then(|m| Kvp::from_message(&m).ok())
            .and_then(|k| k.get(PROCESS_STATUS_KEY).map(str::to_string));
        // A reply without `processStatus` (or not a KVP) counts as delivered; `Success` in any letter
        // case is success, anything else a failure.
        match status.as_deref() {
            None => Ok(()),
            Some(s) if s.eq_ignore_ascii_case("Success") => Ok(()),
            Some(_) => Err(ContextSendError::Status(status)),
        }
    }
}

/// The handler of one partner's `/Context/Message/{partnerId}`.
pub struct ContextMessageRoute {
    partner: DcgClient,
    router: Weak<Router>,
    hub: Weak<ContextHub>,
}

impl ContextMessageRoute {
    pub fn new(partner: DcgClient, router: Weak<Router>, hub: Weak<ContextHub>) -> Self {
        Self { partner, router, hub }
    }

    /// Answers a phone-initiated initialize with `ERROR`.
    async fn refuse_initialize(&self, hub: &ContextHub, trace: Option<ProtoTraceContext>) {
        let Some(handle) = hub.handle(&self.partner) else {
            return;
        };

        let ack = ContextSourceMessage {
            r#type: Some(Type::CsInitializeAckMessage(CsInitializeAckMessage {
                status: CsInitializeStatus::Error as i32,
                trace_context: trace,
            })),
        };

        if let Err(e) = handle.send(&ack, ContextRoute::Own, TraceContext::new_root(Vec::new())).await {
            warn!(partner = %self.partner, error = %e, "initialize refusal not delivered");
        }
    }
}

#[async_trait]
impl PlatformRouteHandler for ContextMessageRoute {
    async fn handle(&self, inbound: InboundMessage) {
        if inbound.source != self.partner {
            warn!(partner = %self.partner, source = %inbound.source, "context-source message on another partner's route; dropped");
            return;
        }

        let assessed = assess(&inbound.message.payload);
        let status = match &assessed {
            Ok(()) | Err(Anomaly::UnhandledCase(_)) => ReplyStatus::Success,
            Err(_) => ReplyStatus::Error,
        };

        let has_request_id = inbound.message.headers.contains(header::REQUEST_ID);
        match &assessed {
            Err(anomaly) if has_request_id => {
                warn!(partner = %self.partner, status = status.wire(), reason = %anomaly, "context-source message refused or not handled; answered with its status");
            }
            Err(anomaly) => {
                warn!(partner = %self.partner, status = status.wire(), reason = %anomaly, "context-source message refused or not handled; no _requestId, no reply sent");
            }
            Ok(()) if !has_request_id => {
                warn!(partner = %self.partner, status = status.wire(), "context-source message without _requestId; no reply sent");
            }
            Ok(()) => debug!(partner = %self.partner, status = status.wire(), "context-source message handled"),
        }

        if !has_request_id {
            return;
        }
        let Some(router) = self.router.upgrade() else {
            return;
        };

        let reply = Kvp::new().with(PROCESS_STATUS_KEY, status.wire()).platform_message(None);
        let sent = router.respond(&inbound.source, &inbound.message, reply, priority::KVP, inbound.trace.clone()).await;
        if sent != DcgSendStatus::Success {
            warn!(partner = %self.partner, send_status = ?sent, error = sent.cause(), "context-source reply not delivered");
        }

        if status != ReplyStatus::Success {
            return;
        }
        let (Some(hub), Ok(message)) =
            (self.hub.upgrade(), ContextSourceMessage::decode(inbound.message.payload.as_slice()))
        else {
            return;
        };

        match &message.r#type {
            Some(Type::CsInitializeMessage(m)) => self.refuse_initialize(&hub, m.trace_context.clone()).await,
            Some(Type::CsOpenDeviceConnectionRequest(_)) => {
                debug!(partner = %self.partner, "open-device-connection request ignored")
            }
            _ => hub.deliver(&self.partner, message),
        }
    }
}

#[cfg(test)]
mod tests;
