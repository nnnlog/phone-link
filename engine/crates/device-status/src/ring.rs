//! Ring my phone: the ring control of the selected phone, the `RING_MY_PHONE` request it sends
//! through the App layer, and what the control shows.
//!
//! The request is a `/DeviceResourceManager` `UPDATE` addressed by the type `RING_MY_PHONE`, path
//! `/ringmyphone`, whose payload is a `RingMyPhoneRequestMessage`. The control counts down 20 s
//! whatever the request's outcome; only a stop that fails brings the ringing state back. The
//! reply's `RingMyPhoneResponseStatus` is logged and changes nothing.
//! The feature handle ([`super::DeviceStatus`]) owns the per-phone state and the countdown.

use std::time::Duration;

use prost::Message;
use tracing::{info, warn};

use phonelink_app::phone_record::status_capability as sc;
use phonelink_app::{AppResponse, DrmAppRequest, PhoneRecord, RequestOptions};
use phonelink_proto::platform_sdk::v1::{ProtoDeviceResourceRequestType, ProtoDeviceResourceType};
use phonelink_proto::ring_my_phone::v1::{
    RingMyPhoneRequestMessage, RingMyPhoneRequestType, RingMyPhoneResponseMessage, RingMyPhoneResponseStatus,
};

/// `ypTriggerId` of both requests.
pub const TRIGGER: &str = "ToggleRingMyPhoneIndicator";
/// `ypScenarioId` of both requests.
pub const SCENARIO: &str = "SendRingMyPhoneRequest";

/// What the ring control shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingShown {
    /// The phone is not connected, or its features are all disabled: the control is disabled.
    Unknown,
    /// Not ringing: the control offers *ring*.
    Off,
    /// Ringing for `remaining_seconds` more (`20` … `1`). `confirm_stop`: the user asked to stop once;
    /// the next stop sends it.
    Ringing { remaining_seconds: u32, confirm_stop: bool },
}

/// The ring control of the selected phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingControl {
    pub shown: RingShown,
    /// The phone is connected and its features are not all disabled.
    pub enabled: bool,
    /// A ringing control offers *stop* (the stop option is on).
    pub can_stop: bool,
}

/// Which request a control sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingRequest {
    On,
    Off,
}

/// The ring control exists for `record`.
///
/// The phone can ring (its last `connect` reply said so), the option is on, the phone is Android
/// with the device action strip (`DeviceActionIndicator`), and it belongs to the enrolled account
/// (`in_account`).
pub fn exists(record: &PhoneRecord, option_on: bool, in_account: bool) -> bool {
    record.ring_my_phone_capable
        && option_on
        && record.platform == phonelink_app::consts::platform::ANDROID
        && record.status_capabilities & sc::DEVICE_ACTION_INDICATOR != 0
        && in_account
}

/// The whole seconds left of a countdown with `left` remaining, at least `1` while it runs.
pub fn remaining_seconds(left: Duration) -> u32 {
    let ms = left.as_millis();
    u32::try_from(ms.div_ceil(1000)).unwrap_or(u32::MAX).max(1)
}

/// The `RingMyPhoneRequestMessage` bytes of `request`.
pub fn request_payload(request: RingRequest) -> Vec<u8> {
    let request_type = match request {
        RingRequest::On => RingMyPhoneRequestType::On,
        RingRequest::Off => RingMyPhoneRequestType::Off,
    };
    RingMyPhoneRequestMessage { request_type: request_type as i32 }.encode_to_vec()
}

/// The App-layer DRM request of `request`: type `RING_MY_PHONE`, `UPDATE`, user-initiated, with the
/// ring indicator's trace names and `timeout`.
pub fn drm_request(request: RingRequest, timeout: Duration) -> DrmAppRequest {
    DrmAppRequest {
        resource_type: ProtoDeviceResourceType::RingMyPhone,
        request_type: ProtoDeviceResourceRequestType::Update,
        payload: request_payload(request),
        options: RequestOptions::user_initiated().named(TRIGGER, SCENARIO).with_timeout(timeout),
        trace: None,
    }
}

/// Logs a request's outcome and the reply's status; the control does not read either.
pub fn log_outcome(request: RingRequest, response: &AppResponse) {
    match reply_status(response) {
        Some(Ok(status)) => {
            info!(request_kind = ?request, outcome = ?response.outcome, %status, "ring my phone request answered");
        }
        Some(Err(e)) => {
            warn!(request_kind = ?request, error = %e, "ring my phone reply is not a RingMyPhoneResponseMessage; ignored");
        }
        None => {
            warn!(request_kind = ?request, outcome = ?response.outcome, error = response.outcome.cause(), "ring my phone request failed")
        }
    }
}

/// Logs a stop the phone did not carry out, in place of [`log_outcome`]: `is_shown_ringing` when
/// the control shows the phone ringing again.
pub fn log_stop_not_delivered(response: &AppResponse, is_shown_ringing: bool) {
    let status = reply_status(response).and_then(Result::ok);
    warn!(outcome = ?response.outcome, error = response.outcome.cause(), ?status, is_shown_ringing, "ring my phone stop not delivered");
}

/// The status of `response`'s reply as text; `None` without a reply.
fn reply_status(response: &AppResponse) -> Option<Result<String, prost::DecodeError>> {
    let bytes = response.resource_payload.as_deref()?;
    Some(RingMyPhoneResponseMessage::decode(bytes).map(|reply| {
        RingMyPhoneResponseStatus::try_from(reply.status)
            .map_or_else(|_| format!("unknown ({})", reply.status), |s| format!("{s:?}"))
    }))
}
