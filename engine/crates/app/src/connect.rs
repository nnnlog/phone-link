//! The connect message (`/connect/v1`) and its sequence.
//!
//! The request is a KVP request whose `ValueSet` the shared layer builds in a fixed provider order:
//! the containers first, then the providers — the non-feature providers and the off-states of the
//! features the app does not implement here, the six implemented features' providers through
//! [`ConnectContributor`]s — then the standard keys. The response is applied `DeviceInfo` first, then
//! the other non-feature keys, then the contributors in provider order.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::runtime::Handle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

use phonelink_core::codec::valueset::{Value, ValueSet};
use phonelink_core::types::{DcgClient, TraceContext};

use super::consts::{key, literal, priority, route};
use super::phone_record::PhoneRecord;
use super::request::{AppRequest, AppResponse, RequestKind, RequestOptions};
use super::{AppError, AppEvent, AppLayer};
use phonelink_core::sync::MutexExt;

/// The two request shapes of a connect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectKind {
    /// Not user-initiated, priority `100`, no new session, 600 s; dropped while another background
    /// connect is in flight.
    Background,
    /// User-initiated, priority `0`, `NewSession` + `IgnoreDeviceDisabled`, 120 s; always runs.
    Immediate,
}

/// The connect triggers the engine maps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectTrigger {
    /// Engine start with a linked phone — background.
    EngineStart,
    /// The user selected another phone: to the new phone — background.
    SelectedPhoneChanged,
    /// The platform connection to the phone became `Connected` — background, at most once per
    /// cooldown (60 s).
    BecameConnected,
    /// The app came to the foreground — immediate.
    AppActivated,
    /// macOS unlock or wake — background.
    UnlockOrWake,
    /// A user refresh or retry action — immediate.
    Refresh,
    /// A write changed the recorded Wi-Fi state or sync-over-mobile-data setting and afterwards
    /// either is on — background.
    StatusChanged,
    /// macOS lock or sleep — the disconnect trigger: cancels an in-flight connect.
    LockOrSleep,
}

impl ConnectTrigger {
    /// `ypTriggerId` of the connect's trace state (free-form).
    pub fn name(self) -> &'static str {
        match self {
            ConnectTrigger::EngineStart => "EngineStart",
            ConnectTrigger::SelectedPhoneChanged => "ActiveDeviceChanged",
            ConnectTrigger::BecameConnected => "PlatformConnected",
            ConnectTrigger::AppActivated => "AppActivated",
            ConnectTrigger::UnlockOrWake => "UnlockOrWake",
            ConnectTrigger::Refresh => "Refresh",
            ConnectTrigger::StatusChanged => "DeviceStatusChanged",
            ConnectTrigger::LockOrSleep => "LockOrSleep",
        }
    }

    /// The connect kind the trigger sends; `None` for the disconnect trigger.
    pub fn kind(self) -> Option<ConnectKind> {
        match self {
            ConnectTrigger::EngineStart
            | ConnectTrigger::SelectedPhoneChanged
            | ConnectTrigger::BecameConnected
            | ConnectTrigger::UnlockOrWake
            | ConnectTrigger::StatusChanged => Some(ConnectKind::Background),
            ConnectTrigger::AppActivated | ConnectTrigger::Refresh => Some(ConnectKind::Immediate),
            ConnectTrigger::LockOrSleep => None,
        }
    }
}

/// The implemented features' providers' slots in the provider order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ConnectProvider {
    /// Reads the Bluetooth keys of the reply; adds nothing.
    Bluetooth,
    /// Reads the hotspot keys of the reply; adds nothing.
    Hotspot,
    SyncDeviceStatus,
    Wallpaper,
    Photos,
    Messaging,
    /// Android only.
    SyncNotifications,
    PhoneApps,
    /// Call history, Android only.
    Calling,
    Contacts,
}

