//! The connection map and the relay facade: connection sharing, keys and region, the outbound hub
//! methods, the partner set and the `SendConnectedAsync` exchange, and the `OnConnected` payload.
//!
//! One hub connection per environment, shared by all partners of that environment. Every
//! connection the engine opens is PC-initiated and requests no region, so the reuse rule
//! ("environment matches and the requested region is null or equal") always reuses the
//! environment's connection; `DCG-PartnerId` is the partner the connection was first created for.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::broadcast;

use super::connection::{ConnectionParams, HubConnection};
use super::negotiate::HTTP_TIMEOUT;
use super::presence::Presence;
use super::{ConnectionKey, EVENT_CAPACITY, RelayShared};
use crate::dcg::HubOpener;
use phonelink_core::codec::hub::{self, MultiplexPacket, OnConnectedPayload};
use phonelink_core::config::EngineConfig;
use phonelink_core::edge::Reachability;
use phonelink_core::identification::ClientIdentification;
use phonelink_core::seams::{HubControl, HubLink, HubRegionTimeout, PacketSink, RelayEvent, TokenSource, WakeHub};
use phonelink_core::sync::{MutexExt, RwLockExt};
use phonelink_core::tap::Tap;
use phonelink_core::types::{
    ConnectReason, DcgClient, Environment, HubSendResult, HubState, Opened, PartnerDisconnectReason, TraceContext,
};

struct EnvSlot {
    presence: Arc<Presence>,
    connection: Mutex<Option<Arc<HubConnection>>>,
    /// Held while a connection is created or closed-and-removed, so a new connection never
    /// coexists with the closing one on the environment's presence.
    lifecycle: tokio::sync::Mutex<()>,
}

/// The relay layer: the hub connections of all environments and their partner presence.
pub struct RelayManager {
    shared: Arc<RelayShared>,
    slots: HashMap<Environment, Arc<EnvSlot>>,
}

impl RelayManager {
    /// Must be called inside a Tokio runtime only when connections are opened (the constructor
    /// spawns nothing).
    pub fn new(
        config: Arc<EngineConfig>,
        ident: Arc<ClientIdentification>,
        tokens: Arc<dyn TokenSource>,
        reachability: Arc<dyn Reachability>,
        tap: Tap,
    ) -> Arc<Self> {
        // Name-resolution failures stay typed, so a negotiate that fails for want of the network
        // opens as `InternetError` (`crate::auth::network`).
        let http = phonelink_core::net::http_client(HTTP_TIMEOUT)
            .expect("building the negotiate HTTP client cannot fail with the compiled-in TLS backend");

        let shared = Arc::new(RelayShared {
            config,
            ident,
            tokens,
            reachability,
            tap,
            http,
            events: broadcast::channel(EVENT_CAPACITY).0,
            packet_sink: RwLock::new(None),
            keys: Mutex::new(HashSet::new()),
            next_session_id: std::sync::atomic::AtomicU64::new(0),
        });

        let slots = Environment::ALL
            .into_iter()
            .map(|env| {
                (
                    env,
                    Arc::new(EnvSlot {
                        presence: Presence::new(env, shared.clone()),
                        connection: Mutex::new(None),
                        lifecycle: tokio::sync::Mutex::new(()),
                    }),
                )
            })
            .collect();

        Arc::new(Self { shared, slots })
    }

    /// The PC's session was unlocked or it resumed from sleep: every partner of every connection is
    /// removed with reason `SystemReset`, which re-wakes it.
    pub fn system_reset(&self) {
        for slot in self.slots.values() {
            for id in slot.presence.partners() {
                slot.presence.remove(None, &id, PartnerDisconnectReason::SystemReset);
            }
        }
    }

    /// Where `OnReceiveMessage` / `OnReceiveSessionBasedMessage` packets go (the DCG layer).
    pub fn set_packet_sink(&self, sink: Arc<dyn PacketSink>) {
        *self.shared.packet_sink.write_or_panic() = Some(sink);
    }

    fn slot(&self, env: Environment) -> &Arc<EnvSlot> {
        self.slots.get(&env).expect("every environment has a slot")
    }

    fn connection(&self, env: Environment) -> Option<Arc<HubConnection>> {
        self.slot(env).connection.lock_or_panic().clone()
    }

    /// Gets the connection of `partner.env`, creating it for `partner` (its `DCG-PartnerId`) with
    /// `trace` as the trace context of its connection headers.
    fn get_or_create(&self, partner: &DcgClient, trace: TraceContext) -> Arc<HubConnection> {
        let slot = self.slot(partner.env);
        let mut connection = slot.connection.lock_or_panic();

        if let Some(existing) = connection.as_ref() {
            return existing.clone();
        }

        let created = HubConnection::new(
            ConnectionParams { env: partner.env, partner_id: Some(partner.id.clone()), trace },
            self.shared.clone(),
            slot.presence.clone(),
        );
        tracing::debug!(env = %partner.env, partner = %partner.id, "hub connection created");
        *connection = Some(created.clone());
        created
    }

    /// Opens (or reuses) the connection of `partner`'s environment, the connection headers carrying
    /// `trace` when this call creates it.
    pub async fn open_with_trace(&self, partner: &DcgClient, reason: ConnectReason, trace: TraceContext) -> Opened {
        assert!(!partner.id.is_empty(), "a partner connection needs a non-empty dcgClientId");

        let connection = {
            let _lifecycle = self.slot(partner.env).lifecycle.lock().await;
            self.get_or_create(partner, trace)
        };

        self.shared.keys.lock_or_panic().insert(ConnectionKey {
            partner: partner.id.clone(),
            env: partner.env,
            connection_session_id: None,
        });
        connection.open(reason).await
    }

