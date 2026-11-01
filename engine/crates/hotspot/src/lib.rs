//! Instant hotspot: the PC asks the selected phone to turn its Wi-Fi hotspot on and to hand over the
//! hotspot's SSID and passphrase, then joins that network.
//!
//! - **Phone state** rides the App layer: the `connect` reply's hotspot keys (this module's connect
//!   provider) and `phoneCommand` 3 write the phone record's [`HotspotRecord`]; the phone app's
//!   Bluetooth facts are the shared [`BluetoothRecord`](phonelink_app::BluetoothRecord).
//! - **Connect** ([`Hotspot::connect`]), the one user action, brings whatever is missing first: one
//!   wait for consent, the OEM Bluetooth permission and the phone app's Bluetooth permission (the
//!   consent notification request or the Bluetooth `/permission` request); then the phone's
//!   Bluetooth device, paired on the spot when no paired device is the phone ([`Bluetooth::pair`]);
//!   then *setup complete* and `/hotspotdevicedatasyncrequest` the first time. Then a fresh RSA-2048
//!   key pair and nonce, the `SideChannelClientRequest` authorized by a data JWT (`hotspot.request`)
//!   over the phone's RFCOMM side channel ([`wire`]) — the Mac's service records for the phone read
//!   again once when they lack the side channel — the reply's checks and its JWT
//!   (`hotspot.response`, signed by the phone), the RSA-OAEP credentials, and the join through the
//!   [`HotspotWifi`] edge; the SSID joined is kept in the record.
//! - **Joined** ([`HotspotView::joined`]): the Mac's Wi-Fi is on the network the last connect joined;
//!   **disconnect** ([`Hotspot::disconnect`]) leaves it.
//!
//! Phone Link on Windows offers the hotspot as an entry in the system Wi-Fi list, which macOS has no
//! API for; here the user starts the connect from the menu bar and the engine joins the network
//! itself. When the join fails the credentials are returned so the user can join by hand.

#![cfg_attr(feature = "test-support", warn(unreachable_pub))]

uniffi::setup_scaffolding!();

pub mod edge;
pub mod wire;

use phonelink_core::config::features::HotspotConfig;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use prost::Message;
use tracing::{debug, info, warn};

use phonelink_app::consts::ypp;
use phonelink_app::{
    AppRequest, ConnectContext, ConnectContribution, ConnectContributor, ConnectProvider, PhoneRecord, RequestOptions,
};
use phonelink_bluetooth::{Bluetooth, BluetoothFailure, PhoneBluetooth, RfcommError};
use phonelink_core::codec::json::OrderedStringMap;
use phonelink_core::codec::valueset::ValueSet;
use phonelink_core::types::DcgClient;
use phonelink_transport::auth::trust::InboundJwtError;
use phonelink_transport::auth::{AuthError, AuthService, FetchMode};

#[cfg(feature = "test-support")]
pub use edge::WifiError;
pub use edge::{HotspotWifi, NoWifi};
pub use wire::{Credentials, Unavailable};

use phonelink_features::{FeatureEnv, FeatureError};

/// The consent notification request's route and the setup-complete sync's.
pub const SETUP_NOTIFICATION_ROUTE: &str = "/hotspotsetupnotificationrequest";
pub const DEVICE_DATA_SYNC_ROUTE: &str = "/hotspotdevicedatasyncrequest";

/// Why a reply JWT was not accepted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JwtRejection {
    /// No valid trust relationship knows the signer, or its certificates reject the signature:
    /// fresh trust may help.
    #[error("{0}")]
    Untrusted(String),
    /// The JWT is malformed, has another scope, or a trusted partner other than the phone signed it.
    #[error("{0}")]
    Invalid(String),
}

/// The crypto-trust operations the side channel needs: the request's data JWT and the check of the
/// phone's reply JWT. [`AuthService`] implements it.
#[async_trait]
pub trait SideChannelAuth: Send + Sync {
    /// A data JWT for `phone` with `data` and `scope`.
    fn sign(&self, phone: &DcgClient, data: &OrderedStringMap, scope: &str) -> Result<String, AuthError>;
    /// The `Data` claim of `token` when the phone signed it with `scope`.
    fn validate(&self, phone: &DcgClient, token: &str, scope: &str) -> Result<Option<String>, JwtRejection>;
    /// Refreshes the trust relationships from the cloud.
    async fn refresh_trust(&self);
}

