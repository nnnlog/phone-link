//! Phone Link's contract.
//!
//! It covers the `deviceStatus` content processor, the `SyncDeviceStatus` connect provider and its
//! sync request, the commands, the notifications-sync permission request, and the reset of the
//! recorded Wi-Fi state after a failed App request.
//!
//! The model they write is the phone
//! record's [`StatusKeys`](phonelink_app::StatusKeys).

use std::sync::{Arc, Weak};

use async_trait::async_trait;
use tokio::runtime::Handle;
use tracing::{debug, error, info, warn};

use phonelink_app::consts::{key, permission, result_code};
use phonelink_app::phone_record::status_capability;
use phonelink_app::{
    AppLayer, AppOutcome, AppRequest, AppResponse, ConnectContext, ConnectContribution, ConnectContributor,
    ContentProcessor, ContentProcessorFactory, InboundContext, Processed, Rejection, RequestObserver, RequestOptions,
    simple_answer,
};
use phonelink_core::codec::valueset::{Value, ValueSet};
use phonelink_core::types::{DcgClient, TraceContext};

/// The `contentType` of status content and of the sync request.
pub const CONTENT_TYPE: &str = "deviceStatus";
/// The `contentType` of a command.
pub const COMMAND_CONTENT_TYPE: &str = "deviceStatusCommand";
/// The sync request's route.
pub const SYNC_ROUTE: &str = "/devicestatus";
/// The commands' route.
pub const COMMAND_ROUTE: &str = "/command/devicestatus";

/// `DeviceStatusCommandType`. The app sends `2`, `3` and `9`.
pub mod command {
    pub const NONE: i32 = 0;
    pub const BLUETOOTH: i32 = 1;
    pub const DO_NOT_DISTURB: i32 = 2;
    pub const RINGER: i32 = 3;
    pub const AUDIO_INDICATOR: i32 = 4;
    pub const SYNC_OVER_METERED_CONNECTION: i32 = 5;
    pub const LTW_UPDATE_GENTLE: i32 = 6;
    pub const ENABLE_LTW: i32 = 7;
    pub const A2D_MIGRATION_TERMS_REVIEW_GENTLE: i32 = 8;
    pub const LAUNCH_DO_NOT_DISTURB_SETTINGS: i32 = 9;
}

/// The sync request (App `/devicestatus`): a legacy request with default options (not
/// user-initiated, priority `100`) whose `correlationVector` is its trace correlation id.
///
/// Its trace names are the `connect` message's trigger (`connect_trigger`) and `DeviceStatusSync`.
/// Its context is a fresh root, since the connect provider knows the trigger's name but not its
/// context (Phone Link on Windows sends a child of the connect trigger's context).
pub fn sync_request(connect_trigger: &str) -> AppRequest {
    let trace = TraceContext::new_root(Vec::new());
    let cid = trace.correlation_id.clone();
    let payload = ValueSet::new()
        .with(key::CORRELATION_VECTOR, cid.clone())
        .with(key::CONTENT_TYPE, CONTENT_TYPE)
        .with("relatedCorrelationIds", vec![cid])
        .with("subcontentTypes", vec![CONTENT_TYPE]);

    AppRequest::legacy(SYNC_ROUTE, payload)
        .with_options(RequestOptions::default().named(connect_trigger, "DeviceStatusSync"))
        .with_trace(trace)
}

/// A command (App `/command/devicestatus`): user-initiated (priority `0`).
pub fn command_request(command_type: i32, value: i32, trigger: &str) -> AppRequest {
    let trace = TraceContext::new_root(Vec::new());
    let payload = ValueSet::new()
        .with(key::CORRELATION_VECTOR, trace.correlation_id.clone())
        .with("deviceStatusCommandType", command_type)
        .with("deviceStatusCommandValue", value)
        .with(key::CONTENT_TYPE, COMMAND_CONTENT_TYPE);

    AppRequest::legacy(COMMAND_ROUTE, payload)
        .with_options(RequestOptions::user_initiated().named(trigger, "SendDeviceStatusCommand"))
        .with_trace(trace)
}