impl ConnectProvider {
    /// The provider's position in the provider order.
    pub fn number(self) -> u8 {
        match self {
            ConnectProvider::Bluetooth => 1,
            ConnectProvider::Hotspot => 2,
            ConnectProvider::SyncDeviceStatus => 3,
            ConnectProvider::Wallpaper => 4,
            ConnectProvider::Photos => 9,
            ConnectProvider::Messaging => 10,
            ConnectProvider::SyncNotifications => 11,
            ConnectProvider::PhoneApps => 12,
            ConnectProvider::Calling => 13,
            ConnectProvider::Contacts => 14,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ConnectProvider::Bluetooth => "Bluetooth",
            ConnectProvider::Hotspot => "Hotspot",
            ConnectProvider::SyncDeviceStatus => "SyncDeviceStatus",
            ConnectProvider::Wallpaper => "Wallpaper",
            ConnectProvider::Photos => "Photos",
            ConnectProvider::Messaging => "Messaging",
            ConnectProvider::SyncNotifications => "SyncNotifications",
            ConnectProvider::PhoneApps => "PhoneApps",
            ConnectProvider::Calling => "Calling",
            ConnectProvider::Contacts => "Contacts",
        }
    }
}

/// What a provider sees while the connect is built and while its response is applied.
pub struct ConnectContext<'a> {
    /// The App layer, for state the provider updates and requests of its own
    /// ([`AppLayer::self_arc`] to spawn them; a contributor should not hold an `Arc<AppLayer>`
    /// itself, which would keep the layer alive through its own registry).
    pub app: &'a AppLayer,
    pub phone: &'a DcgClient,
    pub kind: ConnectKind,
    /// The phone record: before the connect when building; after `DeviceInfo` and the other
    /// non-feature keys of the response when applying.
    pub record: &'a PhoneRecord,
    /// The connect's `correlationVector` (the trigger's correlation id).
    pub correlation_vector: &'a str,
    /// The connect's trigger name (its `ypTriggerId`), for requests a provider starts on the
    /// connect's behalf (such as the trace context of the notifications full sync it starts).
    pub trigger: &'a str,
}

/// One feature provider's request keys.
///
/// `sequence_numbers` values must be `Int64`; no key may be
/// written by two providers, and `top_level` must not use the shared layer's keys (a violation is a
/// programming error that fails the connect).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConnectContribution {
    pub configuration: ValueSet,
    pub sequence_numbers: ValueSet,
    pub top_level: ValueSet,
}

