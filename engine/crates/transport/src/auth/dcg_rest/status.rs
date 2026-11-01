//! How error responses and transport failures map to `ServiceRequestStatus`.

use phonelink_core::net;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::time::Duration;

/// `ServiceRequestStatus`: the HTTP status + `categoryCode` mapping plus the local conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ServiceRequestStatus {
    Success,
    /// 400 + `categoryCode 10`.
    UnsupportedMarketError,
    /// 400, any other category.
    BadRequest,
    /// 401 + `categoryCode 1`.
    MsaUnauthorizedRequest,
    /// 401 + `categoryCode 4`.
    StrongAuthUnauthorizedRequest,
    /// 401, any other category.
    UnauthorizedRequest,
    /// 403 + `categoryCode 7`.
    LtwDisabled,
    /// 403 + `categoryCode 13`.
    UnlinkedDevice,
    /// 403, any other category or no body.
    ForbiddenDcgRequest,
    /// 404.
    NotFoundError,
    /// 429.
    TooManyRequests,
    /// 5xx.
    ServiceError,
    /// Any other status.
    ClientError,
    // Local conditions.
    InternetError,
    Cancelled,
    BrokenCircuit,
    AccessTokenRetrievalError,
    MsaTokenUserInteractionRequired,
}

impl ServiceRequestStatus {
    /// The three 401 statuses (`Unauthorized`).
    pub fn is_unauthorized(self) -> bool {
        matches!(
            self,
            ServiceRequestStatus::MsaUnauthorizedRequest
                | ServiceRequestStatus::StrongAuthUnauthorizedRequest
                | ServiceRequestStatus::UnauthorizedRequest
        )
    }

    /// The statuses that count as a *transient failure*.
    pub fn is_transient(self) -> bool {
        matches!(
            self,
            ServiceRequestStatus::NotFoundError
                | ServiceRequestStatus::TooManyRequests
                | ServiceRequestStatus::ServiceError
        )
    }
}

/// Maps a non-2xx HTTP status and the parsed `error.categoryCode` (when the body had one).
pub fn map_http_status(http: u16, category: Option<i64>) -> ServiceRequestStatus {
    use ServiceRequestStatus as S;
    match (http, category) {
        (200..=299, _) => S::Success,
        (400, Some(10)) => S::UnsupportedMarketError,
        (400, _) => S::BadRequest,
        (401, Some(1)) => S::MsaUnauthorizedRequest,
        (401, Some(4)) => S::StrongAuthUnauthorizedRequest,
        (401, _) => S::UnauthorizedRequest,
        (403, Some(7)) => S::LtwDisabled,
        (403, Some(13)) => S::UnlinkedDevice,
        (403, _) => S::ForbiddenDcgRequest,
        (404, _) => S::NotFoundError,
        (429, _) => S::TooManyRequests,
        (500..=599, _) => S::ServiceError,
        _ => S::ClientError,
    }
}

/// `error` of a non-2xx body: `{ "error": { categoryCode, code, message, details } }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorDetail {
    #[serde(default)]
    pub category_code: Option<i64>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub details: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub(super) struct ErrorEnvelope {
    error: ErrorDetail,
}

/// Parses a non-2xx body; anything that is not the error envelope counts as *no body*.
pub fn parse_error_body(body: &str) -> Option<ErrorDetail> {
    serde_json::from_str::<ErrorEnvelope>(body).ok().map(|e| e.error)
}

/// A non-2xx reply.
#[derive(Debug, Clone, PartialEq)]
pub struct ServiceFailure {
    pub call: &'static str,
    pub http_status: u16,
    pub status: ServiceRequestStatus,
    pub error: Option<ErrorDetail>,
    pub body: String,
}

impl ServiceFailure {
    pub fn code(&self) -> Option<&str> {
        self.error.as_ref().and_then(|e| e.code.as_deref())
    }
}

/// Why a request got no usable HTTP reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransportCause {
    /// The network is the cause — `InternetError`: the connection could not be established (the
    /// host name did not resolve, the TCP connection, the TLS handshake or a proxy tunnel failed),
    /// or the failure's cause chain holds a network that is down or unreachable, an unreachable
    /// host, or an aborted or reset connection ([`net::is_internet_cause`]).
    Internet,
    /// The request timed out — `Cancelled`.
    TimedOut,
    /// Anything else, e.g. a reply that broke off without one of the causes above — no status.
    Other,
}

impl TransportCause {
    /// Classifies a request that failed after it was built: reqwest's connect phase (name
    /// resolution through [`net::TypedResolver`], TCP connect, TLS handshake, proxy) is
    /// `Internet` whatever went wrong there; otherwise a timeout is `TimedOut`, an internet cause
    /// anywhere in the error's chain is `Internet`, and the rest is `Other`. The engine sets no
    /// connect timeout of its own, so a timed-out connect is the operating system's (`Internet`).
    pub fn of(error: &reqwest::Error) -> Self {
        if error.is_connect() {
            TransportCause::Internet
        } else if error.is_timeout() {
            TransportCause::TimedOut
        } else if net::is_internet_cause(error) {
            TransportCause::Internet
        } else {
            TransportCause::Other
        }
    }
}

