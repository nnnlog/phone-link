//! The pairing wire values.

use super::session::Cause;
use super::{PairingResult, ShownCode};
use serde_json::{Map, Value};
use std::time::SystemTime;

/// How the user pairs: by scanning a QR code (then confirming a challenge PIN), or by typing a PIN
/// into Link to Windows after opening a link by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PairingFlow {
    /// `QRLINK_CHALLENGEPIN_SIGNIN`.
    QrCode,
    /// `SIGNIN_PIN`.
    ManualPin,
}

impl PairingFlow {
    pub fn wire(self) -> &'static str {
        match self {
            PairingFlow::QrCode => "QRLINK_CHALLENGEPIN_SIGNIN",
            PairingFlow::ManualPin => "SIGNIN_PIN",
        }
    }
}

/// The permission set the phone is asked to grant (`optionalPermission`).
pub const OPTIONAL_PERMISSIONS: [&str; 1] = ["ALL"];

/// A pairing session's `state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceState {
    PendingPairingLinkVisit,
    PairingLinkExpired,
    PendingChallengePinValidate,
    ChallengePinExpired,
    ChallengePinValidateFailed,
    PendingPairingPinValidate,
    PairingAccountMismatched,
    PairingPinExpired,
    PendingPairingPermissionApprove,
    PairingRequiredPermissionDenied,
    ReadyForComplete,
    PairingFailed,
    DevicePaired,
    PairingSessionExpired,
}

pub(super) const STATES: [(&str, ServiceState); 14] = [
    ("PENDING_PAIRING_LINK_VISIT", ServiceState::PendingPairingLinkVisit),
    ("PAIRING_LINK_EXPIRED", ServiceState::PairingLinkExpired),
    ("PENDING_CHALLENGE_PIN_VALIDATE", ServiceState::PendingChallengePinValidate),
    ("CHALLENGE_PIN_EXPIRED", ServiceState::ChallengePinExpired),
    ("CHALLENGE_PIN_VALIDATE_FAILED", ServiceState::ChallengePinValidateFailed),
    ("PENDING_PAIRING_PIN_VALIDATE", ServiceState::PendingPairingPinValidate),
    ("PAIRING_ACCOUNT_MISMATCHED", ServiceState::PairingAccountMismatched),
    ("PAIRING_PIN_EXPIRED", ServiceState::PairingPinExpired),
    ("PENDING_PAIRING_PERMISSION_APPROVE", ServiceState::PendingPairingPermissionApprove),
    ("PAIRING_REQUIRED_PERMISSION_DENIED", ServiceState::PairingRequiredPermissionDenied),
    ("READY_FOR_COMPLETE", ServiceState::ReadyForComplete),
    ("PAIRING_FAILED", ServiceState::PairingFailed),
    ("DEVICE_PAIRED", ServiceState::DevicePaired),
    ("PAIRING_SESSION_EXPIRED", ServiceState::PairingSessionExpired),
];

impl ServiceState {
    /// Parses a `state` value, ignoring case; `None` for any other string.
    pub fn parse(value: &str) -> Option<Self> {
        STATES.iter().find(|(name, _)| name.eq_ignore_ascii_case(value)).map(|(_, s)| *s)
    }

    pub fn wire(self) -> &'static str {
        STATES.iter().find(|(_, s)| *s == self).map(|(name, _)| *name).expect("every state has a wire name")
    }
}

/// The create request's body: `flow` and `optionalPermission`; `requiredPermission` and `nextFlow`
/// are never set and omitted.
pub fn create_request_body(flow: PairingFlow) -> String {
    serde_json::json!({ "flow": flow.wire(), "optionalPermission": OPTIONAL_PERMISSIONS }).to_string()
}

/// One session response: the keys the PC reads, as sent (`state` parsed).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionResponse {
    pub state: ServiceState,
    fields: Map<String, Value>,
}

impl SessionResponse {
    /// A response body: a JSON object whose `state` is one of [`ServiceState`] (any case). A body that
    /// is not a JSON object is `InternalError`; a missing, empty, non-string or unknown `state` is
    /// `ServiceError`.
    #[cfg(any(test, feature = "test-support"))]
    pub fn parse(body: &str) -> Result<Self, PairingResult> {
        Self::parse_reply(body).map_err(|(result, _)| result)
    }

    /// [`SessionResponse::parse`], with what made the body unusable.
    pub(super) fn parse_reply(body: &str) -> Result<Self, (PairingResult, Cause)> {
        let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(body) else {
            return Err((PairingResult::InternalError, Cause::NotAnObject { body_len: body.len() }));
        };

        let state = match fields.get("state") {
            Some(Value::String(s)) => ServiceState::parse(s).ok_or_else(|| Some(s.clone())),
            _ => Err(None),
        };

        match state {
            Ok(state) => Ok(Self { state, fields }),
            Err(raw) => Err((PairingResult::ServiceError, Cause::UnknownState(raw))),
        }
    }

    /// A string key's value; `None` when absent, not a string, or empty.
    pub fn text(&self, key: &str) -> Option<&str> {
        self.fields.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
    }

    /// The expiry `key` moved onto the PC's clock by the skew this response measures:
    /// `parse(key) + (now − parse(currentServiceTime))`. `None` when either is missing or unparsable.
    pub(super) fn local_expiry(&self, key: &str, now: SystemTime) -> Option<SystemTime> {
        let expiry = phonelink_core::time::parse_date_time(self.text(key)?)?;
        let service_now = phonelink_core::time::parse_date_time(self.text("currentServiceTime")?)?;
        Some(phonelink_core::time::system_time(
            expiry.checked_sub(service_now)?.checked_add(phonelink_core::time::unix_ms(now))?,
        ))
    }

    /// A value the PC shows (`value_key`) with its expiry (`expiry_key`): both present and parsable,
    /// and the expiry at least a minute away once rounded.
    pub(super) fn shown_code(&self, value_key: &str, expiry_key: &str, now: SystemTime) -> Option<ShownCode> {
        let value = self.text(value_key)?.to_string();
        let expires_at = self.local_expiry(expiry_key, now)?;
        let minutes = minutes_until(expires_at, now);
        (minutes >= 1).then_some(ShownCode { value, expires_at, expires_in_minutes: minutes as u32 })
    }
}

/// `round((expiry − now) / 1 min)`, negative once past.
pub(super) fn minutes_until(expiry: SystemTime, now: SystemTime) -> i64 {
    let ms = phonelink_core::time::unix_ms(expiry) - phonelink_core::time::unix_ms(now);
    (ms as f64 / 60_000.0).round() as i64
}
