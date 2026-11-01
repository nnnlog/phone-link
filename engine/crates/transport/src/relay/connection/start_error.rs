//! Why a start failed, and the `OpenStatus` it reports.

use super::{ReceiveClock, WsStream};
use phonelink_core::net;
use phonelink_core::seams::TokenError;
use phonelink_core::types::OpenStatus;
use std::sync::Arc;

/// Why one start (negotiate → upgrade → handshake) failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum StartError {
    #[error("services token: {0}")]
    Token(TokenError),
    /// An empty services token is a hard error, not an anonymous connect.
    #[error("the services token provider returned an empty token")]
    EmptyToken,
    #[error("HTTP {status} on {stage}")]
    Http { stage: &'static str, status: u16 },
    #[error("transport failure: {0}")]
    Transport(String),
    /// A transport failure whose cause chain shows that the network is the problem
    /// ([`net::is_internet_cause`]): its root cause is lack of internet connectivity.
    #[error("transport failure (network): {0}")]
    Network(String),
    #[error("the hub rejected the handshake: {0}")]
    HandshakeRejected(String),
    #[error("protocol failure: {0}")]
    Protocol(String),
    /// The 429 breaker is open.
    #[error("the HTTP 429 circuit breaker is open")]
    BrokenCircuit,
    #[error("cancelled")]
    Cancelled,
}

impl StartError {
    /// A transport failure described by `detail` and caused by `cause`: [`StartError::Network`] when
    /// the cause chain shows that the network is the problem, else [`StartError::Transport`].
    pub(crate) fn transport(detail: String, cause: &(dyn std::error::Error + 'static)) -> Self {
        if net::is_internet_cause(cause) { StartError::Network(detail) } else { StartError::Transport(detail) }
    }

    fn http_status(&self) -> Option<u16> {
        match self {
            StartError::Http { status, .. } => Some(*status),
            _ => None,
        }
    }

    pub(crate) fn is_401(&self) -> bool {
        self.http_status() == Some(401)
    }

    pub(crate) fn is_429(&self) -> bool {
        self.http_status() == Some(429)
    }

    /// The start retry filter: retry on anything but a failure of the MSA token provider — whether
    /// it needs the user or not — or a cancellation.
    pub(crate) fn is_retryable(&self) -> bool {
        !matches!(
            self,
            StartError::Token(TokenError::UserInteractionRequired | TokenError::Msa(_)) | StartError::Cancelled
        )
    }

    /// The `OpenStatus` of a failed start. The failure's root cause is lack of internet connectivity
    /// when the device reports none (`internet`, the reachability at the time of the failure) or its
    /// cause chain says so — [`StartError::Network`] for a hub start, and [`TokenError::Internet`]
    /// for the auth layer's internet failure; the internet check comes first for wrapped auth
    /// failures and hub failures alike.
    pub(crate) fn open_status(&self, internet: bool) -> OpenStatus {
        match self {
            StartError::Cancelled => OpenStatus::Cancelled,
            StartError::BrokenCircuit => OpenStatus::BrokenCircuitError,
            StartError::Network(_) => OpenStatus::InternetError,
            StartError::Token(_) if !internet => OpenStatus::InternetError,
            StartError::Token(e) => e.open_status(),
            // An empty token maps to no more specific status.
            StartError::EmptyToken => OpenStatus::NonTransientAuthFailure,
            StartError::Http { status: 401, .. } => OpenStatus::AuthTransientFailure,
            StartError::Http { .. }
            | StartError::Transport(_)
            | StartError::HandshakeRejected(_)
            | StartError::Protocol(_) => {
                if internet {
                    OpenStatus::Failure
                } else {
                    OpenStatus::InternetError
                }
            }
        }
    }
}

/// A started transport: the WebSocket after a successful handshake, plus any hub bytes the server
/// coalesced after the handshake's record separator.
pub(crate) struct Established {
    pub(super) ws: WsStream,
    pub(super) leftover: Vec<u8>,
    /// When the socket last delivered bytes.
    pub(super) received: Arc<ReceiveClock>,
}
