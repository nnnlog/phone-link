//! Opening a Nano connection.
//!
//! It covers the attempts each path races, the exchange of offer and answer over DCG —
//! `/nanohandshake` App requests on the screen-mirroring path, context messages on the
//! device-connection path — and the race, whose first attempt to open and complete wins.

use std::future::Future;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::stream::{FuturesUnordered, StreamExt};
use prost::Message as _;
use serde_json::{Map, Value, json};
use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, info, warn};

use super::cloud_relay::{RelayServersError, RelaySource};
use super::connection::{CloseReason, ConnectObject, ConnectOptions, ConnectionType, NanoConnection};
use super::contract::IceServers;
use super::netif::HostAddress;
use phonelink_core::config::NanoConfig;
use phonelink_core::types::{TraceContext, new_guid_lower};
use phonelink_proto::platform_sdk::v1::context_source_message::Type;
use phonelink_proto::platform_sdk::v1::{
    ContextSourceMessage, CsConnectionInfo, CsInitializeMessage, CsInitializeStatus,
};
use phonelink_transport::platform::context_message::proto_trace;
use phonelink_transport::platform::{ContextHandle, ContextRoute, ContextSendError};

/// The phone's `remoting` capability bits.
pub mod remoting {
    pub const ICE_ENABLED: i32 = 0x20;
    pub const MULTIPLE_CONNECTIONS: i32 = 0x200;
    pub const PLATFORM_NANO: i32 = 0x10000;
}

/// The screen-mirroring path's transport types (`settings.connectionType`).
pub mod mirroring_transport {
    pub const IP: i32 = 0;
    pub const ICE: i32 = 1;
    pub const ICE_STUN: i32 = 2;
    pub const ICE_STUN_AND_TURN: i32 = 3;
}

/// The device-connection path's transport types (`connectiontype`).
pub mod device_transport {
    pub const IP: i32 = 0;
    pub const ICE_LOCAL: i32 = 1;
    pub const ICE_STUN: i32 = 2;
    pub const ICE_TURN: i32 = 3;
}

/// The phone's App contract version from which it takes ICE offers.
const ICE_CONTRACT_VERSION: f64 = 3.99;

/// Why one attempt failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AttemptError {
    #[error("the phone refused the handshake: result {result}, reason {reason:?}")]
    Rejected { result: i32, reason: Option<String> },
    #[error("the phone's answer is unusable: {0}")]
    BadAnswer(String),
    #[error("the handshake exchange failed: {0}")]
    Exchange(String),
    /// The handshake request was not delivered or not answered: the screen-mirroring path's send
    /// status (`600 +` the cause).
    #[error("the handshake request was not answered: status {status}")]
    Transport { status: i32 },
    #[error("the context handle was dropped")]
    ContextDropped,
    #[error("the phone's connection information did not parse: {0}")]
    FailedToReceive(String),
    #[error(transparent)]
    Connection(#[from] CloseReason),
    #[error("the connection did not complete: {0}")]
    Completion(String),
    #[error("no attempt to run")]
    NoAttempt,
    #[error("the attempt timed out")]
    Timeout,
}

/// One attempt of a race.
#[derive(Debug, Clone)]
pub struct Attempt {
    /// The path's own transport-type value, sent in the handshake.
    pub transport: i32,
    pub kind: ConnectionType,
    pub servers: IceServers,
    /// Matches the phone's answer on the device-connection path.
    pub connection_id: String,
    /// The first attempt of a race.
    pub first: bool,
    pub multiple_connections: bool,
    /// Diagnostics: offer and pair only relayed candidates.
    pub relay_only: bool,
}

impl Attempt {
    pub fn new(transport: i32, kind: ConnectionType, servers: IceServers) -> Self {
        Self {
            transport,
            kind,
            servers,
            connection_id: new_guid_lower(),
            first: false,
            multiple_connections: true,
            relay_only: false,
        }
    }
}

/// Sends an attempt's offer to the phone and returns its answer object.
#[async_trait]
pub trait Exchange: Send + Sync {
    async fn exchange(&self, attempt: &Attempt, offer: Map<String, Value>) -> Result<Value, AttemptError>;
}

