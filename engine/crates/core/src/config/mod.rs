//! Engine configuration: the timers, limits and switches of every layer, the identity the engine
//! presents, and the network endpoints.
//!
//! Defaults are the production values; tests shorten timers and
//! point every endpoint at `127.0.0.1`.

use std::time::Duration;

use crate::constants::{self, Profile, ScopeFamily};
pub mod features;

use crate::config::features::FeaturesConfig;
use crate::types::Environment;

const fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}
const fn mins(m: u64) -> Duration {
    Duration::from_secs(m * 60)
}
const fn hours(h: u64) -> Duration {
    Duration::from_secs(h * 3600)
}
const fn days(d: u64) -> Duration {
    Duration::from_secs(d * 86_400)
}

/// Complete engine configuration. `Default` is one identity (`WEA`) carrying every feature; the
/// app runs [`EngineConfig::production`] — two identities.
#[derive(Debug, Clone, Default)]
pub struct EngineConfig {
    pub endpoints: Endpoints,
    pub identity: IdentityConfig,
    pub auth: AuthConfig,
    pub relay: RelayConfig,
    pub dcg: DcgConfig,
    pub platform: PlatformConfig,
    pub app: AppConfig,
    /// The Nano connection to the phone.
    pub nano: NanoConfig,
    /// The features.
    pub features: FeaturesConfig,
}

impl EngineConfig {
    /// The app's configuration: Phone Link's `PL` identity, which keeps the App layer's account, trust
    /// and device list, and the Cross Device Experience Host's `WEA` identity, enrolled beside it, whose
    /// one connection carries both CDEH's features (`/Context/Publish`, `/DeviceResourceManager`) and
    /// the App layer's messages.
    pub fn production() -> Self {
        let mut c = Self::default();
        c.identity.profile = constants::PROFILE_PL;
        c.auth.capabilities = constants::PL_CAPABILITIES.iter().map(|s| s.to_string()).collect();
        c.identity.cdeh =
            Some(CdehIdentity { profile: constants::PROFILE_WEA, capabilities: AuthConfig::default().capabilities });
        c
    }

    /// The configuration of the CDEH identity's stack: this configuration with the CDEH profile and
    /// capabilities and no further CDEH identity; `None` when this configuration has one identity.
    pub fn cdeh_config(&self) -> Option<Self> {
        let cdeh = self.identity.cdeh.as_ref()?;
        let mut c = self.clone();
        c.identity.profile = cdeh.profile;
        c.auth.capabilities = cdeh.capabilities.clone();
        c.identity.cdeh = None;
        Some(c)
    }
}

/// The Cross Device Experience Host identity enrolled beside the primary one.
#[derive(Debug, Clone)]
pub struct CdehIdentity {
    /// `WEA`.
    pub profile: Profile,
    /// Its enrollment `metadata.capabilities` before the feature switches add theirs.
    pub capabilities: Vec<String>,
}

/// Where every request goes. Production values by default; tests replace all of them.
#[derive(Debug, Clone)]
pub struct Endpoints {
    /// `<environment base>` per environment, ending in `/`.
    pub prod_base: String,
    pub beta_base: String,
    pub dogfood_base: String,
    /// Base of `GET /EnvironmentDiscovery/UserServiceEnvironment` (always prod, as with Phone Link on
    /// Windows).
    pub discovery_base: String,
    /// Test seam: when set, the relay hub is `<hub_base>relayhub/` for every environment instead of
    /// `<environment base>relayhub/`, so a test can serve the hub and the DCG REST services from two
    /// servers. Production leaves it unset.
    pub hub_base: Option<String>,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            prod_base: constants::PROD_BASE.into(),
            beta_base: constants::BETA_BASE.into(),
            dogfood_base: constants::DOGFOOD_BASE.into(),
            discovery_base: constants::ENVIRONMENT_DISCOVERY_BASE.into(),
            hub_base: None,
        }
    }
}

