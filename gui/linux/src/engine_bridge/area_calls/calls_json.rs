//! The calls as the QML reads them.
//!
//! The call history's items and state, call control's status and notices, and the phone's current
//! calls.

use phonelink_engine::api::calling::{
    ActiveCall, ActiveCallRole, ActiveCallStatus, CallControlState, CallControlStatus, CallHistoryItem, CallKind,
    CallingState, CallsAccess, DialFailureKind,
};
use serde_json::{Value, json};

use super::state::PhoneRequest;
use super::sync_wait::SyncWait;

fn call_kind(kind: CallKind) -> &'static str {
    match kind {
        CallKind::Incoming => "incoming",
        CallKind::Outgoing => "outgoing",
        CallKind::Missed => "missed",
        CallKind::Voicemail => "voicemail",
        CallKind::Rejected => "rejected",
        CallKind::Blocked => "blocked",
        CallKind::AnsweredExternally => "answeredExternally",
        CallKind::Unknown => "unknown",
    }
}

pub(super) fn history_item_json(call: &CallHistoryItem, photo: Option<String>) -> Value {
    json!({
        "id": call.call_id,
        "number": call.number,
        "name": call.name,
        "contactId": call.contact_id,
        "photo": photo,
        "kind": call_kind(call.kind),
        "startMs": call.start_ms,
        "durationSecs": call.duration_secs,
        "read": call.is_read,
    })
}

fn calls_access(access: CallsAccess) -> &'static str {
    match access {
        CallsAccess::Unsupported => "unsupported",
        CallsAccess::PcConsentRequired => "consentRequired",
        CallsAccess::CallLogsUnsupported => "callLogsUnsupported",
        CallsAccess::PostNotificationRequired => "postNotificationRequired",
        CallsAccess::DevicePermissionRequired => "permissionRequired",
        CallsAccess::Granted => "granted",
    }
}

pub(super) fn calls_state_json(
    state: &CallingState,
    wait: &SyncWait,
    request: &PhoneRequest,
    access_pending: bool,
) -> Value {
    let granted = state.access == CallsAccess::Granted;
    json!({
        "access": calls_access(state.access),
        "pcConsent": state.pc_consent,
        "refreshing": state.refreshing,
        "updating": granted && (state.refreshing || (!state.up_to_date && !wait.over)),
        "unseenMissed": state.unseen_missed,
        "requesting": request.pending,
        "timedOut": request.timed_out,
        "accessPending": access_pending,
    })
}

pub(super) fn control_status(status: CallControlStatus) -> &'static str {
    match status {
        CallControlStatus::Off => "off",
        CallControlStatus::Starting => "starting",
        CallControlStatus::BluetoothOff => "bluetoothOff",
        CallControlStatus::NotPaired => "notPaired",
        CallControlStatus::Connecting => "connecting",
        CallControlStatus::ConnectionFailed => "connectionFailed",
        CallControlStatus::NotConnected => "notConnected",
        CallControlStatus::Ready => "ready",
    }
}

/// What the connection needs from the user.
///
/// `None` while it is ready, getting ready or off, and while call control or call history is off.
pub(super) fn connection_notice(state: &CallControlState) -> Option<&'static str> {
    if !state.enabled || !state.pc_consent {
        return None;
    }
    match state.status {
        CallControlStatus::BluetoothOff => Some("bluetoothOff"),
        CallControlStatus::NotPaired => Some("notPaired"),
        CallControlStatus::NotConnected => Some("notConnected"),
        CallControlStatus::ConnectionFailed => Some("connectionFailed"),
        CallControlStatus::Off
        | CallControlStatus::Starting
        | CallControlStatus::Connecting
        | CallControlStatus::Ready => None,
    }
}

pub(super) fn dial_failure(kind: DialFailureKind) -> &'static str {
    match kind {
        DialFailureKind::NoNetwork => "noNetwork",
        DialFailureKind::AirplaneMode => "airplaneMode",
        DialFailureKind::Failed => "failed",
    }
}

pub(super) fn active_call_json(call: &ActiveCall, photo: Option<String>) -> Value {
    let status = match call.status {
        ActiveCallStatus::Incoming => "incoming",
        ActiveCallStatus::Dialing => "dialing",
        ActiveCallStatus::Talking => "talking",
        ActiveCallStatus::Held => "held",
    };
    let role = match call.role {
        ActiveCallRole::Untracked => "untracked",
        ActiveCallRole::Primary => "primary",
        ActiveCallRole::Secondary => "secondary",
    };
    json!({
        "id": call.id,
        "number": call.number,
        "name": call.name,
        "contactId": call.contact_id,
        "photo": photo,
        "incoming": call.incoming,
        "status": status,
        "waiting": call.waiting,
        "talkingSinceMs": call.talking_since_ms,
        "role": role,
    })
}