#[async_trait]
impl SideChannelAuth for AuthService {
    fn sign(&self, phone: &DcgClient, data: &OrderedStringMap, scope: &str) -> Result<String, AuthError> {
        self.signed_data_jwt(phone, data, scope)
    }

    fn validate(&self, phone: &DcgClient, token: &str, scope: &str) -> Result<Option<String>, JwtRejection> {
        match self.validate_inbound_jwt(token, Some(scope)) {
            Ok(jwt) if jwt.signer.iter().any(|s| s.is_same_client(phone)) => Ok(jwt.data),
            Ok(jwt) => Err(JwtRejection::Invalid(format!("signed by {}, not the phone", jwt.partner_client_id))),
            Err(e @ (InboundJwtError::Untrusted(_) | InboundJwtError::Invalid(_))) => {
                Err(JwtRejection::Untrusted(e.to_string()))
            }
            Err(e) => Err(JwtRejection::Invalid(e.to_string())),
        }
    }

    async fn refresh_trust(&self) {
        if let Err(e) = self.sync_trust(FetchMode::ForceCloud).await {
            warn!(error = %e, "trust refresh failed; the hotspot reply is checked against the stored trust");
        }
    }
}

/// Why a connect ended without joining.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HotspotFailure {
    #[error("a connect is already running")]
    Busy,
    #[error("the phone does not support instant hotspot")]
    NotSupported,
    /// The phone's Bluetooth device is not at hand: Bluetooth is off, or pairing did not finish.
    #[error("{0}")]
    Bluetooth(BluetoothFailure),
    #[error("the phone did not answer the consent request")]
    ConsentTimedOut,
    /// The authorization JWT could not be made, or the phone's reply is not one this Mac trusts; the
    /// text says which.
    #[error("the phone does not trust this Mac: {0}")]
    NotTrusted(String),
    #[error("the phone's side channel could not be reached: {0}")]
    Unreachable(String),
    /// The phone offers no side-channel service: its RFCOMM server is closed.
    #[error("the phone's side channel is not open")]
    SideChannelNotOpen,
    #[error("the phone did not answer")]
    NoAnswer,
    #[error("the phone answered with an error: {0}")]
    PhoneError(String),
    #[error("the phone cannot share its connection: {0:?}")]
    Unavailable(Unavailable),
    #[error("the phone sent no credentials")]
    NoCredentials,
    #[error("the credentials are invalid: {0}")]
    InvalidCredentials(String),
    /// The credentials arrived; the network did not show up.
    #[error("the hotspot is not visible")]
    NotVisible(Credentials),
    /// The credentials arrived; joining failed.
    #[error("joining the hotspot failed: {reason}")]
    JoinFailed { credentials: Credentials, reason: String },
    /// Leaving the hotspot's network failed.
    #[error("leaving the hotspot failed: {0}")]
    LeaveFailed(String),
    #[error("{0}")]
    Internal(String),
}

impl From<BluetoothFailure> for HotspotFailure {
    fn from(f: BluetoothFailure) -> Self {
        HotspotFailure::Bluetooth(f)
    }
}

/// What the hotspot item shows for one phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotspotView {
    /// The phone has side-channel OEM support and is hotspot-capable, and this Mac has Wi-Fi.
    pub supported: bool,
    /// A connect or disconnect is running.
    pub busy: bool,
    /// The Mac's Wi-Fi is associated with the network the last connect joined.
    pub joined: bool,
}

/// The instant hotspot feature.
pub struct Hotspot {
    env: FeatureEnv,
    config: HotspotConfig,
    auth: Arc<dyn SideChannelAuth>,
    bluetooth: Arc<Bluetooth>,
    wifi: Arc<dyn HotspotWifi>,
    busy: AtomicBool,
}

/// Clears the busy flag when an action ends.
struct BusyGuard<'a>(&'a AtomicBool);

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// The connect provider: adds nothing; reads the hotspot keys of the reply.
struct HotspotContributor;

#[async_trait]
impl ConnectContributor for HotspotContributor {
    async fn contribute(&self, _ctx: &ConnectContext<'_>) -> ConnectContribution {
        ConnectContribution::default()
    }