/// Runs `attempts` at the same time: each initializes its connect object, exchanges offer and answer,
/// opens, then `complete`s; the first to complete wins and the others are closed.
///
/// With every attempt failed, the last failure is returned (its caller logs it); each earlier one
/// is logged here, where the race drops it.
pub async fn race<T, F, Fut>(
    attempts: Vec<Attempt>,
    exchange: Arc<dyn Exchange>,
    hosts: Option<Vec<HostAddress>>,
    attempt_timeout: Duration,
    complete: F,
) -> Result<T, AttemptError>
where
    F: Fn(NanoConnection) -> Fut,
    Fut: Future<Output = Result<T, AttemptError>>,
{
    if attempts.is_empty() {
        return Err(AttemptError::NoAttempt);
    }

    let mut running = FuturesUnordered::new();
    for attempt in attempts {
        let exchange = exchange.clone();
        let hosts = hosts.clone();
        let complete = &complete;

        running.push(async move {
            let transport = attempt.transport;
            let run = async {
                let options = ConnectOptions {
                    kind: attempt.kind,
                    servers: attempt.servers.clone(),
                    host_addresses: hosts,
                    open_timeout: None,
                    relay_only: attempt.relay_only,
                };

                let (object, offer) = ConnectObject::initialize(options).await?;
                let answer = exchange.exchange(&attempt, offer).await?;
                let connection = object.open(answer).await?;
                complete(connection).await
            };

            let result = tokio::time::timeout(attempt_timeout, run).await.unwrap_or(Err(AttemptError::Timeout));
            (transport, result)
        });
    }

    let mut last = AttemptError::NoAttempt;
    while let Some((transport, result)) = running.next().await {
        match result {
            Ok(t) => {
                // The winner's connection logs its opening; this names the attempt behind it.
                debug!(transport, "Nano attempt won");
                return Ok(t);
            }
            Err(e) if !running.is_empty() => {
                info!(transport, error = %e, "Nano attempt failed; the other attempts go on");
                last = e;
            }
            Err(e) => last = e,
        }
    }

    Err(last)
}

// ------------------------------------------------------------------------------------------------
// Screen-mirroring path
// ------------------------------------------------------------------------------------------------

/// What the phone's `connect` reply said about remoting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhoneRemoting {
    /// The `remoting` flags.
    pub flags: i32,
    /// The phone's App contract version.
    pub contract_version: f64,
}

impl PhoneRemoting {
    pub fn ice_supported(&self) -> bool {
        self.flags & remoting::ICE_ENABLED != 0 && self.contract_version >= ICE_CONTRACT_VERSION
    }
}

/// Logs that `CloudRelay` gave no servers, so the race goes without the `skipped` attempts: `warn`
/// when the request failed, `debug` when no relay source is configured.
fn log_no_relay(error: &RelayServersError, skipped: &'static str) {
    match error {
        RelayServersError::NoSource => debug!(skipped, "no relay source; attempts skipped"),
        RelayServersError::Token(_) | RelayServersError::Rest(_) | RelayServersError::Json(_) => {
            warn!(%error, skipped, "CloudRelay servers not received; attempts skipped");
        }
    }
}

/// The screen-mirroring path's attempts: `Ip`, and with ICE support `Ice` and `IceStun` (when a STUN
/// server is known), plus `IceStunAndTurn` when configured and a TURN server is available.
pub async fn mirroring_attempts(config: &NanoConfig, phone: PhoneRemoting, relay: &dyn RelaySource) -> Vec<Attempt> {
    if !phone.ice_supported() {
        let mut ip = Attempt::new(mirroring_transport::IP, ConnectionType::IpSrtp, IceServers::default());
        ip.multiple_connections = false;
        ip.first = true;
        return vec![ip];
    }

    let mut attempts = vec![
        Attempt::new(mirroring_transport::IP, ConnectionType::IpSrtp, IceServers::default()),
        Attempt::new(mirroring_transport::ICE, ConnectionType::IceSrtp, IceServers::default()),
    ];

    let stun = if config.mirroring_stun_from_cloud_relay {
        relay.relay_servers(true).await.map(|r| r.stun_urls.first().cloned())
    } else {
        Ok(config.mirroring_stun_servers.split(',').next().map(|s| s.trim().to_string()))
    }
    .map(|url| url.filter(|s| !s.is_empty()));

    match stun {
        Ok(Some(url)) => attempts.push(Attempt::new(
            mirroring_transport::ICE_STUN,
            ConnectionType::IceSrtp,
            IceServers { stun_url: url, ..Default::default() },
        )),
        Ok(None) => debug!("no STUN server; the IceStun attempt is skipped"),
        Err(e) => log_no_relay(&e, "IceStun"),
    }

    if config.mirroring_turn {
        match relay.relay_servers(false).await {
            Ok(r) if !r.turn_urls.is_empty() => attempts.push(Attempt::new(
                mirroring_transport::ICE_STUN_AND_TURN,
                ConnectionType::IceSrtp,
                IceServers {
                    stun_url: r.stun_urls.first().cloned().unwrap_or_default(),
                    turn_url: r.turn_urls[0].clone(),
                    turn_username: r.turn_username,
                    turn_password: r.turn_credential,
                },
            )),
            Ok(_) => debug!("CloudRelay names no TURN server; the IceStunAndTurn attempt is skipped"),
            Err(e) => log_no_relay(&e, "IceStunAndTurn"),
        }
    } else {
        let bits = config.mirroring_attempts & 0x7;
        if bits != 0 {
            attempts.retain(|a| match a.transport {
                mirroring_transport::IP => bits & 1 != 0,
                mirroring_transport::ICE => bits & 2 != 0,
                mirroring_transport::ICE_STUN => bits & 4 != 0,
                _ => true,
            });
        }
    }

    if let Some(first) = attempts.first_mut() {
        first.first = true;
    }

    attempts
}