/// An implemented feature's connect provider.
#[async_trait]
pub trait ConnectContributor: Send + Sync {
    /// The provider's request keys. May start requests of its own (not ordered against the connect).
    async fn contribute(&self, ctx: &ConnectContext<'_>) -> ConnectContribution;
    /// Reads the provider's keys from the **whole** response `ValueSet` (feature keys, including
    /// the expected sequence numbers, are top-level keys). Called only when a response arrived,
    /// whatever its `result`.
    async fn apply(&self, ctx: &ConnectContext<'_>, response: &ValueSet);
}

/// What [`AppLayer::send_connect`] did.
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectSendResult {
    /// Sent; the response (or failure) is in the [`AppResponse`].
    Sent(AppResponse),
    /// A background connect was already in flight.
    Dropped,
    /// No phone is selected.
    NoPhone,
    /// The request could not be built (a contributor broke the key rules).
    Failed(AppError),
}

/// Shared-layer keys a provider must not write at the top level.
const RESERVED_TOP_LEVEL: [&str; 14] = [
    key::CORRELATION_VECTOR,
    key::CONTENT_TYPE,
    key::SEQUENCE_NUMBERS,
    key::CONFIGURATION,
    key::ATTRIBUTES,
    key::DISPLAY_NAME,
    key::APP_VERSION,
    key::INSTALLATION_ID,
    key::CONTRACT_VERSION,
    key::IS_USER_INITIATED,
    key::IGNORE_RESPONSE,
    key::REQUEST_ID,
    key::ROUTE,
    key::RESULT,
];

/// The request under construction: one map per container plus the providers' top-level keys.
#[derive(Default)]
struct Draft {
    sequence_numbers: ValueSet,
    configuration: ValueSet,
    attributes: ValueSet,
    top_level: ValueSet,
}

fn put(container: &mut ValueSet, container_name: &str, provider: &str, k: &str, v: Value) -> Result<(), AppError> {
    if container.contains_key(k) {
        return Err(AppError::Programming(format!(
            "connect provider {provider} writes {container_name}.{k}, which another provider wrote"
        )));
    }
    container.insert(k, v);
    Ok(())
}

impl Draft {
    fn configuration(&mut self, provider: &str, k: &str, v: impl Into<Value>) -> Result<(), AppError> {
        put(&mut self.configuration, key::CONFIGURATION, provider, k, v.into())
    }

    fn sequence_number(&mut self, provider: &str, k: &str, v: i64) -> Result<(), AppError> {
        put(&mut self.sequence_numbers, key::SEQUENCE_NUMBERS, provider, k, Value::Int64(v))
    }

    fn attribute(&mut self, provider: &str, k: &str, v: impl Into<Value>) -> Result<(), AppError> {
        put(&mut self.attributes, key::ATTRIBUTES, provider, k, v.into())
    }

    fn merge(&mut self, provider: &str, c: ConnectContribution) -> Result<(), AppError> {
        for (k, v) in c.sequence_numbers.iter() {
            if !matches!(v, Value::Int64(_)) {
                return Err(AppError::Programming(format!(
                    "connect provider {provider} writes sequenceNumbers.{k} as {}, not Int64",
                    v.type_name()
                )));
            }
            put(&mut self.sequence_numbers, key::SEQUENCE_NUMBERS, provider, k, v.clone())?;
        }

        for (k, v) in c.configuration.iter() {
            put(&mut self.configuration, key::CONFIGURATION, provider, k, v.clone())?;
        }

        for (k, v) in c.top_level.iter() {
            if RESERVED_TOP_LEVEL.contains(&k) {
                return Err(AppError::Programming(format!(
                    "connect provider {provider} writes the shared-layer key {k}"
                )));
            }
            put(&mut self.top_level, "top level", provider, k, v.clone())?;
        }

        Ok(())
    }

    fn into_value_set(self, correlation_vector: &str) -> ValueSet {
        let mut vs = ValueSet::new()
            .with(key::CORRELATION_VECTOR, correlation_vector)
            .with(key::CONTENT_TYPE, literal::CONNECT)
            .with(key::SEQUENCE_NUMBERS, self.sequence_numbers)
            .with(key::CONFIGURATION, self.configuration)
            .with(key::ATTRIBUTES, self.attributes);

        for (k, v) in self.top_level.iter() {
            vs.insert(k, v.clone());
        }

        vs
    }
}

/// Connect bookkeeping: the background in-flight flag, the disconnect-trigger token and the
/// became-`Connected` cooldown.
pub(crate) struct ConnectState {
    background_in_flight: AtomicBool,
    cancel: Mutex<CancellationToken>,
    last_connected_trigger: Mutex<Option<Instant>>,
}

impl ConnectState {
    pub(crate) fn new() -> Self {
        Self {
            background_in_flight: AtomicBool::new(false),
            cancel: Mutex::new(CancellationToken::new()),
            last_connected_trigger: Mutex::new(None),
        }
    }
}

/// Clears the background in-flight flag when the connect ends.
struct BackgroundGuard<'a>(&'a AtomicBool);

impl Drop for BackgroundGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl AppLayer {
    /// Builds the connect `ValueSet` (without the standard keys, which the KVP request path adds).
    pub(crate) async fn build_connect(&self, ctx: &ConnectContext<'_>) -> Result<ValueSet, AppError> {
        let contributors = self.contributors_snapshot();
        let contributor = |p: ConnectProvider| contributors.iter().find(|(q, _)| *q == p).map(|(_, c)| c.clone());
        let mut d = Draft::default();

        // Bluetooth, Hotspot: nothing.
        // SyncDeviceStatus.
        if let Some(c) = contributor(ConnectProvider::SyncDeviceStatus) {
            d.merge(ConnectProvider::SyncDeviceStatus.name(), c.contribute(ctx).await)?;
        }

        // Wallpaper; without its feature, off: `false`, no sequence number.
        match contributor(ConnectProvider::Wallpaper) {
            Some(c) => d.merge(ConnectProvider::Wallpaper.name(), c.contribute(ctx).await)?,
            None => d.configuration("Wallpaper", "Wallpaper", false)?,
        }

        // RemoteControl — always `true`, as Phone Link on Windows sends.
        d.configuration("RemoteControl", "RemoteControlPhoneToPc", true)?;

        // DeviceInfo.
        d.configuration("DeviceInfo", "SequencedContentSyncs", true)?;
        d.configuration("DeviceInfo", "SyncSkipMetadataForAuto", false)?;
        d.configuration("DeviceInfo", "AndroidCanaryOptInNotifications", false)?;
        d.configuration("DeviceInfo", "yppPresenceFlowConfig", false)?;
        d.attribute("DeviceInfo", "flightRing", self.config.identity.ring_name.clone())?;
        d.attribute("DeviceInfo", "appVersion", self.standard_keys.app_version.clone())?;
        d.attribute("DeviceInfo", "displayName", self.standard_keys.display_name.clone())?;

        // Unlinkable.
        d.configuration("Unlinkable", "UnlinkableVersion", 2i32)?;

        // AccountId: nothing.
        // Photos, Messaging, SyncNotifications.
        for p in [ConnectProvider::Photos, ConnectProvider::Messaging, ConnectProvider::SyncNotifications] {
            if let Some(c) = contributor(p) {
                d.merge(p.name(), c.contribute(ctx).await)?;
            }
        }

        // PhoneApps. The engine always installs the phone apps feature; only an app layer assembled without
        // it (its own tests) takes the off-state below: no sequence numbers, `Exp006` kept.
        match contributor(ConnectProvider::PhoneApps) {
            Some(c) => d.merge(ConnectProvider::PhoneApps.name(), c.contribute(ctx).await)?,
            None => {
                d.configuration("PhoneApps", "RecentApps", false)?;
                d.configuration("PhoneApps", "AppHandoff", false)?;
                d.configuration("PhoneApps", "Exp006", true)?;
            }
        }

        // Calling; without its feature, calls turned off on the PC: `false`, no sequence number.
        match contributor(ConnectProvider::Calling) {
            Some(c) => d.merge(ConnectProvider::Calling.name(), c.contribute(ctx).await)?,
            None => d.configuration("Calling", "CallLogs", false)?,
        }

        // Contacts.
        if let Some(c) = contributor(ConnectProvider::Contacts) {
            d.merge(ConnectProvider::Contacts.name(), c.contribute(ctx).await)?;
        }

        // SharedContent — file transfer retry off.
        d.configuration("SharedContent", "fileTransferToPcRetryEnabled", false)?;

        // GlobalPermissions: nothing. TelemetryInfo: omitted (Phone Link on Windows sends it).
        // Ypp: nothing.
        // BluetoothWake — the user never enabled Bluetooth wake (the port does not wake the phone over
        // Bluetooth).
        d.configuration("BluetoothWake", "btWakeUserEnabled", false)?;

        // SyncTrustMigration: nothing.
        // Continuity — off, nothing stored.
        d.sequence_number("Continuity", "browserHistory", -1)?;
        d.sequence_number("Continuity", "continuityAppContext", -1)?;
        d.configuration("Continuity", "BrowserHistory", false)?;
        d.configuration("Continuity", "TaskContinuityVersion", 1u32)?;
        d.configuration("Continuity", "TaskContinuity", false)?;

        Ok(d.into_value_set(ctx.correlation_vector))
    }