/// `error` and its causes, joined by `: ` (reqwest's own text names none of them).
pub(super) fn error_chain(error: &reqwest::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();

    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// Why a REST call did not produce a usable result.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RestError {
    /// A non-2xx reply, mapped to a [`ServiceRequestStatus`].
    #[error("{} answered HTTP {} ({:?}, code {:?})", .0.call, .0.http_status, .0.status, .0.code())]
    Status(Box<ServiceFailure>),
    /// No HTTP reply, or one whose body could not be read: `cause` is `InternetError`, `Cancelled`
    /// or no status ([`TransportCause`]). The device's reported connectivity plays no part.
    #[error("{call}: transport failure ({cause:?}): {detail}")]
    Transport { call: &'static str, cause: TransportCause, detail: String },
    /// A 2xx reply whose body breaks the contract.
    #[error("{call}: unexpected response: {detail}")]
    InvalidResponse { call: &'static str, detail: String },
    /// A precondition of the request failed locally (an empty token, a request that could not be
    /// built); nothing was sent.
    #[error("{call}: {detail}")]
    Local { call: &'static str, detail: String },
    /// The resilience policy ended the call without a reply to map ([`PolicyFailure`]).
    #[error("{call}: {failure}")]
    Policy { call: &'static str, failure: PolicyFailure },
}

/// How the resilience policy ended a call without a reply to map: an open circuit is
/// `BrokenCircuit`; the others have no status.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyFailure {
    /// The circuit of the call's service client is open; nothing was sent.
    #[error("circuit open for another {retry_after:?}")]
    BrokenCircuit { retry_after: Duration },
    /// The call's service client already had `limit` calls in flight; nothing was sent.
    #[error("{limit} calls already in flight")]
    ConcurrencyLimit { limit: usize },
    /// Still answered `status` (503, 504 or 429) after `sends` sends of one attempt.
    #[error("HTTP {status} after {sends} sends")]
    ResendsExhausted { status: u16, sends: u32 },
    /// A 503, 504 or 429 whose `Retry-After` is a negative number of seconds.
    #[error("negative Retry-After {value:?}")]
    NegativeRetryAfter { value: String },
}

impl RestError {
    pub fn call(&self) -> &'static str {
        match self {
            RestError::Status(f) => f.call,
            RestError::Transport { call, .. }
            | RestError::InvalidResponse { call, .. }
            | RestError::Local { call, .. }
            | RestError::Policy { call, .. } => call,
        }
    }

    /// The mapped status of a non-2xx reply; for a transport failure `InternetError` when its cause
    /// is the network and `Cancelled` when it timed out; `BrokenCircuit` for an open circuit.
    pub fn status(&self) -> Option<ServiceRequestStatus> {
        match self {
            RestError::Status(f) => Some(f.status),
            RestError::Policy { failure: PolicyFailure::BrokenCircuit { .. }, .. } => {
                Some(ServiceRequestStatus::BrokenCircuit)
            }
            RestError::Transport { cause: TransportCause::Internet, .. } => Some(ServiceRequestStatus::InternetError),
            RestError::Transport { cause: TransportCause::TimedOut, .. } => Some(ServiceRequestStatus::Cancelled),
            _ => None,
        }
    }

    fn has_status(&self, s: ServiceRequestStatus) -> bool {
        self.status() == Some(s)
    }

    fn bad_request_code(&self, code: &str) -> bool {
        matches!(self, RestError::Status(f)
            if f.status == ServiceRequestStatus::BadRequest && f.code() == Some(code))
    }

    /// 400 + `error.code == "InvalidNonce"` → retry with a fresh nonce.
    pub fn is_invalid_nonce(&self) -> bool {
        self.bad_request_code("InvalidNonce")
    }

    /// 400 + `error.code == "InvalidCertificate"` → `InvalidIdentity`.
    pub fn is_invalid_certificate(&self) -> bool {
        self.bad_request_code("InvalidCertificate")
    }

    /// 403 + `categoryCode 13` → device unlinked.
    pub fn is_unlinked(&self) -> bool {
        self.has_status(ServiceRequestStatus::UnlinkedDevice)
    }

    /// 403 without a known category (`ForbiddenDcgRequest`).
    pub fn is_forbidden_uncategorized(&self) -> bool {
        self.has_status(ServiceRequestStatus::ForbiddenDcgRequest)
    }

    /// Any 401 status.
    pub fn is_unauthorized(&self) -> bool {
        self.status().is_some_and(ServiceRequestStatus::is_unauthorized)
    }

    pub fn is_internet(&self) -> bool {
        self.has_status(ServiceRequestStatus::InternetError)
    }
}