/// Opens the screen-mirroring path's connection: its attempts raced over `handshake`, each
/// completed by `complete` (the phone's `launcher` channel opening is the feature's).
///
/// The winner closes as lost when it misses packets for longer than the configured limit.
pub async fn open_mirroring<T, F, Fut>(
    config: &NanoConfig,
    phone: PhoneRemoting,
    relay: &dyn RelaySource,
    handshake: Arc<dyn Exchange>,
    hosts: Option<Vec<HostAddress>>,
    complete: F,
) -> Result<T, AttemptError>
where
    F: Fn(NanoConnection) -> Fut,
    Fut: Future<Output = Result<T, AttemptError>>,
{
    let attempts = mirroring_attempts(config, phone, relay).await;
    let unrecovered = config.mirroring_unrecovered_close;

    race(attempts, handshake, hosts, config.attempt_timeout, |connection| {
        connection.close_when_unrecovered(unrecovered);
        complete(connection)
    })
    .await
}

/// The process's connection-service instance id: one per process, so the phone recognises a new PC
/// instance.
pub static INSTANCE_ID: LazyLock<String> = LazyLock::new(new_guid_lower);

// ------------------------------------------------------------------------------------------------
// Device-connection path
// ------------------------------------------------------------------------------------------------

/// Why the device-connection initialization failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InitializeError {
    #[error("the context handle was dropped")]
    ContextDropped,
    #[error("initialize not delivered: {0}")]
    Send(#[from] ContextSendError),
    #[error("no initialize acknowledgement in time")]
    Timeout,
    #[error("the phone's device connection failed")]
    RemoteDeviceConnectionFailure,
    #[error("the phone refused the initialize: status {0}")]
    PartnerChannelInitFailure(i32),
}

/// Signals initialization on the context and waits for the phone's acknowledgement.
pub async fn initialize_context(
    handle: &ContextHandle,
    transport_preference: i32,
    timeout: Duration,
) -> Result<(), InitializeError> {
    let mut inbound = handle.subscribe();
    let trace = TraceContext::new_root(Vec::new());
    let message = ContextSourceMessage {
        r#type: Some(Type::CsInitializeMessage(CsInitializeMessage {
            context_source_id: handle.context_source_id().to_string(),
            trace_context: Some(proto_trace(&trace)),
            transport_preference,
        })),
    };

    handle.send(&message, ContextRoute::Initialize, trace).await?;

    let dropped = handle.dropped();
    let wait = async {
        loop {
            match inbound.recv().await {
                Ok(ContextSourceMessage { r#type: Some(Type::CsInitializeAckMessage(ack)) }) => {
                    return match CsInitializeStatus::try_from(ack.status) {
                        Ok(CsInitializeStatus::Success) => Ok(()),
                        Ok(CsInitializeStatus::ErrorDeviceConnectionFailure) => {
                            Err(InitializeError::RemoteDeviceConnectionFailure)
                        }
                        _ => Err(InitializeError::PartnerChannelInitFailure(ack.status)),
                    };
                }
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return Err(InitializeError::ContextDropped),
            }
        }
    };

    tokio::select! {
        r = tokio::time::timeout(timeout, wait) => r.unwrap_or(Err(InitializeError::Timeout)),
        _ = dropped.cancelled() => Err(InitializeError::ContextDropped),
    }
}