    /// Sends one connect: built, sent once, no retry; the response applied when one arrived. A
    /// background connect is dropped while another is in flight; a disconnect trigger
    /// ([`ConnectTrigger::LockOrSleep`]) cancels an in-flight one.
    pub async fn send_connect(&self, kind: ConnectKind, trigger_name: &str) -> ConnectSendResult {
        let Some(phone) = self.selected_phone() else {
            return ConnectSendResult::NoPhone;
        };

        // Held until the connect ends — its request and, when a response arrived, the response's
        // application.
        let in_flight = match self.claim_connect_slot(kind) {
            Ok(guard) => guard,
            Err(dropped) => return dropped,
        };

        let cancel = self.connect.cancel.lock_or_recover().child_token();
        let Some(record) = self.phone_record() else {
            return ConnectSendResult::NoPhone;
        };

        // The trigger's context: its correlation id is the `correlationVector`; the request is sent
        // with a child context whose id is fresh.
        let trigger_ctx = TraceContext::new_root(Vec::new());
        let correlation_vector = trigger_ctx.correlation_id.clone();
        let ctx = ConnectContext {
            app: self,
            phone: &phone,
            kind,
            record: &record,
            correlation_vector: &correlation_vector,
            trigger: trigger_name,
        };

        let response = match self.request_connect(&ctx, &trigger_ctx, cancel).await {
            Ok(r) => r,
            Err(failed) => return failed,
        };

        if let Some(message) = &response.message {
            self.apply_connect_response(&phone, kind, &correlation_vector, trigger_name, &message.values).await;
        }

        // The connect has ended: the background slot is free before anyone hears of it, so a
        // background connect triggered from here on is sent, not dropped.
        drop(in_flight);
        self.emit(AppEvent::ConnectCompleted { phone, kind, outcome: response.outcome.clone() });
        ConnectSendResult::Sent(response)
    }