    /// `SendConnectedAsync(target)`, awaited for its completion.
    pub async fn send_connected_to(&self, target: &DcgClient) -> HubSendResult {
        assert!(!target.id.is_empty(), "SendConnectedAsync needs a non-empty targetDcgClientId");
        match self.connection(target.env) {
            Some(connection) => connection.send_connected(&target.id).await,
            None => HubSendResult::NotConnected,
        }
    }

    /// The hub reports `partner` connected (it is in its environment's partner set). The
    /// [`HubControl`] and [`HubLink`] methods of the same name answer this.
    pub fn is_partner_connected(&self, partner: &DcgClient) -> bool {
        self.slot(partner.env).presence.contains(&partner.id)
    }

    /// The resolved region of `env`'s connection (`OnConnected.RegionName`), surfaced to upper layers.
    pub fn resolved_region(&self, env: Environment) -> Option<String> {
        self.slot(env).presence.on_connected().and_then(|p| p.region_name)
    }

    /// The last `OnConnected` payload of `env`'s connection, kept until the connection closes.
    pub fn on_connected_payload(&self, env: Environment) -> Option<OnConnectedPayload> {
        self.slot(env).presence.on_connected()
    }

    /// The partner set of `env`'s connection, ordered by id.
    pub fn partners(&self, env: Environment) -> Vec<DcgClient> {
        self.slot(env).presence.partners().into_iter().map(|id| DcgClient::new(id, env)).collect()
    }
}

#[async_trait]
impl HubControl for RelayManager {
    async fn open(&self, partner: &DcgClient, reason: ConnectReason, trace: &TraceContext) -> Opened {
        self.open_with_trace(partner, reason, trace.clone()).await
    }

    fn hub_state(&self, env: Environment) -> HubState {
        self.connection(env).map_or(HubState::Disconnected, |c| c.state())
    }

    fn is_partner_connected(&self, partner: &DcgClient) -> bool {
        RelayManager::is_partner_connected(self, partner)
    }

    async fn send_connected(&self, partner: &DcgClient) -> HubSendResult {
        self.send_connected_to(partner).await
    }

    fn subscribe(&self) -> broadcast::Receiver<RelayEvent> {
        self.shared.events.subscribe()
    }

    /// Stops `env`'s connection and removes it with its keys, so the next open creates a new one.
    ///
    /// The close runs in its own task, so a caller that stops waiting (a timeout, a `select!`)
    /// cannot leave it half done: a connection taken out of its slot is always disposed, with its
    /// `Closed` reaction on the environment's presence, before the next open may create a new one.
    async fn close(&self, env: Environment) {
        let slot = self.slot(env).clone();
        let shared = self.shared.clone();
        let task = tokio::spawn(async move {
            let _lifecycle = slot.lifecycle.lock().await;
            let connection = slot.connection.lock_or_panic().take();
            if let Some(connection) = connection {
                connection.dispose().await;
                shared.keys.lock_or_panic().retain(|k| k.env != env);
            }
        });

        if let Err(e) = task.await {
            if e.is_panic() {
                std::panic::resume_unwind(e.into_panic());
            }
            unreachable!("the close task is never aborted: {e}");
        }
    }
}

#[async_trait]
impl HubLink for RelayManager {
    /// `SendMessageAsync` without, `SendSessionBasedMessageAsync` with a `connection_session_id`;
    /// both go over the environment's connection. A send never opens a connection: the fragment
    /// layer opens it before each fragment try and ack (`dcg::HubOpener`, implemented over
    /// `open_with_trace` in `engine`), the per-partner connection through `HubControl::open`.
    async fn send_packet(
        &self,
        target: &DcgClient,
        trace: &TraceContext,
        packet: MultiplexPacket,
        connection_session_id: Option<String>,
    ) -> HubSendResult {
        assert!(!target.id.is_empty(), "a hub send needs a non-empty targetDcgClientId");
        let Some(connection) = self.connection(target.env) else {
            return HubSendResult::NotConnected;
        };

        let (method, arguments) =
            hub::send_message_arguments(trace, &target.id, &packet, connection_session_id.as_deref());
        connection.invoke(method, arguments).await
    }

    fn is_partner_connected(&self, partner: &DcgClient) -> bool {
        RelayManager::is_partner_connected(self, partner)
    }

    fn remove_partner(&self, partner: &DcgClient, reason: PartnerDisconnectReason) {
        if !self.slot(partner.env).presence.remove(None, &partner.id, reason) {
            tracing::debug!(partner = %partner, ?reason, "remove_partner for a partner not in the set");
        }
    }
}

#[async_trait]
impl WakeHub for RelayManager {
    async fn flush_connected(&self, target: &DcgClient) -> HubSendResult {
        self.send_connected_to(target).await
    }

    async fn hub_region(&self, env: Environment, timeout: Duration) -> Result<Option<String>, HubRegionTimeout> {
        self.slot(env).presence.wait_on_connected(timeout).await.map(|payload| payload.region_name)
    }

    async fn wait_partner_connected(&self, target: &DcgClient, timeout: Duration) -> bool {
        self.slot(target.env).presence.wait_partner(&target.id, timeout).await
    }
}

/// The fragment layer opens the hub connection serving a partner before each fragment try and each
/// ack: the relay's open, which uses an open connection as it is.
#[async_trait]
impl HubOpener for RelayManager {
    async fn open_for(&self, partner: &DcgClient, reason: ConnectReason, trace: &TraceContext) -> Opened {
        self.open_with_trace(partner, reason, trace.clone()).await
    }
}

#[cfg(test)]
mod tests;
