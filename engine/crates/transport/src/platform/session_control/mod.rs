//! The session-control routes `/PlatformDisconnectRequest` and `/PlatformActivateSessionRequest`: the
//! PC's outbound `{}` messages and the inbound disconnect handler.
//!
//! The state transitions they cause live in [`super::session::PartnerSessions`].

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use prost::Message;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::kvp::Kvp;
use super::router::{InboundMessage, PlatformRouteHandler, Router};
use super::session::PartnerSessions;
use super::{priority, route};
use phonelink_core::seams::{PlatformMessenger, PlatformRequest, PlatformResponse};
use phonelink_core::types::{DcgClient, SendOptions, TraceContext};
use phonelink_proto::platform_sdk::v1::PlatformDisconnectRequest;

/// `PlatformDisconnectRequestReason` values the PC maps in an inbound `/PlatformDisconnectRequest`:
/// `0` unknown session end … `6` two-phone mode.
pub const MAX_KNOWN_DISCONNECT_REASON: i32 = 6;

/// Reasons whose session-end status is persistent across a `Default` connect: `0`–`4` (and an
/// unparsable reason, which maps to `0`).
pub fn is_persistent_disconnect_reason(reason: i32) -> bool {
    (0..=4).contains(&reason)
}

/// The mapped reason of an inbound `/PlatformDisconnectRequest` body.
///
/// The protobuf `PlatformDisconnectRequest.reason`; any parse failure — including the PC's own `{}`
/// body — and any value outside the enumeration is `0`, the unknown session end.
pub fn disconnect_reason(payload: &[u8]) -> i32 {
    match PlatformDisconnectRequest::decode(payload) {
        Ok(request) if (0..=MAX_KNOWN_DISCONNECT_REASON).contains(&request.reason) => request.reason,
        Ok(request) => {
            warn!(reason = request.reason, "unknown PlatformDisconnectRequestReason; read as an unknown session end");
            0
        }
        Err(e) => {
            warn!(error = %e, "PlatformDisconnectRequest body did not parse; read as an unknown session end");
            0
        }
    }
}

/// Sends the `{}` `/PlatformDisconnectRequest`: `text/json`, priority `0`, `OneWay |
/// SkipWakeIfNotConnected` (+ `SkipPartnerAck` when asked), bounded by `timeout` (30 s).
pub async fn send_disconnect_request(
    router: &Router,
    partner: &DcgClient,
    skip_partner_ack: bool,
    timeout: Duration,
) -> PlatformResponse {
    let options =
        SendOptions { one_way: true, skip_wake_if_not_connected: true, skip_partner_ack, ..SendOptions::NONE };
    router.request(empty_kvp_request(partner, route::PLATFORM_DISCONNECT_REQUEST, options, timeout)).await
}

/// Sends the `{}` `/PlatformActivateSessionRequest`: `text/json`, priority `0`, `OneWay` (+ the
/// connection-level `SkipWakeIfNotConnected`).
pub async fn send_activate_session_request(
    router: &Router,
    partner: &DcgClient,
    timeout: Duration,
) -> PlatformResponse {
    let options = SendOptions { one_way: true, skip_wake_if_not_connected: true, ..SendOptions::NONE };
    router.request(empty_kvp_request(partner, route::PLATFORM_ACTIVATE_SESSION_REQUEST, options, timeout)).await
}

fn empty_kvp_request(partner: &DcgClient, route: &str, options: SendOptions, timeout: Duration) -> PlatformRequest {
    PlatformRequest {
        target: partner.clone(),
        message: Kvp::new().platform_message(Some(route)),
        priority: priority::KVP,
        options,
        timeout,
        trace: TraceContext::new_root(Vec::new()),
        cancel: CancellationToken::new(),
    }
}

/// Inbound `/PlatformDisconnectRequest`: parse the reason, send **no** reply, and hand the transition to
/// the partner's connection.
pub struct PlatformDisconnectRoute {
    sessions: Arc<PartnerSessions>,
}

impl PlatformDisconnectRoute {
    pub fn new(sessions: Arc<PartnerSessions>) -> Self {
        Self { sessions }
    }
}

#[async_trait]
impl PlatformRouteHandler for PlatformDisconnectRoute {
    async fn handle(&self, inbound: InboundMessage) {
        let reason = disconnect_reason(&inbound.message.payload);
        self.sessions.on_remote_disconnect_request(&inbound.source, reason);
    }
}

#[cfg(test)]
mod tests;
