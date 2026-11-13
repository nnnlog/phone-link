//! What the PC sends the phone.

use super::{Escaping, Obj, scope, write_string};

/// `/workflow` and `/stopWorkflow`: the two keys, two-space indented, CRLF, no trailing new line.
pub fn workflow_body(name: &str, id: &str) -> Vec<u8> {
    let mut s = String::from("{\r\n  ");
    write_string(&mut s, "workflow", Escaping::Strict);
    s.push_str(": ");
    write_string(&mut s, name, Escaping::Strict);
    s.push_str(",\r\n  ");
    write_string(&mut s, "workflowId", Escaping::Strict);
    s.push_str(": ");
    write_string(&mut s, id, Escaping::Strict);
    s.push_str("\r\n}");
    s.into_bytes()
}

/// The empty orchestrator body (`wakeScreen`): no bytes.
pub fn empty_orchestrator() -> Vec<u8> {
    Vec::new()
}

/// The empty nearby body (`requestPinCode`, the skip notifications): an empty object.
pub fn empty_nearby() -> Vec<u8> {
    b"{}".to_vec()
}

pub(super) fn obj() -> Obj {
    Obj::new(Escaping::Minimal)
}

pub fn request_permission() -> Vec<u8> {
    obj().str("permission", "screenScrape").bytes()
}

/// `checkPermission`, `showPermission` and `launchSettings`.
pub fn permission_transaction(permission: &str, transaction_id: &str) -> Vec<u8> {
    obj().str("permission", permission).str("transactionId", transaction_id).bytes()
}

/// `/mirror/phone/config` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigValues {
    pub max_main_display_fps: i64,
    pub apps_feature_type: i64,
    pub width: i64,
    pub height: i64,
    pub dpi: i64,
    pub host_display_width: i64,
    pub host_display_height: i64,
}

pub fn config(c: &ConfigValues) -> Vec<u8> {
    obj()
        .num("maxMainDisplayFps", c.max_main_display_fps)
        .num("appsFeatureType", c.apps_feature_type)
        .num("width", c.width)
        .num("height", c.height)
        .num("dpi", c.dpi)
        .num("hostDisplayWidth", c.host_display_width)
        .num("hostDisplayHeight", c.host_display_height)
        .bytes()
}

pub fn always_expanded(on: bool) -> Vec<u8> {
    obj().bool("isAlwaysInExpandLayout", on).bytes()
}

pub fn launch_app(package: &str, session_id: &str, layout: i32, transfer_task_id: Option<i64>, flags: i64) -> Vec<u8> {
    let o = obj().str("packageName", package).str("sessionId", session_id).num("targetScreenLayout", layout.into());
    let o = match transfer_task_id {
        Some(t) => o.num("transferTaskId", t),
        None => o,
    };
    o.num("flags", flags).bytes()
}

pub fn launch_phone_screen_app(package: &str, session_id: &str, launch_type: i64) -> Vec<u8> {
    obj().str("packageName", package).str("sessionId", session_id).num("launchType", launch_type).bytes()
}

pub fn request_session_result(session_id: &str, transaction_id: &str, already_exists: bool, layout: i32) -> Vec<u8> {
    obj()
        .str("sessionId", session_id)
        .str("transactionId", transaction_id)
        .bool("sessionAlreadyExists", already_exists)
        .num("targetScreenLayout", layout.into())
        .bytes()
}

pub fn black_screen(on: bool) -> Vec<u8> {
    obj().bool("blackScreenToggleState", on).bytes()
}

pub fn app_resumable(transaction_id: &str) -> Vec<u8> {
    obj().str("transactionId", transaction_id).bytes()
}

pub fn launch_notification_v1(key: &str, action_index: i32) -> Vec<u8> {
    obj().str("key", key).num("actionIndex", action_index.into()).bytes()
}

pub fn launch_notification_v2(transaction_id: &str, key: &str, action_index: i32) -> Vec<u8> {
    obj().str("transactionId", transaction_id).str("key", key).num("actionIndex", action_index.into()).bytes()
}

pub const KEYBOARD_SETTINGS_INTENT: &str = "hardware_keyboard_settings";

/// The kind of the Mac's current keyboard input source, which picks the keyboard settings the phone
/// opens in the phone screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputSourceKind {
    /// A keyboard layout (ABC, U.S.): the phone's physical-keyboard settings.
    Layout,
    /// An input method (Korean, Japanese, Chinese): the phone's on-screen keyboard settings.
    InputMethod,
}

