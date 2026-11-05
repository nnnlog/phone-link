//! A diagnostic run of the Nano path against the selected phone, for a process started with
//! `PHONELINK_NANO_PROBE` set.
//!
//! It covers once the Cross Device session is connected, open the device connection, create an
//! `InternalMessaging` platform channel (the phone accepts it without asking a feature), log each
//! step's outcome and time and the connection metrics, then close.
//!
//! It sends nothing a feature
//! would act on. `PHONELINK_NANO_PROBE=relay` instead opens the device connection with one attempt
//! that offers and pairs only relayed candidates, and holds it 50 s (past the relay's 40 s refresh).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{info, warn};

use super::cloud_relay::{CloudRelay, RelayServersError, RelaySource};
use super::connect::{Attempt, ContextExchange, device_transport, initialize_context, race};
use super::connection::{ConnectObject, ConnectOptions, ConnectionType};
use super::contract::IceServers;
use super::device::{DeviceConnection, DeviceConnections, PlatformChannelType, scenario};
use super::ice::candidate::{CandidateType, SessionDescription};
use phonelink_core::config::NanoConfig;
use phonelink_core::seams::TokenSource;
use phonelink_core::types::DcgClient;
use phonelink_transport::auth::AuthService;
use phonelink_transport::platform::PlatformRoutes;
use phonelink_transport::platform::session::{PartnerSessions, WaitOutcome};

/// The variable that turns the probe on.
pub const PROBE_VAR: &str = "PHONELINK_NANO_PROBE";

/// Whether this process was started with the probe on.
pub fn enabled() -> bool {
    std::env::var_os(PROBE_VAR).is_some()
}

/// Gathers candidates with the `CloudRelay` servers as an `IceTurn` attempt would, without the phone,
/// and logs how many of each kind came back.
async fn gather(r: &super::cloud_relay::RelayServers, step: &impl Fn(&str, bool, String)) {
    let servers = IceServers {
        stun_url: r.stun_urls.join(" "),
        turn_url: r.turn_urls.join(" "),
        turn_username: r.turn_username.clone(),
        turn_password: r.turn_credential.clone(),
    };
    let options = ConnectOptions { servers, ..ConnectOptions::new(ConnectionType::IceSrtp) };

    match ConnectObject::initialize(options).await {
        Ok((object, offer)) => {
            let text = offer.get("ice").and_then(|i| i.get("candidates")).and_then(|c| c.as_str()).unwrap_or("");
            let detail = match SessionDescription::parse(text) {
                Ok(d) => {
                    let count = |k: CandidateType| d.candidates.iter().filter(|c| c.kind == k).count();
                    format!(
                        "host={} srflx={} relayed={}",
                        count(CandidateType::Host),
                        count(CandidateType::ServerReflexive),
                        count(CandidateType::Relayed)
                    )
                }
                Err(e) => e.to_string(),
            };

            step("gather", true, detail);
            object.close();
        }
        Err(e) => step("gather", false, e.to_string()),
    }
}

/// The probe was started as `PHONELINK_NANO_PROBE=relay`.
fn relay_only() -> bool {
    std::env::var(PROBE_VAR).is_ok_and(|v| v == "relay")
}

/// Opens the device connection with one `IceTurn` attempt that offers and pairs only relayed
/// candidates, so the path must go through the relay.
async fn open_relayed(
    handle: &phonelink_transport::platform::ContextHandle,
    config: &NanoConfig,
    r: &super::cloud_relay::RelayServers,
    phone: DcgClient,
) -> Result<DeviceConnection, String> {
    initialize_context(handle, 0, config.initialize_ack_timeout).await.map_err(|e| e.to_string())?;

    let servers = IceServers {
        stun_url: r.stun_urls.join(" "),
        turn_url: r.turn_urls.join(" "),
        turn_username: r.turn_username.clone(),
        turn_password: r.turn_credential.clone(),
    };
    let mut attempt = Attempt::new(device_transport::ICE_TURN, ConnectionType::IceSrtp, servers);
    attempt.first = true;
    attempt.relay_only = true;

    let exchange = Arc::new(ContextExchange { handle: handle.clone() });
    let config = config.clone();
    race(vec![attempt], exchange, None, config.attempt_timeout, |c| {
        DeviceConnection::complete(phone.clone(), c, config.clone())
    })
    .await
    .map_err(|e| e.to_string())
}