impl Endpoints {
    /// The DCG base URL of `env`.
    pub fn base(&self, env: Environment) -> &str {
        match env {
            Environment::Prod => &self.prod_base,
            Environment::Beta => &self.beta_base,
            Environment::Dogfood => &self.dogfood_base,
        }
    }

    /// `<environment base>relayhub/`, or `<hub_base>relayhub/` when the test seam is set.
    pub fn hub_url(&self, env: Environment) -> String {
        let base = self.hub_base.as_deref().unwrap_or_else(|| self.base(env));
        format!("{base}{}", constants::RELAY_HUB_PATH)
    }
}

/// The DCG-level identity the engine presents, with its own values where Phone Link on Windows
/// derives one from Windows.
#[derive(Debug, Clone)]
pub struct IdentityConfig {
    /// The primary identity: the App layer's account, trust and device list, and also the connection
    /// and every feature when there is no CDEH identity.
    pub profile: Profile,
    /// The CDEH identity, whose connection carries CDEH's features and the App layer's messages;
    /// `None`: the primary identity carries them.
    pub cdeh: Option<CdehIdentity>,
    /// Which scope family / `AuthorizationPortal` pair to use.
    pub scope_family: ScopeFamily,
    /// `DCG-OS` and `metadata.osName`.
    pub os_name: String,
    /// `DCG-OSVersion` and `metadata.osVersion`.
    pub os_version: String,
    /// `DCG-RingName`, also sent as the App-layer `attributes.flightRing`.
    pub ring_name: String,
    /// `metadata.displayName` / App `displayName`; `None` = the host name cut before its first `.`.
    pub display_name: Option<String>,
}

impl Default for IdentityConfig {
    fn default() -> Self {
        Self {
            profile: constants::PROFILE_WEA,
            cdeh: None,
            scope_family: ScopeFamily::Rps,
            os_name: constants::CLIENT_OS_NAME_WINDOWS.into(),
            os_version: "10.0.26100".into(),
            ring_name: "Public".into(),
            display_name: None,
        }
    }
}

/// Identity, enrollment, trust and wake timers and constants.
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// Enrollment `metadata.capabilities` of `/DeviceAuthProxy/EnrollDevice` and `SetDeviceInfo`:
    /// `DEVICE_STATUS` and `UNLINKABLE_V1` always; feature switches add theirs.
    pub capabilities: Vec<String>,
    pub identity_expiration: Duration,
    pub token_refresh_leeway: Duration,
    pub identity_key_rotation: Duration,
    pub identity_key_rotation_retry: Duration,
    pub key_valid_remaining_days_threshold: i64,
    pub auth_recover_retries: u32,
    pub client_info_refresh: Duration,
    pub client_info_attempt_interval: Duration,
    pub peer_refresh: Duration,
    pub forced_peer_refresh_per_hour: u32,
    pub relationship_expiration: Duration,
    pub relationship_last_access_cooldown: Duration,
    pub trust_rotate_before_expiry: Duration,
    pub partner_temporary_certificate: Duration,
    pub wake: WakeConfig,
    /// The resilience policy of every DCG REST call but the pairing service's.
    pub rest: DcgRestConfig,
    pub pairing: PairingConfig,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            capabilities: vec!["DEVICE_STATUS".into(), "UNLINKABLE_V1".into()],
            identity_expiration: days(180),
            token_refresh_leeway: mins(10),
            identity_key_rotation: Duration::from_secs(365 * 86_400 / 2),
            identity_key_rotation_retry: days(1),
            key_valid_remaining_days_threshold: 182,
            auth_recover_retries: 3,
            client_info_refresh: days(7),
            client_info_attempt_interval: days(1),
            peer_refresh: days(1),
            forced_peer_refresh_per_hour: 5,
            relationship_expiration: days(180),
            relationship_last_access_cooldown: hours(12),
            trust_rotate_before_expiry: days(182),
            partner_temporary_certificate: mins(10),
            wake: WakeConfig::default(),
            rest: DcgRestConfig::default(),
            pairing: PairingConfig::default(),
        }
    }
}