    async fn apply(&self, ctx: &ConnectContext<'_>, response: &ValueSet) {
        ctx.app.update_record_for(ctx.phone, |r| r.hotspot.apply_connect_reply(response));
    }
}

/// How one attempt over the side channel ended without a reply to read.
#[derive(Debug)]
enum AttemptFailure {
    /// A transport failure that another attempt may cure.
    Retry(String),
    /// The phone offers no service with the UUID; another attempt may find it.
    NoService,
    /// An outcome another attempt does not change.
    Fatal(HotspotFailure),
}

impl AttemptFailure {
    fn reason(&self) -> String {
        match self {
            AttemptFailure::Retry(reason) => reason.clone(),
            AttemptFailure::NoService => RfcommError::ServiceNotFound.to_string(),
            AttemptFailure::Fatal(f) => f.to_string(),
        }
    }
}

impl Hotspot {
    /// Registers the connect provider. Call it once, before the App layer starts. `bluetooth` is the
    /// shared phone Bluetooth device the side channel rides.
    pub fn install(
        env: &FeatureEnv,
        config: HotspotConfig,
        auth: Arc<dyn SideChannelAuth>,
        bluetooth: Arc<Bluetooth>,
        wifi: Arc<dyn HotspotWifi>,
    ) -> Result<Arc<Self>, FeatureError> {
        env.app.register_connect_contributor(ConnectProvider::Hotspot, Arc::new(HotspotContributor))?;
        Ok(Arc::new(Self { env: env.clone(), config, auth, bluetooth, wifi, busy: AtomicBool::new(false) }))
    }

    fn record(&self, phone: &DcgClient) -> Option<PhoneRecord> {
        self.env.app.phone_record().filter(|r| r.phone().is_same_client(phone))
    }

    fn supported(&self, record: &PhoneRecord) -> bool {
        record.ypp_capabilities & ypp::SIDE_CHANNEL_OEM_SUPPORT != 0
            && record.hotspot.capable
            && self.wifi.has_interface()
    }

    /// The Mac's Wi-Fi is associated with the network the last connect to `record`'s phone joined.
    fn joined(&self, record: &PhoneRecord) -> bool {
        record.hotspot.ssid.is_some() && self.wifi.current_ssid() == record.hotspot.ssid
    }

    /// What the hotspot item shows for `phone`; `None` when it is not the selected phone.
    pub fn view(&self, phone: &DcgClient) -> Option<HotspotView> {
        let record = self.record(phone)?;
        Some(HotspotView {
            supported: self.supported(&record),
            busy: self.busy.load(Ordering::SeqCst),
            joined: self.joined(&record),
        })
    }

