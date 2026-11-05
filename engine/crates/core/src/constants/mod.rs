//! Protocol and client-identity constants, as data. Values the engine may choose differently live in
//! [`crate::config`] as overridable defaults; this module holds only what Phone Link fixes as a
//! constant.

use crate::types::Environment;

// ------------------------------------------------------------------------------------------------
// Hosts, hub paths and URL composition
// ------------------------------------------------------------------------------------------------

/// Base URL of the production DCG service, for REST calls and the relay hub.
pub const PROD_BASE: &str = "https://dcg.microsoft.com/";
/// Base URL of the beta DCG service.
pub const BETA_BASE: &str = "https://dcg-beta.microsoft.com/";
/// Base URL of the dogfood DCG service.
pub const DOGFOOD_BASE: &str = "https://dcg-df.microsoft.com/";
/// `GET /EnvironmentDiscovery/UserServiceEnvironment` always goes to the prod base.
pub const ENVIRONMENT_DISCOVERY_BASE: &str = PROD_BASE;
/// The `SignalR` relay hub path; full URL = `<environment base>` + `relayhub/`.
pub const RELAY_HUB_PATH: &str = "relayhub/";

/// The DCG base URL of `env`, ending in `/`.
pub fn environment_base(env: Environment) -> &'static str {
    match env {
        Environment::Prod => PROD_BASE,
        Environment::Beta => BETA_BASE,
        Environment::Dogfood => DOGFOOD_BASE,
    }
}

// ------------------------------------------------------------------------------------------------
// Client identities and profiles: the app's two identities and the Cross Device Experience Host's differences
// ------------------------------------------------------------------------------------------------

/// One DCG client identity of the PC side. Both Windows packages follow the same identity rules, so a
/// profile holds only the values its identity sends and stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    /// The client type by name (`clientType` in enrollment metadata, `applicationId` in push).
    pub client_type: &'static str,
    /// MSA client id = MSA app id.
    pub msa_client_id: &'static str,
    /// Package family name, sent as `DCG-AppId`.
    pub package_family: &'static str,
    /// Package version, sent as `DCG-AppVersion` and `metadata.clientVersion`.
    pub package_version: &'static str,
    /// Certificate friendly name; local only.
    pub friendly_name: &'static str,
}

/// Phone Link's own profile — the app's primary identity.
pub const PROFILE_PL: Profile = Profile {
    client_type: "PL",
    msa_client_id: "8CF55838-E496-42C5-829B-F8D6945288F3",
    package_family: "Microsoft.YourPhone_8wekyb3d8bbwe",
    package_version: "1.26092.119.0",
    friendly_name: "Microsoft Your Phone",
};

/// Phone Link's enrollment `metadata.capabilities` (`/DeviceAuthProxy/EnrollDevice`, `SetDeviceInfo`)
/// that the app implements; Phone Link on Windows also sends `FILE_TRANSFER_ONEDRIVE`.
pub const PL_CAPABILITIES: [&str; 2] = ["UNLINKABLE_V1", "UNLINKABLE_V2"];

/// The Cross Device Experience Host profile — the app's CDEH identity.
pub const PROFILE_WEA: Profile = Profile {
    client_type: "WEA",
    msa_client_id: "ca3b40e4-3001-4842-8f21-49c0045404f8",
    package_family: "MicrosoftWindows.CrossDevice_cw5n1h2txyewy",
    package_version: "1.26092.74.0",
    friendly_name: "CrossDevice",
};

/// Link to Windows (the Android app); only the target client of the Account-Transfer-v2 request.
pub const LTW_CLIENT_ID: &str = "8ec4ef98-53cc-4d15-a43b-dc15ea21a974";

/// The OS name the PC always reports.
pub const CLIENT_OS_NAME_WINDOWS: &str = "Windows";

/// Phone Link's App-layer values, which the app sends inside App `ValueSet`s whatever its DCG profile.
pub const APP_LAYER_APP_VERSION: &str = "1.26092.119.0";
/// The `contractVersion` the engine sends in App `ValueSet`s.
pub const APP_LAYER_CONTRACT_VERSION: f64 = 3.99;

// ------------------------------------------------------------------------------------------------
// MSA scopes and portals
// ------------------------------------------------------------------------------------------------

/// The two scope families; a client must pair a family with its `AuthorizationPortal` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ScopeFamily {
    /// RPS family (`service::msatoken.<host>::MBI_SSL`, `AuthorizationPortal: MSM`) — the default.
    Rps,
    /// First-party-portal family (`https://<host>/DCG.ReadWrite`, `AuthorizationPortal: FirstPartyAppsRepo`).
    FirstPartyPortal,
}