/// The resilience policy every DCG REST call goes through, outermost layer first.
///
/// A concurrency limit, the total timeout, retries with back-off, a circuit breaker, the attempt
/// timeout, and inside each attempt the resends of a 503, 504 or 429 reply.
///
/// The limit and the breaker are kept
/// per service client (`auth::rest_policy::ServiceClient`).
#[derive(Debug, Clone)]
pub struct DcgRestConfig {
    /// Calls of one service client in flight at once; one more is refused.
    pub concurrency_limit: usize,
    /// Bound on one call: every attempt and every wait between them.
    pub total_timeout: Duration,
    /// Retries after a transient attempt (so `retries + 1` attempts).
    pub retries: u32,
    /// Base delay of the jittered exponential back-off between attempts.
    pub retry_base_delay: Duration,
    /// Share of failed attempts among the sampled ones that opens the circuit.
    pub breaker_failure_ratio: f64,
    /// Sampled attempts needed before the circuit can open.
    pub breaker_minimum_throughput: u32,
    pub breaker_sampling: Duration,
    pub breaker_break: Duration,
    /// Bound on one attempt, its resends included; `None` for no bound (the pairing service).
    pub attempt_timeout: Option<Duration>,
    /// Resends of a 503, 504 or 429 reply inside one attempt.
    pub resends: u32,
    /// The resend wait without `Retry-After`: `resend_delay · 2^k` before resend `k`.
    pub resend_delay: Duration,
    pub resend_max_delay: Duration,
}

impl Default for DcgRestConfig {
    fn default() -> Self {
        Self {
            concurrency_limit: 1000,
            total_timeout: secs(30),
            retries: 3,
            retry_base_delay: secs(2),
            breaker_failure_ratio: 0.1,
            breaker_minimum_throughput: 100,
            breaker_sampling: secs(30),
            breaker_break: secs(5),
            attempt_timeout: Some(secs(10)),
            resends: 3,
            resend_delay: secs(3),
            resend_max_delay: secs(180),
        }
    }
}

impl DcgRestConfig {
    /// The policy of the `DevicePairingProxy` calls: the same layers without the attempt timeout, and
    /// the whole call bounded by 100 s.
    pub fn pairing() -> Self {
        Self { total_timeout: secs(100), attempt_timeout: None, ..Self::default() }
    }
}

/// QR/PIN account pairing: the bounds and attempts of its steps and its own REST policy.
#[derive(Debug, Clone)]
pub struct PairingConfig {
    /// Bound on each create attempt; its expiry ends the session.
    pub create_timeout: Duration,
    /// Attempts of the create call on a recoverable failure.
    pub create_attempts: u32,
    /// Bound on each attempt of a poll step (the request and the wait after it).
    pub poll_step_timeout: Duration,
    /// Attempts of a poll step on a recoverable failure; counted again after every state change.
    pub poll_step_attempts: u32,
    /// The wait after a response that keeps the phase.
    pub poll_interval: Duration,
    /// Bound on a challenge-PIN refresh.
    pub refresh_timeout: Duration,
    pub rest: DcgRestConfig,
}

impl Default for PairingConfig {
    fn default() -> Self {
        Self {
            create_timeout: secs(30),
            create_attempts: 3,
            poll_step_timeout: secs(60),
            poll_step_attempts: 2,
            poll_interval: secs(2),
            refresh_timeout: secs(30),
            rest: DcgRestConfig::pairing(),
        }
    }
}

/// `/Dispatcher/Wake` and the wake sequence around it.
#[derive(Debug, Clone)]
pub struct WakeConfig {
    pub ttl_default: Duration,
    pub ttl_long: Duration,
    pub ttl_cancel: Duration,
    pub on_connected_wait: Duration,
    pub minimum_partner_wait: Duration,
    pub retries: u32,
    pub breaker_sampling_window: Duration,
    pub breaker_minimum_throughput: u32,
    pub breaker_break: Duration,
}

