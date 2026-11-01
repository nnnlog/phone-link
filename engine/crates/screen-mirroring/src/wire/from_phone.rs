//! What the phone sends the PC.

use serde_json::Value;

/// Why an inbound body was dropped.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("the body is not a JSON object")]
    NotAnObject,
    #[error("required key {0} missing or of another type")]
    Missing(&'static str),
}

/// A parsed inbound body.
pub struct Body(serde_json::Map<String, Value>);

impl Body {
    pub fn parse(bytes: &[u8]) -> Result<Self, ParseError> {
        match serde_json::from_slice::<Value>(bytes) {
            Ok(Value::Object(m)) => Ok(Self(m)),
            _ => Err(ParseError::NotAnObject),
        }
    }

    pub fn str(&self, k: &'static str) -> Result<String, ParseError> {
        self.opt_str(k).ok_or(ParseError::Missing(k))
    }

    pub fn opt_str(&self, k: &str) -> Option<String> {
        self.0.get(k).and_then(Value::as_str).map(str::to_string)
    }

    pub fn bool(&self, k: &'static str) -> Result<bool, ParseError> {
        self.0.get(k).and_then(Value::as_bool).ok_or(ParseError::Missing(k))
    }

    pub fn opt_bool(&self, k: &str) -> Option<bool> {
        self.0.get(k).and_then(Value::as_bool)
    }

    /// An `Int32` number.
    pub fn int(&self, k: &'static str) -> Result<i32, ParseError> {
        self.opt_int(k).ok_or(ParseError::Missing(k))
    }

    pub fn opt_int(&self, k: &str) -> Option<i32> {
        self.0.get(k).and_then(Value::as_i64).and_then(|v| i32::try_from(v).ok())
    }

    /// A number read as a double and truncated toward zero.
    pub fn number(&self, k: &'static str) -> Result<i64, ParseError> {
        self.0.get(k).and_then(Value::as_f64).map(|v| v as i64).ok_or(ParseError::Missing(k))
    }

    pub fn raw(&self, k: &str) -> Option<&Value> {
        self.0.get(k)
    }
}

/// `/stopWorkflow` from the phone: `workflow` required, `workflowId` optional.
pub fn parse_stop_workflow(bytes: &[u8]) -> Result<(String, Option<String>), ParseError> {
    let b = Body::parse(bytes)?;
    Ok((b.str("workflow")?, b.opt_str("workflowId")))
}

/// The files of a phone → Mac drag (`/drag/clipData`'s `clipDataItems`): four parallel arrays, all
/// required when the object is present; their lengths are not checked here.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClipData {
    pub data_ids: Vec<String>,
    pub mime_types: Vec<String>,
    pub file_sizes: Vec<i64>,
    pub filenames: Vec<String>,
}

pub(super) fn strings(v: Option<&Value>, k: &'static str) -> Result<Vec<String>, ParseError> {
    v.and_then(Value::as_array)
        .and_then(|a| a.iter().map(|e| e.as_str().map(str::to_string)).collect())
        .ok_or(ParseError::Missing(k))
}

/// The drag bodies are read strictly: a required key must be present (`null` allowed for a string), a present
/// key must have its type (`null` allowed for a string or a list), and anything else drops the message.
pub(super) fn drag_body(bytes: &[u8]) -> Result<Body, ParseError> {
    let b = Body::parse(bytes)?;
    match b.raw("correlationId") {
        Some(Value::String(_) | Value::Null) => Ok(b),
        _ => Err(ParseError::Missing("correlationId")),
    }
}

/// An optional string key: absent or `null` → `None`; another type drops the message.
pub(super) fn opt_string(b: &Body, k: &'static str) -> Result<Option<String>, ParseError> {
    match b.raw(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(ParseError::Missing(k)),
    }
}

/// An optional `Int32` key: absent → `None`; another type or range drops the message.
pub(super) fn opt_i32(b: &Body, k: &'static str) -> Result<Option<i32>, ParseError> {
    match b.raw(k) {
        None => Ok(None),
        Some(v) => v.as_i64().and_then(|n| i32::try_from(n).ok()).map(Some).ok_or(ParseError::Missing(k)),
    }
}

/// `/drag/startPhone`: `correlationId` required; `mimeTypes` a list of strings or `null`; `sessionId`
/// a string or `null`.
pub fn parse_drag_start_phone(bytes: &[u8]) -> Result<(), ParseError> {
    let b = drag_body(bytes)?;
    match b.raw("mimeTypes") {
        None | Some(Value::Null) => {}
        v @ Some(_) => {
            strings(v, "mimeTypes")?;
        }
    }
    opt_string(&b, "sessionId")?;
    Ok(())
}

/// `/drag/clipData`: `correlationId` required; `clipDataItems` optional (absent or `null` → `None`),
/// its four arrays required.
pub fn parse_clip_data(bytes: &[u8]) -> Result<Option<ClipData>, ParseError> {
    let b = drag_body(bytes)?;
    let items = match b.raw("clipDataItems") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Object(m)) => m,
        Some(_) => return Err(ParseError::Missing("clipDataItems")),
    };

    let file_sizes = items
        .get("fileSizes")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().map(Value::as_i64).collect())
        .ok_or(ParseError::Missing("fileSizes"))?;

    Ok(Some(ClipData {
        data_ids: strings(items.get("dataIds"), "dataIds")?,
        mime_types: strings(items.get("mimeTypes"), "mimeTypes")?,
        file_sizes,
        filenames: strings(items.get("filenames"), "filenames")?,
    }))
}