/// Sends `request` without waiting; only its outcome is logged.
pub(crate) fn spawn_request(runtime: &Handle, app: Arc<AppLayer>, request: AppRequest, what: &'static str) {
    runtime.spawn(async move {
        match app.request(request).await {
            Ok(response) => info!(what, outcome = ?response.outcome, "device-status request finished"),
            Err(e) => warn!(what, error = %e, "device-status request not sent"),
        }
    });
}

/// The notifications-sync permission request of the DND and ringer prompts: `/permission`
/// `permissionType` `64`, `silent` `false`, user-initiated. `Success`/`Accepted` grant the
/// permission; `Denied`/`PermanentlyDenied`/`NeedsPermission` clear it; anything else changes
/// nothing.
pub(crate) fn spawn_notifications_sync_permission_request(runtime: &Handle, app: Arc<AppLayer>, phone: DcgClient) {
    runtime.spawn(async move {
        let options =
            RequestOptions::user_initiated().named("DeviceActionIndicatorsSetup", "NotificationsDevicePermission");
        let request = app.permission_request(permission::NOTIFICATIONS_SYNC, false, options);
        let response = match app.request(request).await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "notifications-sync permission request not sent");
                return;
            }
        };

        let granted = match response.outcome {
            AppOutcome::Success | AppOutcome::Accepted => Some(true),
            AppOutcome::Denied | AppOutcome::PermanentlyDenied | AppOutcome::NeedsPermission => Some(false),
            _ => None,
        };

        info!(outcome = ?response.outcome, ?granted, "notifications-sync permission request finished");
        if let Some(granted) = granted {
            app.update_record_for(&phone, |r| r.set_permission(permission::NOTIFICATIONS_SYNC, granted));
        }
    });
}

// ------------------------------------------------------------------------------------------------
// deviceStatus content
// ------------------------------------------------------------------------------------------------

/// Creates the `deviceStatus` processor. A message without `correlationVector` (the dispatcher has
/// copied the header's in when the payload lacks it) is rejected; a value of another type is
/// accepted.
pub(crate) struct DeviceStatusFactory;

impl ContentProcessorFactory for DeviceStatusFactory {
    fn create(
        &self,
        _ctx: &InboundContext<'_>,
        payload: &ValueSet,
    ) -> Result<Option<Box<dyn ContentProcessor>>, Rejection> {
        if !payload.contains_key(key::CORRELATION_VECTOR) {
            return Err(Rejection::new("deviceStatus without correlationVector"));
        }
        Ok(Some(Box::new(DeviceStatusProcessor)))
    }
}

struct DeviceStatusProcessor;

#[async_trait]
impl ContentProcessor for DeviceStatusProcessor {
    /// The optional metadata step: `{contractVersion, itemCount: 0, result: 0}`, transaction open.
    async fn metadata(&mut self, ctx: &InboundContext<'_>, _payload: &ValueSet) -> Processed {
        Processed::open(
            ValueSet::new()
                .with(key::CONTRACT_VERSION, ctx.app.standard_keys().contract_version)
                .with(key::ITEM_COUNT, 0i32)
                .with(key::RESULT, result_code::SUCCESS),
        )
    }

    /// The item: applied to the Phone Link model (a wrongly typed key is skipped), `{result: 0}`.
    async fn content(&mut self, ctx: &InboundContext<'_>, payload: &ValueSet) -> Processed {
        if ctx.app.update_record_for(ctx.phone, |r| r.apply_device_status_item(payload)).is_none() {
            // Inbound App requests come only from the selected phone, which has a record.
            error!(phone = %ctx.phone, "deviceStatus item for a phone without a record; not applied (broken invariant)");
        }
        Processed::completed(simple_answer(result_code::SUCCESS))
    }
}

// ------------------------------------------------------------------------------------------------
// The connect contribution
// ------------------------------------------------------------------------------------------------