impl Default for WakeConfig {
    fn default() -> Self {
        Self {
            ttl_default: secs(20),
            ttl_long: mins(45),
            ttl_cancel: secs(1),
            on_connected_wait: secs(10),
            minimum_partner_wait: secs(10),
            retries: 2,
            breaker_sampling_window: mins(5),
            breaker_minimum_throughput: 15,
            breaker_break: mins(1),
        }
    }
}

/// Timers and constants of the `SignalR` relay connection.
#[derive(Debug, Clone)]
pub struct RelayConfig {
    pub keep_alive: Duration,
    pub server_timeout: Duration,
    /// `DCG-HeartBeatFrequency`; zero disables heartbeat expectations.
    pub heartbeat_frequency: Duration,
    /// Window = frequency + this.
    pub heartbeat_grace: Duration,
    /// The engine's own limit on the heartbeat window: remove after this many consecutive missed windows.
    pub missed_heartbeats_threshold: u32,
    pub start_retry_attempts: u32,
    pub start_retry_sleep: Duration,
    pub too_many_requests_base: Duration,
    pub too_many_requests_variance: Duration,
    pub reconnect_delays: Vec<Duration>,
    pub on_connected_wait: Duration,
    pub rekey_close_wait: Duration,
    pub partner_retry_base: Duration,
    pub partner_retry_fatal: Duration,
    /// Reconnects to a partner on its own after the hub announces the partner's disconnect.
    pub automatic_reconnects_enabled: bool,
    /// Suppresses the session end on an inbound disconnect and forces `DCG-RequestNewSession: "True"`
    /// on every wake.
    pub keep_session_on_peer_disconnect: bool,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            keep_alive: secs(15),
            server_timeout: secs(30),
            heartbeat_frequency: secs(15),
            heartbeat_grace: secs(2),
            missed_heartbeats_threshold: 3,
            start_retry_attempts: 3,
            start_retry_sleep: secs(1),
            too_many_requests_base: secs(15),
            too_many_requests_variance: secs(30),
            reconnect_delays: vec![secs(0), secs(2), secs(10), secs(30), mins(1), mins(3), mins(5)],
            on_connected_wait: secs(10),
            rekey_close_wait: secs(20),
            partner_retry_base: secs(1),
            partner_retry_fatal: hours(6),
            automatic_reconnects_enabled: false,
            keep_session_on_peer_disconnect: false,
        }
    }
}

/// Constants of the `ms-dcg` fragment layer.
#[derive(Debug, Clone)]
pub struct DcgConfig {
    pub fragment_size: usize,
    pub ack_timeout: Duration,
    pub ack_retry_count: u32,
    /// Repeats of a fragment try (open, partner check or wake, hub send) after a hub `Failure` or
    /// `NotConnected`. An ack is sent once, as Phone Link on Windows does.
    pub hub_send_retries: u32,
    /// Consecutive ack timeouts that remove the partner (`TooManyAckTimeouts`; differs from Phone Link
    /// on Windows); 0 disables.
    pub partner_disconnect_ack_timeouts: u32,
}

impl Default for DcgConfig {
    fn default() -> Self {
        Self {
            fragment_size: 536_870_912,
            ack_timeout: secs(15),
            ack_retry_count: 2,
            hub_send_retries: 2,
            partner_disconnect_ack_timeouts: 5,
        }
    }
}

/// Platform routes and the per-partner connection: preconditions, state and timers.
#[derive(Debug, Clone)]
pub struct PlatformConfig {
    /// Send `/SessionValidation` on every connect.
    pub session_validation_enabled: bool,
    /// Adds `NANO_TRANSPORT_PREFERENCE`; off.
    pub enable_nano_transport_preference: bool,
    pub request_timeout: Duration,
    pub connection_timeout: Duration,
    pub session_maximum_wait: Duration,
    pub send_disconnect_request_timeout: Duration,
    /// Phone `clientVersion` floor of CDEH's `/DeviceResourceManager` requests.
    pub drm_minimum_client_version: String,
}

