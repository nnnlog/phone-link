//! Inbound crypto-trust key rotation — `/CryptoTrustKeyRotationCheck` and `/CryptoTrustKeyRotation`
//! and the key rotation sequence (inbound handling, **M1**).
//!
//! Both are `text/json` KVP exchanges answered on `/internal/response` over the [`TrustGate`]. The
//! requester side is a later milestone.

use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use tracing::{debug, error, info, warn};

use super::kvp::Kvp;
use super::priority;
use super::router::{InboundMessage, PlatformRouteHandler, Router};
use crate::auth::crypto;
use phonelink_core::edge::Clock;
use phonelink_core::seams::{PlatformMessenger, TrustGate, TrustRotationError};
use phonelink_core::types::{DcgClient, DcgSendStatus, TransportMessage, header};

pub const RESULT_KEY: &str = "CryptoKeyRotationResult";
pub const TIME_TO_EXPIRE_KEY: &str = "TimeToExpireInSeconds";
pub const RESULT_DETAILS_KEY: &str = "ResultDetails";
pub const CERTIFICATE_KEY: &str = "Certificate";

pub const RELATIONSHIP_NOT_FOUND_DETAILS: &str = "crypto trust relationship not found";
pub const INVALID_CERTIFICATE_DETAILS: &str = "Can't get valid partner certificate.";
/// The name of the trust-manager error type of a new partner certificate that has already expired.
pub const EXPIRED_CERTIFICATE_DETAILS: &str = "PartnerCertExpirationTimeStampInvalid";

/// `CryptoKeyRotationResult`, sent as its decimal string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoKeyRotationResult {
    Success = 0,
    CryptoTrustRelationshipNotFound = 1,
    InvalidPayload = 2,
    InternalError = 3,
    CryptoTrustDisabled = 4,
}

impl CryptoKeyRotationResult {
    pub fn wire(self) -> String {
        (self as i32).to_string()
    }
}

/// `ceil((expiration − now).TotalSeconds)`, negative when already expired.
pub fn ceil_seconds_until(expiration: SystemTime, now: SystemTime) -> i64 {
    match expiration.duration_since(now) {
        Ok(left) => {
            let whole = i64::try_from(left.as_secs()).unwrap_or(i64::MAX);
            whole.saturating_add(i64::from(left.subsec_nanos() > 0))
        }
        // ceil(-x) = -floor(x)
        Err(past) => -i64::try_from(past.duration().as_secs()).unwrap_or(i64::MAX),
    }
}

/// A success reply: the extra keys first, then `CryptoKeyRotationResult: "0"`.
fn success(extra: Kvp) -> Kvp {
    extra.with(RESULT_KEY, CryptoKeyRotationResult::Success.wire())
}

/// A failure reply always carries `ResultDetails`.
fn failure(result: CryptoKeyRotationResult, details: &str) -> Kvp {
    Kvp::new().with(RESULT_KEY, result.wire()).with(RESULT_DETAILS_KEY, details)
}

fn relationship_not_found() -> Kvp {
    failure(CryptoKeyRotationResult::CryptoTrustRelationshipNotFound, RELATIONSHIP_NOT_FOUND_DETAILS)
}

fn invalid_certificate() -> Kvp {
    failure(CryptoKeyRotationResult::InvalidPayload, INVALID_CERTIFICATE_DETAILS)
}

/// The `/CryptoTrustKeyRotationCheck` reply for `partner` (the request body is not read).
pub fn check_reply(trust: &dyn TrustGate, now: SystemTime, partner: &DcgClient) -> Kvp {
    match trust.partner_key_expiration(partner) {
        None => relationship_not_found(),
        Some(expiration) => {
            success(Kvp::new().with(TIME_TO_EXPIRE_KEY, ceil_seconds_until(expiration, now).to_string()))
        }
    }
}

/// The request's `Certificate`, decoded as standard base64 (white space ignored, padding required)
/// and loaded as an X.509 certificate; `None` when missing or not a certificate.
fn requested_certificate(kvp: &Kvp) -> Option<crypto::ParsedCertificate> {
    let der = crypto::b64_std_decode(kvp.get(CERTIFICATE_KEY)?).ok()?;
    crypto::parse_certificate(&der).ok()
}