    /// Builds the connect request and sends it with a child of `trigger_ctx`; `Err(Failed)` when it
    /// could not be built or the request layer refused it.
    async fn request_connect(
        &self,
        ctx: &ConnectContext<'_>,
        trigger_ctx: &TraceContext,
        cancel: CancellationToken,
    ) -> Result<AppResponse, ConnectSendResult> {
        let payload = self.build_connect(ctx).await.map_err(ConnectSendResult::Failed)?;
        let request = AppRequest {
            route: route::CONNECT.to_string(),
            kind: RequestKind::Kvp,
            payload,
            options: self.connect_options(ctx.kind, ctx.trigger),
            trace: Some(trigger_ctx.child()),
        };
        self.send_request(request, cancel).await.map_err(ConnectSendResult::Failed)
    }

    /// Claims the background slot for a background connect (`Err(Dropped)` while one is in flight);
    /// an immediate connect claims nothing.
    fn claim_connect_slot(&self, kind: ConnectKind) -> Result<Option<BackgroundGuard<'_>>, ConnectSendResult> {
        match kind {
            ConnectKind::Background => {
                if self.connect.background_in_flight.swap(true, Ordering::SeqCst) {
                    return Err(ConnectSendResult::Dropped);
                }
                Ok(Some(BackgroundGuard(&self.connect.background_in_flight)))
            }
            ConnectKind::Immediate => Ok(None),
        }
    }

    /// The request options of a connect of `kind`, named after its trigger.
    fn connect_options(&self, kind: ConnectKind, trigger_name: &str) -> RequestOptions {
        match kind {
            ConnectKind::Background => RequestOptions {
                timeout: Some(self.config.app.background_connect_timeout),
                priority: priority::LOW,
                ..RequestOptions::default()
            },
            ConnectKind::Immediate => {
                RequestOptions { timeout: Some(self.config.app.request_timeout), ..RequestOptions::user_initiated() }
            }
        }
        .named(trigger_name, "EstablishSyncSession")
    }