/// Runs the probe once against `phone` over the identity whose `auth`, `sessions` and `routes` carry
/// the Cross Device features.
pub async fn run(
    config: NanoConfig,
    auth: Arc<AuthService>,
    sessions: Arc<PartnerSessions>,
    routes: Arc<PlatformRoutes>,
    phone: DcgClient,
) {
    let started = Instant::now();
    let step = |name: &str, ok: bool, detail: String| {
        info!(target: "nano_probe", step = name, ok, elapsed_ms = started.elapsed().as_millis() as u64, %detail);
    };

    let wait = sessions.wait_connected(&phone, Duration::from_secs(120)).await;
    step("session", wait == WaitOutcome::Connected, format!("{wait:?}"));
    if wait != WaitOutcome::Connected {
        return;
    }

    let relay = Arc::new(CloudRelay::new(auth.rest().clone(), auth.clone() as Arc<dyn TokenSource>, phone.env));
    let servers = relay.relay_servers(false).await;
    report_cloud_relay(&servers, &step).await;

    let Some(handle) = routes.validator.context_handle(&phone).await else {
        step("context_handle", false, "no context source".into());
        return;
    };
    step("context_handle", true, handle.partner.to_string());

    let hold = if relay_only() { Duration::from_secs(50) } else { Duration::from_secs(3) };
    let device = if relay_only() {
        open_relayed_device(&handle, &config, servers, phone.clone()).await
    } else {
        let connections = DeviceConnections::new(config, relay, None);
        connections.get_or_open(&handle).await.map_err(|e| e.to_string())
    };
    let device = match device {
        Ok(d) => d,
        Err(detail) => return step("device_connection", false, detail),
    };

    step("device_connection", true, format!("{:?}", device.connection.kind));
    probe_platform_channel(&device, &step).await;

    tokio::time::sleep(hold).await;
    step("still_open", !device.is_closed(), format!("after {} s", hold.as_secs()));
    step("metrics_realtime", true, device.connection.realtime_metrics());
    step("metrics_cumulative", true, device.connection.cumulative_metrics());

    close_device(&device);
}

/// Closes the device connection and logs whether it closed at once.
fn close_device(device: &DeviceConnection) {
    device.close();
    if device.is_closed() {
        info!(target: "nano_probe", "done");
    } else {
        warn!(target: "nano_probe", "done; the connection is closing");
    }
}

/// Logs the `CloudRelay` servers and, unless the probe is relay-only, gathers candidates with them.
async fn report_cloud_relay(
    servers: &Result<super::cloud_relay::RelayServers, RelayServersError>,
    step: &impl Fn(&str, bool, String),
) {
    match servers {
        Ok(r) => {
            step(
                "cloud_relay",
                true,
                format!("stun={:?} turn={:?} expires={:?}", r.stun_urls, r.turn_urls, r.turn_expire_time),
            );

            if !relay_only() {
                gather(r, step).await;
            }
        }
        Err(e) => step("cloud_relay", false, e.to_string()),
    }
}

/// The relay-only device connection; the failure's detail when there are no servers or it does not
/// open.
async fn open_relayed_device(
    handle: &phonelink_transport::platform::ContextHandle,
    config: &NanoConfig,
    servers: Result<super::cloud_relay::RelayServers, RelayServersError>,
    phone: DcgClient,
) -> Result<Arc<DeviceConnection>, String> {
    let Ok(servers) = servers else {
        return Err("no CloudRelay servers".into());
    };
    open_relayed(handle, config, &servers, phone).await.map(|d| d.bound_to(handle))
}

/// Creates an `InternalMessaging` platform channel on the device connection and closes it.
async fn probe_platform_channel(device: &DeviceConnection, step: &impl Fn(&str, bool, String)) {
    match device
        .create_platform_channel(
            PlatformChannelType::InternalMessaging,
            scenario::INTERNAL_MESSAGING,
            HashMap::new(),
            &phonelink_core::types::TraceContext::new_root(Vec::new()),
        )
        .await
    {
        Ok(channel) => {
            step("platform_channel", true, format!("{:?}", channel.kind));
            channel.close();
        }
        Err(e) => step("platform_channel", false, e.to_string()),
    }
}