/// The `/CryptoTrustKeyRotation` reply for `partner`, storing the new partner certificate; `None`
/// when no reply is sent, as Phone Link on Windows sends none.
///
/// A body the KVP reader rejects (not JSON, not an object, a repeated key) and a trust store that
/// fails to persist.
pub fn rotation_reply(
    trust: &dyn TrustGate,
    now: SystemTime,
    partner: &DcgClient,
    request: &TransportMessage,
) -> Option<Kvp> {
    let kvp = match Kvp::from_message(request) {
        Ok(kvp) => kvp,
        Err(e) => {
            warn!(partner = %partner, error = %e, "/CryptoTrustKeyRotation body is not a KVP map; dropped, no reply");
            return None;
        }
    };

    if trust.partner_key_expiration(partner).is_none() {
        return Some(relationship_not_found());
    }
    let Some(certificate) = requested_certificate(&kvp) else {
        return Some(invalid_certificate());
    };
    if certificate.not_after <= now {
        return Some(failure(CryptoKeyRotationResult::InternalError, EXPIRED_CERTIFICATE_DETAILS));
    }

    match trust.rotate_partner_certificate(partner, &certificate.der) {
        Ok(()) => Some(success(Kvp::new())),
        Err(TrustRotationError::NotFound) => Some(relationship_not_found()),
        Err(TrustRotationError::InvalidCertificate) => Some(invalid_certificate()),
        Err(TrustRotationError::Store(e)) => {
            error!(partner = %partner, error = %e, "new partner certificate not persisted; no reply");
            None
        }
    }
}

/// How a key-rotation reply went.
enum Delivery {
    Sent,
    /// The request has no `_requestId`, so no reply can be sent.
    NoRequestId,
    Failed(DcgSendStatus),
}

async fn reply(router: &Router, inbound: &InboundMessage, kvp: Kvp) -> Delivery {
    if !inbound.message.headers.contains(header::REQUEST_ID) {
        return Delivery::NoRequestId;
    }

    let status = router
        .respond(&inbound.source, &inbound.message, kvp.platform_message(None), priority::KVP, inbound.trace.clone())
        .await;
    if status == DcgSendStatus::Success { Delivery::Sent } else { Delivery::Failed(status) }
}

/// The `Result` value of a reply, for its line.
fn result_of(kvp: &Kvp) -> Option<String> {
    kvp.get(RESULT_KEY).map(|v| v.to_string())
}

/// Inbound `/CryptoTrustKeyRotationCheck`.
pub struct KeyRotationCheckRoute {
    router: Arc<Router>,
    trust: Arc<dyn TrustGate>,
    clock: Arc<dyn Clock>,
}

impl KeyRotationCheckRoute {
    pub fn new(router: Arc<Router>, trust: Arc<dyn TrustGate>, clock: Arc<dyn Clock>) -> Self {
        Self { router, trust, clock }
    }
}

#[async_trait]
impl PlatformRouteHandler for KeyRotationCheckRoute {
    async fn handle(&self, inbound: InboundMessage) {
        let kvp = check_reply(self.trust.as_ref(), self.clock.now(), &inbound.source);
        let result = result_of(&kvp);

        match reply(&self.router, &inbound, kvp).await {
            Delivery::Sent => debug!(partner = %inbound.source, ?result, "key-rotation check answered"),
            Delivery::NoRequestId => {
                warn!(partner = %inbound.source, ?result, "key-rotation check without _requestId; no reply sent");
            }
            Delivery::Failed(status) => {
                warn!(partner = %inbound.source, ?result, ?status, error = status.cause(), "key-rotation check reply not delivered");
            }
        }
    }
}

/// Inbound `/CryptoTrustKeyRotation`. The trust store write runs on a blocking thread (it persists).
pub struct KeyRotationRoute {
    router: Arc<Router>,
    trust: Arc<dyn TrustGate>,
    clock: Arc<dyn Clock>,
}

impl KeyRotationRoute {
    pub fn new(router: Arc<Router>, trust: Arc<dyn TrustGate>, clock: Arc<dyn Clock>) -> Self {
        Self { router, trust, clock }
    }
}

#[async_trait]
impl PlatformRouteHandler for KeyRotationRoute {
    async fn handle(&self, inbound: InboundMessage) {
        let trust = self.trust.clone();
        let now = self.clock.now();
        let partner = inbound.source.clone();
        let request = inbound.message.clone();

        let kvp =
            match tokio::task::spawn_blocking(move || rotation_reply(trust.as_ref(), now, &partner, &request)).await {
                Ok(Some(kvp)) => kvp,
                Ok(None) => return,
                Err(e) => {
                    error!(partner = %inbound.source, error = %e, "partner key rotation task failed; no reply");
                    return;
                }
            };

        let result = result_of(&kvp);
        match reply(&self.router, &inbound, kvp).await {
            Delivery::Sent => info!(partner = %inbound.source, ?result, "partner key rotation answered"),
            Delivery::NoRequestId => {
                warn!(partner = %inbound.source, ?result, "partner key rotation without _requestId; no reply sent");
            }
            Delivery::Failed(status) => {
                warn!(partner = %inbound.source, ?result, ?status, error = status.cause(), "partner key rotation answered; the reply was not delivered");
            }
        }
    }
}

#[cfg(test)]
mod tests;