    fn begin(&self) -> Result<BusyGuard<'_>, HotspotFailure> {
        if self.busy.swap(true, Ordering::SeqCst) {
            return Err(HotspotFailure::Busy);
        }
        Ok(BusyGuard(&self.busy))
    }

    // --------------------------------------------------------------------------------------------
    // Setup: what the connect brings first when it is missing
    // --------------------------------------------------------------------------------------------

    /// Waits up to the consent wait for consent and both Bluetooth permissions: sends the Bluetooth
    /// permission request when it alone is missing, the consent notification request otherwise, and
    /// the Bluetooth permission request once consent and the OEM permission are granted.
    async fn request_phone_permissions(&self, phone: &DcgClient) -> Result<(), HotspotFailure> {
        let mut events = self.env.app.subscribe_events();
        let current = |hotspot: &Hotspot| hotspot.record(phone).unwrap_or_default();
        let start = current(self);
        if start.hotspot.phone_permissions_granted(&start.bluetooth) {
            return Ok(());
        }

        let only_bluetooth = start.hotspot.user_consent && start.hotspot.oem_bluetooth_permission;
        if !only_bluetooth {
            self.send_consent_notification();
        }

        let deadline = tokio::time::Instant::now() + self.config.consent_wait;
        let mut bluetooth_requested = false;
        loop {
            let record = current(self);
            if record.hotspot.phone_permissions_granted(&record.bluetooth) {
                return Ok(());
            }

            if record.hotspot.user_consent && record.hotspot.oem_bluetooth_permission && !bluetooth_requested {
                bluetooth_requested = true;
                let request = self.bluetooth.request_permission(phone, wire::SCENARIO);
                match tokio::time::timeout_at(deadline, request).await {
                    Err(_) => return Err(HotspotFailure::ConsentTimedOut),
                    Ok(result) => {
                        if result?.is_some_and(|granted| !granted) {
                            return Err(BluetoothFailure::PermissionNotGranted.into());
                        }
                    }
                }
                continue;
            }

            match tokio::time::timeout_at(deadline, events.recv()).await {
                Err(_) => return Err(HotspotFailure::ConsentTimedOut),
                Ok(Ok(_)) | Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                    return Err(HotspotFailure::Internal("the App layer stopped".into()));
                }
            }
        }
    }

    /// `/hotspotsetupnotificationrequest`: the phone posts its consent notification. Sent in the
    /// background; its outcome is logged only.
    fn send_consent_notification(&self) {
        let request = AppRequest::kvp(SETUP_NOTIFICATION_ROUTE, ValueSet::new().with("isuserinitiated", true))
            .with_options(RequestOptions::user_initiated().named("SendNotificationRequest", wire::SCENARIO));

        let app = self.env.app.clone();
        tokio::spawn(async move {
            match app.request(request).await {
                Ok(r) => info!(outcome = ?r.outcome, "consent notification request finished"),
                Err(e) => warn!(error = %e, "consent notification request not sent"),
            }
        });
    }

    /// `/hotspotdevicedatasyncrequest` with `HotspotCompletedFTU: true`; its outcome is logged only.
    fn send_setup_complete(&self) {
        let request = AppRequest::kvp(DEVICE_DATA_SYNC_ROUTE, ValueSet::new().with("HotspotCompletedFTU", true))
            .with_options(RequestOptions::default().named("HotspotCompletedFTU", wire::SCENARIO));

        let app = self.env.app.clone();
        tokio::spawn(async move {
            match app.request(request).await {
                Ok(r) => info!(outcome = ?r.outcome, "setup-complete sync finished"),
                Err(e) => warn!(error = %e, "setup-complete sync not sent"),
            }
        });
    }

    // --------------------------------------------------------------------------------------------
    // Connect
    // --------------------------------------------------------------------------------------------

    /// Joins `phone`'s hotspot; returns the SSID joined. Whatever the connect needs and is missing
    /// comes first: consent and the two Bluetooth permissions, then the phone's Bluetooth device
    /// (paired now when no paired device is the phone), then *setup complete*. The permissions come
    /// before the pairing because the phone opens its side-channel server only once the OEM Bluetooth
    /// permission is granted, and the service records the Mac reads when it pairs must carry it.
    pub async fn connect(&self, phone: &DcgClient) -> Result<String, HotspotFailure> {
        info!("hotspot connect started");
        let ssid = self.try_connect(phone).await?;
        info!("hotspot joined");
        Ok(ssid)
    }

    async fn try_connect(&self, phone: &DcgClient) -> Result<String, HotspotFailure> {
        let _busy = self.begin()?;
        let record = self.record(phone).ok_or(HotspotFailure::NotSupported)?;
        if !self.supported(&record) {
            return Err(HotspotFailure::NotSupported);
        }

        // Bluetooth off ends the connect before the phone is asked for anything.
        self.bluetooth.paired_phone(phone).await?;
        self.request_phone_permissions(phone).await?;
        let address = self.bluetooth.pair(phone, wire::SCENARIO).await?;

        if !self.record(phone).is_some_and(|r| r.hotspot.setup_complete) {
            self.env.app.update_record_for(phone, |r| r.hotspot.setup_complete = true);
            info!("hotspot setup complete");
            self.send_setup_complete();
        }

        let keys = blocking(wire::HotspotKeyPair::generate)
            .await?
            .map_err(|e| HotspotFailure::Internal(format!("RSA key generation failed: {e}")))?;
        let keys = Arc::new(keys);

        let nonce = wire::new_nonce();
        let data = wire::authorization_data(&keys.public_key(), &nonce, phone);
        let jwt = self
            .auth
            .sign(phone, &data, wire::REQUEST_SCOPE)
            .map_err(|e| HotspotFailure::NotTrusted(format!("no authorization JWT: {e}")))?;
        let request = wire::credentials_request(&wire::request_trace(), jwt).encode_to_vec();

        let token = self.exchange(&address, &request, &nonce).await?;
        let data = self.validated_data(phone, &token).await?;

        let (encrypted, ssid_length) = wire::encrypted_credentials(data.as_deref()).map_err(|e| match e {
            wire::CredentialsError::Missing => HotspotFailure::NoCredentials,
            other => HotspotFailure::PhoneError(other.to_string()),
        })?;
        let credentials = blocking(move || keys.credentials(&encrypted, &ssid_length))
            .await?
            .map_err(|e| HotspotFailure::InvalidCredentials(e.to_string()))?;
        info!("hotspot credentials received");

        let ssid = self.join(credentials).await?;
        self.env.app.update_record_for(phone, |r| r.hotspot.ssid = Some(ssid.clone()));
        Ok(ssid)
    }

    /// Leaves `phone`'s hotspot while the Mac's Wi-Fi is on it; nothing is sent to the phone, and
    /// nothing is done while the Mac is not on it.
    pub async fn disconnect(&self, phone: &DcgClient) -> Result<(), HotspotFailure> {
        let _busy = self.begin()?;
        if !self.record(phone).is_some_and(|r| self.joined(&r)) {
            info!("hotspot disconnect skipped: not joined");
            return Ok(());
        }

        let wifi = self.wifi.clone();
        blocking(move || wifi.leave()).await?.map_err(|e| HotspotFailure::LeaveFailed(e.to_string()))?;
        info!("hotspot left");
        Ok(())
    }

    /// Sends `request` over the side channel and returns the reply's JWT, retrying transport failures.
    /// The first time the service is missing, the Mac's service records for the phone are read again
    /// before the next attempt: records read before the phone opened its server lack it.
    async fn exchange(&self, address: &str, request: &[u8], nonce: &str) -> Result<String, HotspotFailure> {
        let attempts = 1 + self.config.retries;
        let mut last = AttemptFailure::Retry(String::new());
        let mut refreshed = false;

        for attempt in 1..=attempts {
            if matches!(last, AttemptFailure::NoService) && !refreshed {
                refreshed = true;
                self.bluetooth.refresh_services(address, wire::SERVICE_UUID).await;
            }

            let bluetooth = self.bluetooth.edge();
            let (address, frame, nonce) = (address.to_string(), wire::encode_frame(request), nonce.to_string());
            let (lookup, reply) =
                (phonelink_core::time::duration_ms(self.config.service_lookup_timeout), self.config.reply_timeout);
            match blocking(move || attempt_once(bluetooth.as_ref(), &address, &frame, &nonce, lookup, reply)).await? {
                Ok(token) => return Ok(token),
                Err(AttemptFailure::Fatal(f)) => return Err(f),
                Err(retry) => {
                    // The last attempt's failure is the connect's, which the app logs.
                    if attempt < attempts {
                        warn!(attempt, reason = %retry.reason(), "hotspot side-channel attempt failed; retrying");
                    }
                    last = retry;
                }
            }
        }

        Err(match last {
            AttemptFailure::NoService => HotspotFailure::SideChannelNotOpen,
            other => HotspotFailure::Unreachable(other.reason()),
        })
    }

    /// The reply JWT's `Data` when the phone signed it; one trust refresh when trust may be stale.
    async fn validated_data(&self, phone: &DcgClient, token: &str) -> Result<Option<String>, HotspotFailure> {
        match self.auth.validate(phone, token, wire::RESPONSE_SCOPE) {
            Ok(data) => return Ok(data),
            Err(JwtRejection::Invalid(reason)) => {
                return Err(HotspotFailure::NotTrusted(format!("the reply JWT is not acceptable: {reason}")));
            }
            Err(JwtRejection::Untrusted(reason)) => {
                warn!(%reason, "hotspot reply JWT untrusted; refreshing trust and checking it again")
            }
        }

        self.auth.refresh_trust().await;
        self.auth
            .validate(phone, token, wire::RESPONSE_SCOPE)
            .map_err(|e| HotspotFailure::NotTrusted(format!("the reply JWT is untrusted after a trust refresh: {e}")))
    }

    /// Scans for the SSID (once more after the delay when it is not visible yet), then joins.
    async fn join(&self, credentials: Credentials) -> Result<String, HotspotFailure> {
        let mut visible = self.visible(&credentials).await?;
        if !visible {
            tokio::time::sleep(self.config.not_visible_retry_delay).await;
            visible = self.visible(&credentials).await?;
        }
        if !visible {
            return Err(HotspotFailure::NotVisible(credentials));
        }

        let wifi = self.wifi.clone();
        let (ssid, passphrase) = (credentials.ssid.clone(), credentials.passphrase.clone());
        match blocking(move || wifi.join(ssid, passphrase)).await? {
            Ok(()) => Ok(credentials.ssid),
            Err(e) => Err(HotspotFailure::JoinFailed { credentials, reason: e.to_string() }),
        }
    }

    async fn visible(&self, credentials: &Credentials) -> Result<bool, HotspotFailure> {
        let wifi = self.wifi.clone();
        let ssid = credentials.ssid.clone();
        blocking(move || wifi.is_visible(ssid))
            .await?
            .map_err(|e| HotspotFailure::JoinFailed { credentials: credentials.clone(), reason: e.to_string() })
    }
}