/// The `SyncDeviceStatus` provider: the feature's contribution to the connect message.
pub(crate) struct SyncDeviceStatusContributor;

/// The six `configuration` flags; `AirplaneMode` is whether the phone's status capabilities have
/// `0x400`.
pub fn connect_configuration(status_capabilities: u32) -> ValueSet {
    ValueSet::new()
        .with("BatteryIndicator", true)
        .with("DeviceIndicators", true)
        .with("DeviceIndicatorsBluetooth", true)
        .with("DeviceIndicatorsDND", true)
        .with("DeviceIndicatorsVolume", true)
        .with("AirplaneMode", status_capabilities & status_capability::AIRPLANE_MODE != 0)
}

/// Whether the connect reply asks for a sync (reply step 3): its `result` is `Int32 0` and it has neither
/// `batteryPercent` nor `bluetoothState`.
pub fn reply_wants_sync(response: &ValueSet) -> bool {
    matches!(response.get(key::RESULT), Some(Value::Int32(0)))
        && !response.contains_key("batteryPercent")
        && !response.contains_key("bluetoothState")
}

#[async_trait]
impl ConnectContributor for SyncDeviceStatusContributor {
    async fn contribute(&self, ctx: &ConnectContext<'_>) -> ConnectContribution {
        ConnectContribution {
            configuration: connect_configuration(ctx.record.status_capabilities),
            ..ConnectContribution::default()
        }
    }

    /// Applies the connect reply in three steps: (1) the capabilities, (2) the status keys, (3) the
    /// sync request — spawned, never awaited
    /// here — when the phone supports device-status requests (`DeviceStatusRequestSupport` or
    /// Android). `ringMyPhoneCapabilities` is written with the capabilities (ring my phone reads it);
    /// `ltwUpdate` belongs to a feature that is not built.
    async fn apply(&self, ctx: &ConnectContext<'_>, response: &ValueSet) {
        let supported = ctx.app.update_record_for(ctx.phone, |r| {
            r.apply_device_status_connect_reply(response);
            r.device_status_request_supported()
        });

        if supported == Some(true) && reply_wants_sync(response) {
            match ctx.app.self_arc() {
                Some(app) => spawn_request(&Handle::current(), app, sync_request(ctx.trigger), "sync request"),
                None => debug!("device-status sync request not sent; the App layer is shutting down"),
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------
// The Wi-Fi state reset
// ------------------------------------------------------------------------------------------------

/// The Wi-Fi state reset, the innermost handler before the sender: after an App request that passed
/// the local gates, a response that is missing or did not arrive successfully — a timeout, a send or
/// connection failure, a rejection, an unreadable payload; any `result` code counts as arrived —
/// resets the recorded Wi-Fi state to unknown. A request a local gate stopped
/// never reaches it, and one its caller canceled passes it by.
pub(crate) struct WifiStateReset {
    app: Weak<AppLayer>,
}

impl WifiStateReset {
    pub(crate) fn new(app: &Arc<AppLayer>) -> Self {
        Self { app: Arc::downgrade(app) }
    }

    /// Whether `response` resets the Wi-Fi state: no response arrived and neither a local gate nor
    /// the request's caller stopped it — a `Canceled` the transport reported (the hub connection
    /// closed during the send, the queued send dropped) resets it like any other missing response.
    pub(crate) fn resets(response: &AppResponse) -> bool {
        response.local_gate.is_none() && !response.canceled_by_caller && !response.arrived()
    }
}

impl RequestObserver for WifiStateReset {
    fn on_request_completed(&self, phone: &DcgClient, route: &str, response: &AppResponse) {
        if !Self::resets(response) {
            return;
        }

        let Some(app) = self.app.upgrade() else { return };
        if app.update_record_for(phone, |r| r.status_keys.reset_wifi_state()).is_some() {
            debug!(route, outcome = ?response.outcome, "App request failed; the recorded Wi-Fi state is reset to unknown");
        } else {
            warn!(%phone, "Wi-Fi state not reset; the phone has no record");
        }
    }
}