impl InputSourceKind {
    /// The `launchType` of the keyboard-settings `launchPhoneScreenApp`.
    pub fn launch_type(self) -> i64 {
        match self {
            Self::Layout => 2,
            Self::InputMethod => 3,
        }
    }
}

pub fn launch_custom_intent(intent_key: &str) -> Vec<u8> {
    obj().str("intentKey", intent_key).bytes()
}

pub fn nearby_request_setting(nearby_state: bool, transaction_id: &str) -> Vec<u8> {
    obj().bool("nearbyState", nearby_state).str("transactionId", transaction_id).bytes()
}

pub fn nearby_send_pin_code(pin: &str) -> Vec<u8> {
    obj().str("pinCode", pin).bytes()
}

pub(super) fn strict() -> Obj {
    Obj::new(Escaping::Strict)
}

pub fn disconnecting_result(transaction_id: &str) -> Vec<u8> {
    strict().str("transactionId", transaction_id).bytes()
}

pub fn request_move_to_vd_result(result: bool, transaction_id: &str) -> Vec<u8> {
    strict().bool("result", result).str("transactionId", transaction_id).bytes()
}

/// The display values a layout request may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutDisplay {
    pub width: i64,
    pub height: i64,
    pub dpi: i64,
    pub host_display_width: i64,
    pub host_display_height: i64,
}

pub fn request_screen_layout(layout: i32, transaction_id: &str, display: Option<LayoutDisplay>) -> Vec<u8> {
    let o = strict().num("targetScreenLayout", layout.into()).str("transactionId", transaction_id);
    let o = match display {
        Some(d) => o
            .num("width", d.width)
            .num("height", d.height)
            .num("dpi", d.dpi)
            .num("hostDisplayWidth", d.host_display_width)
            .num("hostDisplayHeight", d.host_display_height),
        None => o,
    };
    o.bytes()
}

/// The window's focus state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FocusableState {
    InFocus,
    NotInFocus,
    Suspended,
}

impl FocusableState {
    pub fn wire(self) -> &'static str {
        match self {
            Self::InFocus => "inFocus",
            Self::NotInFocus => "notInFocus",
            Self::Suspended => "suspended",
        }
    }
}

pub fn app_focusable_state(state: FocusableState, correlation_id: &str) -> Vec<u8> {
    strict()
        .str("Scope", "/message/windowFocusableState")
        .str("CorrelationId", correlation_id)
        .str("focusableState", state.wire())
        .bytes()
}

/// One file of a Mac → phone drag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipItem {
    pub mime_type: String,
    pub filename: String,
    pub file_size: i64,
    pub data_id: String,
}

/// The phone iterates `clipItems` unchecked, so it is always an array, never `null`.
pub fn drag_start_pc(correlation_id: &str, items: &[ClipItem]) -> Vec<u8> {
    let parts: Vec<String> = items
        .iter()
        .map(|i| {
            strict()
                .str("mimeType", &i.mime_type)
                .str("filename", &i.filename)
                .num("fileSize", i.file_size)
                .str("dataId", &i.data_id)
                .finish()
        })
        .collect();

    strict()
        .str("Scope", scope::DRAG_START_PC)
        .str("correlationId", correlation_id)
        .raw("clipItems", &format!("[{}]", parts.join(",")))
        .bytes()
}

pub fn drag_drop_pc(correlation_id: &str) -> Vec<u8> {
    strict().str("Scope", scope::DRAG_DROP_PC).str("correlationId", correlation_id).bytes()
}

pub fn drag_cancel(correlation_id: &str, reason: i64) -> Vec<u8> {
    strict().str("Scope", scope::DRAG_CANCEL).str("correlationId", correlation_id).num("reason", reason).bytes()
}

pub fn drag_request_ownership(correlation_id: &str) -> Vec<u8> {
    strict().str("Scope", scope::DRAG_REQUEST_OWNERSHIP).str("correlationId", correlation_id).bytes()
}

pub fn drag_request_item(item_id: &str) -> Vec<u8> {
    strict().str("itemId", item_id).bytes()
}

pub fn audio_pc_streaming_allowed(allowed: bool) -> Vec<u8> {
    obj().bool("isAudioStreamingAllowed", allowed).bytes()
}

pub fn audio_set_volume(package: &str, volume: i32) -> Vec<u8> {
    obj().str("packageName", package).num("volume", volume.into()).bytes()
}