impl Default for PlatformConfig {
    fn default() -> Self {
        Self {
            session_validation_enabled: true,
            enable_nano_transport_preference: false,
            request_timeout: secs(120),
            connection_timeout: secs(70),
            session_maximum_wait: secs(120),
            send_disconnect_request_timeout: secs(30),
            drm_minimum_client_version: "1.23112.189.0".into(),
        }
    }
}

/// The App message layer: request sending, timeouts and retries.
#[derive(Debug, Clone)]
pub struct AppConfig {
    /// Phone Link's package version, sent as `appVersion` inside App `ValueSet`s.
    pub app_version: String,
    pub contract_version: f64,
    pub request_timeout: Duration,
    pub background_connect_timeout: Duration,
    /// At most one connect per this interval when the platform connection becomes `Connected`.
    pub connect_on_connected_cooldown: Duration,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            app_version: constants::APP_LAYER_APP_VERSION.into(),
            contract_version: constants::APP_LAYER_CONTRACT_VERSION,
            request_timeout: secs(120),
            background_connect_timeout: secs(600),
            connect_on_connected_cooldown: secs(60),
        }
    }
}

/// The Nano connection: timers, and the inputs that pick which attempts the screen-mirroring path
/// races. The defaults race `Ip` and host-candidate `Ice` only.
#[derive(Debug, Clone)]
pub struct NanoConfig {
    /// Each attempt is cancelled after this.
    pub attempt_timeout: Duration,
    /// A device connection must complete (its message channel open) within this, counted from the
    /// phone's `cs_initialize_ack_message`.
    pub device_connection_timeout: Duration,
    /// The phone's `cs_initialize_ack_message` must arrive within this.
    pub initialize_ack_timeout: Duration,
    /// A platform channel's creation result, and the opening of its Nano channels, within this each.
    pub platform_channel_creation_timeout: Duration,
    /// Closing a `NamedStreams` channel waits this long per child.
    pub named_streams_close_timeout: Duration,
    /// The screen-mirroring path closes a connection that has missed packets this long.
    pub mirroring_unrecovered_close: Duration,
    /// Screen mirroring opens its connection over the device-connection path (when the phone also
    /// advertises it) instead of `/nanohandshake`.
    pub mirroring_over_device_connection: bool,
    /// The `IceStun` attempt of the screen-mirroring path takes its STUN server from a `CloudRelay`
    /// STUN-only fetch; otherwise from [`Self::mirroring_stun_servers`].
    pub mirroring_stun_from_cloud_relay: bool,
    /// Comma-separated STUN servers; the first is the screen-mirroring path's.
    pub mirroring_stun_servers: String,
    /// The screen-mirroring path also races `IceStunAndTurn` (with a `CloudRelay` STUN+TURN fetch) and
    /// ignores [`Self::mirroring_attempts`].
    pub mirroring_turn: bool,
    /// Which screen-mirroring attempts run: bits `Ip 1`, `Ice 2`, `IceStun 4`; 0 or no known bit = all.
    pub mirroring_attempts: u32,
    /// Comma-separated STUN hosts for the phone's NAT test; sent with the first attempt when there are
    /// at least two.
    pub mirroring_diagnostics_servers: String,
}

impl Default for NanoConfig {
    fn default() -> Self {
        Self {
            attempt_timeout: secs(90),
            device_connection_timeout: secs(60),
            initialize_ack_timeout: secs(30),
            platform_channel_creation_timeout: secs(30),
            named_streams_close_timeout: secs(10),
            mirroring_unrecovered_close: secs(45),
            mirroring_over_device_connection: false,
            mirroring_stun_from_cloud_relay: false,
            mirroring_stun_servers: String::new(),
            mirroring_turn: false,
            mirroring_attempts: 0,
            mirroring_diagnostics_servers: String::new(),
        }
    }
}

/// `metadata.displayName` rule (differs from Phone Link on Windows): the host name cut before its
/// first `.`.
pub fn display_name_from_host(host: &str) -> String {
    host.split('.').next().unwrap_or(host).to_string()
}

#[cfg(test)]
mod tests;