    /// Applies a connect response: the phone record's keys first, then each contributor in provider order.
    async fn apply_connect_response(
        &self,
        phone: &DcgClient,
        kind: ConnectKind,
        correlation_vector: &str,
        trigger: &str,
        values: &ValueSet,
    ) {
        let app_config = self.config.app.clone();
        let Some(record) = self.update_record_for(phone, |r| {
            r.apply_connect_response(values, &app_config);
            r.clone()
        }) else {
            return;
        };

        let ctx = ConnectContext { app: self, phone, kind, record: &record, correlation_vector, trigger };
        for (_, contributor) in self.contributors_snapshot() {
            contributor.apply(&ctx, values).await;
        }
    }

    /// Runs `trigger`: sends the connect in the background, or cancels in-flight connects for
    /// [`ConnectTrigger::LockOrSleep`]. Before [`AppLayer::start`] a trigger that would send sends
    /// nothing: the start sends the engine-start connect (and the activation connect when the app is
    /// in the foreground) once every feature's provider is registered.
    pub fn trigger(&self, trigger: ConnectTrigger) {
        let Some(kind) = trigger.kind() else {
            self.cancel_connects();
            return;
        };

        if !self.is_started() {
            debug!(trigger = trigger.name(), "connect not sent; the App layer has not started");
            return;
        }
        if trigger == ConnectTrigger::BecameConnected && !self.connected_cooldown_passed() {
            debug!(trigger = trigger.name(), "connect not sent; within the cooldown of the last one");
            return;
        }

        let Some(this) = self.weak_self.upgrade() else {
            return;
        };
        let Some(handle) = self.runtime_handle() else {
            error!(trigger = trigger.name(), "connect not sent; the trigger came outside a tokio runtime");
            return;
        };

        spawn_connect(&handle, this, trigger, kind);
    }

    /// The disconnect trigger: cancels every in-flight connect.
    pub fn cancel_connects(&self) {
        let mut token = self.connect.cancel.lock_or_recover();
        token.cancel();
        *token = CancellationToken::new();
    }

    /// At most one became-`Connected` connect per cooldown; records the time when one passes.
    fn connected_cooldown_passed(&self) -> bool {
        let now = Instant::now();
        let mut last = self.connect.last_connected_trigger.lock_or_recover();
        if let Some(t) = *last {
            if now.saturating_duration_since(t) < self.config.app.connect_on_connected_cooldown {
                return false;
            }
        }

        *last = Some(now);
        true
    }
}

/// Sends a triggered connect in the background; one line when it fires and one when it ends.
fn spawn_connect(handle: &Handle, app: Arc<AppLayer>, trigger: ConnectTrigger, kind: ConnectKind) {
    let phone = app.selected_phone();
    debug!(phone = phone.as_ref().map(tracing::field::display), trigger = trigger.name(), ?kind, "connect triggered");
    handle.spawn(async move {
        let result = app.send_connect(kind, trigger.name()).await;
        log_connect_result(phone.as_ref(), trigger.name(), kind, &result);
    });
}

/// The line of a triggered connect when it ended: its outcome when it was sent, else why it was not.
fn log_connect_result(phone: Option<&DcgClient>, trigger: &str, kind: ConnectKind, result: &ConnectSendResult) {
    let phone = phone.map(tracing::field::display);
    match result {
        ConnectSendResult::Sent(response) => {
            info!(phone, trigger, ?kind, outcome = ?response.outcome, error = response.outcome.cause(), "connect completed")
        }
        ConnectSendResult::Failed(e) => error!(phone, trigger, ?kind, error = %e, "connect not sent"),
        ConnectSendResult::Dropped | ConnectSendResult::NoPhone => log_connect_skipped(phone, trigger, result),
    }
}

/// The line of a connect that was not sent because of an expected state.
fn log_connect_skipped(
    phone: Option<tracing::field::DisplayValue<&DcgClient>>,
    trigger: &str,
    result: &ConnectSendResult,
) {
    if matches!(result, ConnectSendResult::Dropped) {
        debug!(phone, trigger, "connect not sent; a background connect is in flight");
    } else {
        debug!(trigger, "connect not sent; no phone selected");
    }
}