/// The device-connection path's attempts: `Ip` and `IceLocal`; `IceStun` and `IceTurn` when `CloudRelay`
/// returned both STUN and TURN servers.
///
/// STUN and TURN URLs are space-joined, unchanged. There is never
/// a Wi-Fi Direct attempt (differs from Phone Link on Windows, which can add one): macOS has no public
/// API to join a Wi-Fi Direct group.
pub async fn device_attempts(relay: &dyn RelaySource) -> Vec<Attempt> {
    let mut attempts = vec![
        Attempt::new(device_transport::IP, ConnectionType::IpSrtp, IceServers::default()),
        Attempt::new(device_transport::ICE_LOCAL, ConnectionType::IceSrtp, IceServers::default()),
    ];

    match relay.relay_servers(false).await {
        Ok(r) if r.has_stun_and_turn() => {
            let stun = r.stun_urls.join(" ");
            attempts.push(Attempt::new(
                device_transport::ICE_STUN,
                ConnectionType::IceSrtp,
                IceServers { stun_url: stun.clone(), ..Default::default() },
            ));
            attempts.push(Attempt::new(
                device_transport::ICE_TURN,
                ConnectionType::IceSrtp,
                IceServers {
                    stun_url: stun,
                    turn_url: r.turn_urls.join(" "),
                    turn_username: r.turn_username,
                    turn_password: r.turn_credential,
                },
            ));
        }
        Ok(_) => debug!("CloudRelay lacks STUN or TURN; the IceStun and IceTurn attempts are skipped"),
        Err(e) => log_no_relay(&e, "IceStun and IceTurn"),
    }

    if let Some(first) = attempts.first_mut() {
        first.first = true;
    }

    attempts
}

/// The context-message exchange of one attempt.
pub struct ContextExchange {
    pub handle: ContextHandle,
}

impl ContextExchange {
    /// `{"connectioninformation": offer, "connectionid", "connectiontype", "multipleconnections"}`.
    pub fn wrapper(attempt: &Attempt, offer: Map<String, Value>) -> String {
        json!({
            "connectioninformation": Value::Object(offer),
            "connectionid": attempt.connection_id,
            "connectiontype": attempt.transport,
            "multipleconnections": attempt.multiple_connections,
        })
        .to_string()
    }
}

#[async_trait]
impl Exchange for ContextExchange {
    async fn exchange(&self, attempt: &Attempt, offer: Map<String, Value>) -> Result<Value, AttemptError> {
        if self.handle.is_dropped() {
            return Err(AttemptError::ContextDropped);
        }

        let mut inbound = self.handle.subscribe();
        let trace = TraceContext::new_root(Vec::new());
        let message = ContextSourceMessage {
            r#type: Some(Type::CsConnectionInfo(CsConnectionInfo {
                connection_info: Self::wrapper(attempt, offer),
                trace_context: Some(proto_trace(&trace)),
            })),
        };

        let dropped = self.handle.dropped();
        let send_and_wait = async {
            self.handle.send(&message, ContextRoute::Own, trace).await.map_err(|e| match e {
                ContextSendError::Dropped => AttemptError::ContextDropped,
                other => AttemptError::Exchange(other.to_string()),
            })?;

            loop {
                match inbound.recv().await {
                    Ok(ContextSourceMessage { r#type: Some(Type::CsConnectionInfo(info)) }) => {
                        // Not JSON, or no `connectionid`: dropped. A matching id whose
                        // `connectioninformation` cannot be read fails this attempt.
                        let parsed: Value = match serde_json::from_str(&info.connection_info) {
                            Ok(v) => v,
                            Err(e) => {
                                warn!(error = %e, "connection information that is not JSON dropped");
                                continue;
                            }
                        };

                        let id = match parsed.get("connectionid") {
                            Some(Value::String(s)) => s.clone(),
                            Some(other) => other.to_string(),
                            None => {
                                warn!("connection information without connectionid dropped");
                                continue;
                            }
                        };

                        if id != attempt.connection_id {
                            debug!("connection information for another attempt ignored");
                            continue;
                        }

                        return parsed
                            .get("connectioninformation")
                            .filter(|v| v.is_object())
                            .cloned()
                            .ok_or_else(|| AttemptError::FailedToReceive("no connectioninformation".into()));
                    }
                    Ok(other) => debug!(len = other.encoded_len(), "other context message while exchanging"),
                    Err(RecvError::Lagged(n)) => {
                        warn!(n, "context messages lagged; the skipped ones are lost to this attempt")
                    }
                    Err(RecvError::Closed) => return Err(AttemptError::ContextDropped),
                }
            }
        };

        tokio::select! {
            r = send_and_wait => r,
            _ = dropped.cancelled() => Err(AttemptError::ContextDropped),
        }
    }
}

#[cfg(test)]
mod tests;