/// One attempt: open the channel, write the frame, read until the first `request_response` or the
/// channel closes, close the channel; then check the reply.
fn attempt_once(
    bluetooth: &dyn PhoneBluetooth,
    address: &str,
    frame: &[u8],
    nonce: &str,
    lookup_ms: u64,
    reply_timeout: Duration,
) -> Result<String, AttemptFailure> {
    let channel =
        bluetooth.rfcomm_open(address.to_string(), wire::SERVICE_UUID.to_string(), lookup_ms).map_err(|e| match e {
            RfcommError::Unexpected { message } => AttemptFailure::Fatal(HotspotFailure::Internal(message)),
            RfcommError::ServiceNotFound => AttemptFailure::NoService,
            other => AttemptFailure::Retry(other.to_string()),
        })?;

    let result = (|| {
        bluetooth.rfcomm_write(channel, frame.to_vec()).map_err(|e| AttemptFailure::Retry(e.to_string()))?;

        let deadline = Instant::now() + reply_timeout;
        let mut decoder = wire::FrameDecoder::default();
        loop {
            while let Some(body) =
                decoder.next_frame().map_err(|e| AttemptFailure::Fatal(HotspotFailure::PhoneError(e.to_string())))?
            {
                match wire::server_response(&body) {
                    Ok(Some(response)) => return Ok(response),
                    Ok(None) => debug!("side-channel message without a response skipped"),
                    Err(e) => return Err(AttemptFailure::Fatal(HotspotFailure::PhoneError(format!("reply: {e}")))),
                }
            }

            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(AttemptFailure::Fatal(HotspotFailure::NoAnswer));
            }

            match bluetooth.rfcomm_read(channel, phonelink_core::time::duration_ms(left)) {
                Ok(bytes) if bytes.is_empty() => {
                    return Err(AttemptFailure::Retry("the channel closed before a reply".into()));
                }
                Ok(bytes) => decoder.push(&bytes),
                Err(RfcommError::TimedOut) => return Err(AttemptFailure::Fatal(HotspotFailure::NoAnswer)),
                Err(e) => return Err(AttemptFailure::Retry(e.to_string())),
            }
        }
    })();

    bluetooth.rfcomm_close(channel);

    let response = result?;
    wire::check_reply(&response, nonce).map_err(|e| {
        if e.is_retryable() {
            return AttemptFailure::Retry(e.to_string());
        }
        AttemptFailure::Fatal(match e {
            wire::ReplyError::Untrusted
            | wire::ReplyError::AuthorizationMissing
            | wire::ReplyError::InvalidNonce
            | wire::ReplyError::Remote(
                phonelink_proto::side_channel::v1::SideChannelErrorType::CryptoTrustError
                | phonelink_proto::side_channel::v1::SideChannelErrorType::CryptoValidationError,
            ) => HotspotFailure::NotTrusted(e.to_string()),
            wire::ReplyError::Unavailable(reason) => HotspotFailure::Unavailable(reason),
            other => HotspotFailure::PhoneError(other.to_string()),
        })
    })
}

/// Runs `f` on the blocking pool; a panic in it is an internal failure.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, HotspotFailure> {
    tokio::task::spawn_blocking(f).await.map_err(|e| HotspotFailure::Internal(format!("an edge call failed: {e}")))
}