/// `/drag/cancel` from the phone: its `reason` (`0` Unknown when absent); `correlationId` required.
pub fn parse_drag_cancel(bytes: &[u8]) -> Result<i32, ParseError> {
    let b = drag_body(bytes)?;
    Ok(opt_i32(&b, "reason")?.unwrap_or(0))
}

/// `/drag/complete`: its `dropTarget`; `correlationId` required, `droppedItemCount` an `Int32`.
pub fn parse_drag_complete(bytes: &[u8]) -> Result<Option<String>, ParseError> {
    let b = drag_body(bytes)?;
    opt_i32(&b, "droppedItemCount")?;
    opt_string(&b, "dropTarget")
}

/// `/drag/requestItem` from the phone: `itemId` must be a string.
pub fn parse_drag_request_item(bytes: &[u8]) -> Result<String, ParseError> {
    Body::parse(bytes)?.str("itemId")
}

/// `setLockedState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LockedState {
    pub is_locked: bool,
    pub is_folded: bool,
    pub can_draw: bool,
    pub is_secure_locked: bool,
    pub is_screen_on: bool,
    pub is_always_unlocked_display: bool,
    pub is_virtual_device_streaming_supported: bool,
}

pub fn parse_locked_state(bytes: &[u8]) -> Result<LockedState, ParseError> {
    let b = Body::parse(bytes)?;
    Ok(LockedState {
        is_locked: b.bool("isLocked")?,
        is_folded: b.bool("isFolded")?,
        can_draw: b.bool("canDraw")?,
        is_secure_locked: b.opt_bool("isSecureLocked").unwrap_or(false),
        is_screen_on: b.opt_bool("isScreenOn").unwrap_or(true),
        is_always_unlocked_display: b.opt_bool("isAlwaysUnlockedDisplay").unwrap_or(false),
        is_virtual_device_streaming_supported: b.opt_bool("isVirtualDeviceStreamingSupported").unwrap_or(false),
    })
}

/// `launchNotificationResult`: `launchResult`, and `transactionId` or `key`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationResult {
    pub launch_result: i64,
    pub transaction_id: Option<String>,
    pub key: Option<String>,
}

pub fn parse_notification_result(bytes: &[u8]) -> Result<NotificationResult, ParseError> {
    let b = Body::parse(bytes)?;
    Ok(NotificationResult {
        launch_result: b.number("launchResult")?,
        transaction_id: b.opt_str("transactionId"),
        key: b.opt_str("key"),
    })
}

/// The `DisconnectingReason` names, compared ignoring case; anything else is `Unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DisconnectingReason {
    None,
    Unknown,
    UserClosed,
    AppIsClosed,
    MaxApps,
    AppIsMovedToAnotherDisplay,
    CloseWindow,
    CloseAll,
    Heat,
    ClosedByBack,
    PowerSavingMode,
    Error2,
    Error3,
    MediaProjectionDisconnected,
    InputInjectorDisconnected,
    ScreenMirrorServiceStop,
    ScreenMirrorWorkflowStop,
}

impl DisconnectingReason {
    const NAMES: [(&str, Self); 17] = [
        ("None", Self::None),
        ("Unknown", Self::Unknown),
        ("UserClosed", Self::UserClosed),
        ("AppIsClosed", Self::AppIsClosed),
        ("MaxApps", Self::MaxApps),
        ("AppIsMovedToAnotherDisplay", Self::AppIsMovedToAnotherDisplay),
        ("CloseWindow", Self::CloseWindow),
        ("CloseAll", Self::CloseAll),
        ("Heat", Self::Heat),
        ("ClosedByBack", Self::ClosedByBack),
        ("PowerSavingMode", Self::PowerSavingMode),
        ("Error2", Self::Error2),
        ("Error3", Self::Error3),
        ("MediaProjectionDisconnected", Self::MediaProjectionDisconnected),
        ("InputInjectorDisconnected", Self::InputInjectorDisconnected),
        ("ScreenMirrorServiceStop", Self::ScreenMirrorServiceStop),
        ("ScreenMirrorWorkflowStop", Self::ScreenMirrorWorkflowStop),
    ];

    /// A name, or its number written as text.
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if let Ok(i) = s.parse::<usize>() {
            return Self::NAMES.get(i).map_or(Self::Unknown, |(_, r)| *r);
        }
        Self::NAMES.iter().find(|(n, _)| n.eq_ignore_ascii_case(s)).map_or(Self::Unknown, |(_, r)| *r)
    }
}

/// `disconnecting`: the reason (a string, or a number written as text) and the transaction id.
///
/// `disconnecting`: its reason and its `transactionId`, the empty string when absent (the
/// acknowledgement carries it either way).
pub fn parse_disconnecting(bytes: &[u8]) -> Result<(DisconnectingReason, String), ParseError> {
    let b = Body::parse(bytes)?;
    let reason = match b.raw("reason") {
        Some(Value::String(s)) => DisconnectingReason::parse(s),
        Some(Value::Number(n)) => DisconnectingReason::parse(&n.to_string()),
        _ => DisconnectingReason::Unknown,
    };

    let tx = match b.raw("transactionId") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    };
    Ok((reason, tx))
}