impl ScopeFamily {
    /// The `AuthorizationPortal` header value that goes with this scope family.
    pub fn authorization_portal(self) -> &'static str {
        match self {
            ScopeFamily::Rps => "MSM",
            ScopeFamily::FirstPartyPortal => "FirstPartyAppsRepo",
        }
    }

    /// The DCG scope of `env` (bearer of `/Auth/*`, `DeviceAuthProxy/*`, `EnvironmentDiscovery`, …).
    pub fn dcg_scope(self, env: Environment) -> &'static str {
        match (self, env) {
            (ScopeFamily::Rps, Environment::Prod) => "service::msatoken.dcg.microsoft.com::MBI_SSL",
            (ScopeFamily::Rps, Environment::Beta) => "service::msatoken.dcg-beta.microsoft.com::MBI_SSL",
            (ScopeFamily::Rps, Environment::Dogfood) => "service::msatoken.dcg-df.microsoft.com::MBI_SSL",
            (ScopeFamily::FirstPartyPortal, Environment::Prod) => "https://dcg.microsoft.com/DCG.ReadWrite",
            (ScopeFamily::FirstPartyPortal, Environment::Beta) => "https://dcg-beta.microsoft.com/DCG.ReadWrite",
            (ScopeFamily::FirstPartyPortal, Environment::Dogfood) => "https://dcg-df.microsoft.com/DCG.ReadWrite",
        }
    }

    /// The strong-auth scope of `env` (enroll bearer on the interactive repair path).
    pub fn strong_auth_scope(self, env: Environment) -> &'static str {
        match (self, env) {
            (ScopeFamily::Rps, Environment::Prod) => "service::msatoken.dcg.microsoft.com::SAPI",
            (ScopeFamily::Rps, Environment::Beta) => "service::msatoken.dcg-beta.microsoft.com::SAPI",
            (ScopeFamily::Rps, Environment::Dogfood) => "service::msatoken.dcg-df.microsoft.com::SAPI",
            (ScopeFamily::FirstPartyPortal, Environment::Prod) => "https://dcg.microsoft.com/DCG.ReadWrite.SAPI",
            (ScopeFamily::FirstPartyPortal, Environment::Beta) => "https://dcg-beta.microsoft.com/DCG.ReadWrite.SAPI",
            (ScopeFamily::FirstPartyPortal, Environment::Dogfood) => "https://dcg-df.microsoft.com/DCG.ReadWrite.SAPI",
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Request headers
// ------------------------------------------------------------------------------------------------

/// Header names of DCG REST requests and the relay hub connection, and their literal values.
pub mod http_header {
    /// Identification header carrying the PC's logical device id.
    pub const DCG_LOGICAL_DEVICE_ID: &str = "DCG-LogicalDeviceId";
    /// Identification header carrying the profile's package version.
    pub const DCG_APP_VERSION: &str = "DCG-AppVersion";
    /// Identification header carrying the profile's package family name.
    pub const DCG_APP_ID: &str = "DCG-AppId";
    /// Identification header carrying the per-process session id.
    pub const DCG_SESSION_ID: &str = "DCG-SessionId";
    /// Identification header carrying the configured release ring.
    pub const DCG_RING_NAME: &str = "DCG-RingName";
    /// Identification header carrying the reported OS name.
    pub const DCG_OS: &str = "DCG-OS";
    /// Identification header carrying the reported OS version.
    pub const DCG_OS_VERSION: &str = "DCG-OSVersion";
    /// Hub connection header naming the partner the connection is opened for.
    pub const DCG_PARTNER_ID: &str = "DCG-PartnerId";
    /// The hub region of a phone-initiated connection; the engine never sends it as a header.
    pub const DCG_HUB_REGION: &str = "DCG-HubRegion";
    /// Hub connection header carrying the heartbeat interval in seconds.
    pub const DCG_HEARTBEAT_FREQUENCY: &str = "DCG-HeartBeatFrequency";
    /// W3C trace context header, sent on REST requests and the hub connection.
    pub const TRACEPARENT: &str = "traceparent";
    /// W3C trace state header, sent on REST requests and the hub connection.
    pub const TRACESTATE: &str = "tracestate";
    /// Copy of the `tracestate` value under the DCG header name.
    pub const DCG_TRACE_STATE: &str = "DCG-TraceState";
    /// Bearer token header: the MSA token or the DCG services token.
    pub const AUTHORIZATION: &str = "Authorization";
    /// Kind of bearer in `Authorization`; sent as `MSA` with an MSA token.
    pub const AUTHORIZATION_TYPE: &str = "Authorization-Type";
    /// Kind of user identity token; `MSA` on the `/Auth/*` calls.
    pub const USER_IDENTITY_TYPE: &str = "UserIdentityType";
    /// The MSA token repeated as the user identity on the `/Auth/*` calls.
    pub const USER_IDENTITY_TOKEN: &str = "UserIdentityToken";
    /// The DCG services token, sent beside an MSA bearer.
    pub const DCG_TOKEN: &str = "Dcg-Token";
    /// The portal of the MSA scope family in use, on every DCG REST request.
    pub const AUTHORIZATION_PORTAL: &str = "AuthorizationPortal";
    /// The PC user's region on the pairing service's create call.
    pub const CLIENT_REGION_NAME: &str = "clientRegionName";
    /// Literal value of `Authorization-Type` and `UserIdentityType`.
    pub const MSA: &str = "MSA";
}

// ------------------------------------------------------------------------------------------------
// API versions
// ------------------------------------------------------------------------------------------------

/// The `api-version` query value of each DCG REST service.
pub mod api_version {
    /// `Auth/*`: nonce, identity creation, sign-in and key rotation.
    pub const AUTH: &str = "1.1.0";
    /// `DeviceAuthProxy/*` device calls: enrollment, device info, listing and unlinking.
    pub const DEVICE_AUTH_PROXY_DEVICES: &str = "1.5.0";
    /// `DeviceAuthProxy/GetAccountCert`.
    pub const DEVICE_AUTH_PROXY_TRUST: &str = "1.0.0";
    /// `Dispatcher/Wake`.
    pub const DISPATCHER_WAKE: &str = "1.5.0";
    /// `Dispatcher/Diagnostic`.
    pub const DISPATCHER_DIAGNOSTIC: &str = "1.2.0";
    /// `EnvironmentDiscovery/UserServiceEnvironment`.
    pub const ENVIRONMENT_DISCOVERY: &str = "1.0.0";
    /// `TransportConfiguration`, reached only by peerless account connections.
    pub const TRANSPORT_CONFIGURATION: &str = "1.0.0";
    /// `CloudRelay/Relay`.
    pub const CLOUD_RELAY: &str = "1.0.0";
    /// `SecretDistribution` (`AccountSecret/Get`, `AccountSecret/Prefetch`).
    pub const SECRET_DISTRIBUTION: &str = "1.0.0";
    /// `Registration` (push channel registration).
    pub const REGISTRATION: &str = "2.0.0";
    /// Spelled `Api-Version` on this service only.
    pub const DEVICE_PAIRING_PROXY: &str = "3.0.0";
}

// ------------------------------------------------------------------------------------------------
// Crypto lifetimes and rotation
// ------------------------------------------------------------------------------------------------

/// Lifetimes, clock tolerances and rotation intervals of identities, certificates and trust.
pub mod lifetime {
    use std::time::Duration;

    const DAY: u64 = 24 * 60 * 60;
    const HOUR: u64 = 60 * 60;

    /// An enrolled identity expires this long after its last access (180 d).
    pub const IDENTITY_EXPIRATION: Duration = Duration::from_secs(180 * DAY);
    /// Validity of a generated identity certificate from its issue time (365 d).
    pub const CERTIFICATE_VALIDITY: Duration = Duration::from_secs(365 * DAY);
    /// How far before the issue time a certificate's `notBefore` is set (12 h).
    pub const CERTIFICATE_CLOCK_DRIFT: Duration = Duration::from_secs(12 * HOUR);
    /// Clock skew accepted on inbound JWT lifetimes (12 h).
    pub const JWT_CLOCK_DRIFT: Duration = Duration::from_secs(12 * HOUR);
    /// Span of a signed JWT on each side of now: `nbf = now − 12 h`, `exp = now + 12 h`.
    pub const JWT_LIFETIME: Duration = Duration::from_secs(12 * HOUR);
    /// Half the certificate validity (182.5 d).
    pub const IDENTITY_KEY_ROTATION: Duration = Duration::from_secs(365 * DAY / 2);
    /// Wait before retrying a failed identity key rotation (1 d).
    pub const IDENTITY_KEY_ROTATION_RETRY: Duration = Duration::from_secs(DAY);
    /// `keyValidRemainingDays ≤ 182` triggers rotation when the server reported the value.
    pub const KEY_VALID_REMAINING_DAYS_THRESHOLD: i64 = 182;
    /// A trust relationship expires this long after its last access (180 d).
    pub const TRUST_RELATIONSHIP_EXPIRATION: Duration = Duration::from_secs(180 * DAY);
    /// The trust certificate is replaced once it expires within this window (182 d).
    pub const TRUST_ROTATE_BEFORE_EXPIRY: Duration = Duration::from_secs(182 * DAY);
    /// Minimum time between two trust key rotations (1 d).
    pub const TRUST_ROTATE_MIN_INTERVAL: Duration = Duration::from_secs(DAY);
    /// Grace window in which a partner's previous certificate is still accepted (10 min).
    pub const PARTNER_TEMPORARY_CERTIFICATE: Duration = Duration::from_secs(10 * 60);
    /// Longest lifetime a DCG services token is expected to have (24 h).
    pub const SERVICES_TOKEN_EXPECTED_LIFETIME: Duration = Duration::from_secs(24 * HOUR);
    /// A services token is refreshed this long before it expires (10 min).
    pub const SERVICES_TOKEN_LEEWAY: Duration = Duration::from_secs(10 * 60);
    /// A relationship's last-access time is refreshed at most this often (12 h).
    pub const RELATIONSHIP_LAST_ACCESS_COOLDOWN: Duration = Duration::from_secs(12 * HOUR);
    /// Upper bound on a partner key's expiration, counted from now (365 d).
    pub const PARTNER_KEY_CAP: Duration = Duration::from_secs(365 * DAY);
    /// Most trust relationships the store holds (100).
    pub const MAX_TRUST_RELATIONSHIPS: usize = 100;
}

#[cfg(test)]
mod tests;
