//! The `SignalR` relay connection to `relayhub/`.
//!
//! - `connection`: one hub connection — negotiate, WebSocket upgrade, handshake, framed read loop,
//!   invocations and their completions, keepalive and server timeout, the start retry policy (HTTP
//!   401 and 429), the `OpenStatus` mapping and automatic reconnect.
//! - `negotiate`: the negotiate request, Azure-style redirects and the transport URL.
//! - `presence`: the partner set, heartbeat windows, the `OnConnected` payload and the six inbound
//!   hub methods, including the `SendConnectedAsync` presence exchange.
//! - `manager`: [`RelayManager`] — one connection per environment shared by all partners (connection
//!   sharing, keys and region) and the [`HubControl`](phonelink_core::seams::HubControl),
//!   [`HubLink`](phonelink_core::seams::HubLink) and [`WakeHub`](phonelink_core::seams::WakeHub) facade.
//!
//! Unknown or malformed inbound traffic is logged (tracing and a tap `Undecodable` event) and
//! dropped without tearing the persistent multiplex connection down; local invariants fail loudly.

mod connection;
mod manager;
mod negotiate;
mod presence;

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use tokio::sync::broadcast;

use phonelink_core::config::EngineConfig;
use phonelink_core::edge::Reachability;
use phonelink_core::identification::ClientIdentification;
use phonelink_core::seams::{PacketSink, RelayEvent, TokenSource};
use phonelink_core::tap::{Direction, Tap, TapEvent, TapLayer};
use phonelink_core::types::Environment;

pub use manager::RelayManager;
use phonelink_core::sync::RwLockExt;

/// Capacity of the [`RelayEvent`] broadcast channel; a subscriber that falls this far behind sees
/// `RecvError::Lagged`.
const EVENT_CAPACITY: usize = 1024;

/// One entry of the connection map: `(partnerDcgClientId, environment, connectionSessionId)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ConnectionKey {
    pub partner: String,
    pub env: Environment,
    pub connection_session_id: Option<String>,
}

/// What every connection of one [`RelayManager`] shares.
pub(crate) struct RelayShared {
    pub config: Arc<EngineConfig>,
    pub ident: Arc<ClientIdentification>,
    pub tokens: Arc<dyn TokenSource>,
    pub reachability: Arc<dyn Reachability>,
    pub tap: Tap,
    pub http: reqwest::Client,
    pub events: broadcast::Sender<RelayEvent>,
    pub packet_sink: RwLock<Option<Arc<dyn PacketSink>>>,
    pub keys: Mutex<HashSet<ConnectionKey>>,
    /// Session ids are unique across every connection of the manager, so a late frame of a closed
    /// connection's session can never pass the presence gate of a newer session.
    pub next_session_id: AtomicU64,
}

impl RelayShared {
    pub(crate) fn new_session_id(&self) -> u64 {
        self.next_session_id.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub(crate) fn emit(&self, event: RelayEvent) {
        // No subscriber is not an error: the relay reports, whoever listens.
        let _ = self.events.send(event);
    }

    pub(crate) fn packet_sink(&self) -> Option<Arc<dyn PacketSink>> {
        self.packet_sink.read_or_panic().clone()
    }

    pub(crate) fn internet_available(&self) -> bool {
        self.reachability.is_internet_available()
    }

    /// Reports something the relay could not decode or bind (tap `Undecodable`).
    pub(crate) fn undecodable(&self, env: Environment, what: &str, error: &str, bytes: Option<&[u8]>) {
        tracing::warn!(env = %env, what, error, "undecodable inbound hub traffic dropped");
        self.tap.emit(TapEvent::new(
            TapLayer::Undecodable,
            Direction::In,
            Some(env.name().to_string()),
            serde_json::json!({ "layer": "relay", "what": what, "error": error }),
            bytes,
        ));
    }
}
