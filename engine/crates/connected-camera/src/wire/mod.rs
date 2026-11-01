//! The messages of the connected camera.
//!
//! It covers the `CAMERA_METADATA` and `PERMISSIONS` requests and how their replies are read, the
//! phone's tag-2 and tag-3 publications, and the JSON control messages on the stream channel.

use prost::Message;
use serde::{Deserialize, Serialize};

use phonelink_core::types::new_guid_lower;
use phonelink_proto::permissions::v1::{
    CorePermissions, FeaturePermissions, PermissionsRequestMessage, PermissionsRequestStatus,
    PermissionsResponseMessage,
};
use phonelink_proto::streaming::v1::StreamingConfigurationMessage;

/// The MSAEP tag of the phone's camera publication, which carries its context handle.
pub const CAMERA_TAG: i16 = 2;
/// The MSAEP tag of the phone's permission publication.
pub const PERMISSIONS_TAG: i16 = 3;

/// The phone's device-list capability for the feature.
pub const CAPABILITY: &str = "CAMERA_STREAMING";

/// The `VideoStreaming` scenario of a stream.
pub const SCENARIO: &str = phonelink_nano::device::scenario::STREAMING;

/// The auto-prompt's payload.
pub const AUTO_PROMPT_PAYLOAD: &[u8] = b"CameraStreamingPermissionsRequested";

/// The channel scopes of the control messages.
pub mod scope {
    pub const ACTION: &str = "/streaming/action";
    pub const STATUS: &str = "/streaming/status";
    pub const BATTERY: &str = "/streaming/indicators/battery";
    pub const NETWORK: &str = "/streaming/network";
    pub const CONFIGURATION: &str = "/streaming/configuration";
    /// Every scope the phone sends on.
    pub const INBOUND: &[&str] = &[ACTION, STATUS, BATTERY, NETWORK, CONFIGURATION];
}

/// The trace-state names (`ypScenarioId`, `ypTriggerId`) of each request.
pub mod trace_name {
    pub const SCENARIO: &str = "CameraStream";
    pub const SYNC: &str = "CDEH.CameraStream.SendDeviceRequest";
    pub const PERMISSION_REQUEST: &str = "CDEH.CameraStream.RequestPermissionForDeviceAsync";
    pub const AUTO_PROMPT: &str = "CDEH.CameraStream.StartCameraStream";
    pub const SETTINGS_PERMISSION: &str = "CDEH.CameraStream.OnRequestPermissionAsync";
}

// ------------------------------------------------------------------------------------------------
// Publications and requests
// ------------------------------------------------------------------------------------------------

/// What the phone's tag-2 publication says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CameraPublication {
    pub capable: bool,
    pub camera_permission: bool,
    pub notification_permission: bool,
}

/// Tag 2's `PubSubPayload.data`.
pub fn camera_publication(data: &[u8]) -> Result<CameraPublication, prost::DecodeError> {
    let m = StreamingConfigurationMessage::decode(data)?;
    Ok(CameraPublication {
        capable: m.is_capable & 1 == 1,
        camera_permission: m.permissions_granted & 1 == 1,
        notification_permission: m.core_permissions_granted & 1 == 1,
    })
}

/// The auto-prompt's reply body says the camera permission is permanently denied.
pub fn auto_prompt_permanently_denied(body: &[u8]) -> bool {
    StreamingConfigurationMessage::decode(body).is_ok_and(|m| m.permissions_permanently_denied)
}

/// `PermissionsRequestMessage { feature_permissions: [CAMERA] }`.
pub fn permission_request() -> Vec<u8> {
    PermissionsRequestMessage { feature_permissions: vec![FeaturePermissions::Camera as i32] }.encode_to_vec()
}

/// What an accepted permission reply grants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionReply {
    /// `POST_NOTIFICATIONS` is granted.
    pub notifications: bool,
    /// `CAMERA` is granted.
    pub camera: bool,
    /// The QR code for the notification permission, when non-empty.
    pub qr_code_uri: Option<String>,
    /// `CAMERA` is permanently denied.
    pub camera_permanently_denied: bool,
}

/// An `ACCEPTED` reply to the permission request; `None` for any other reply.
pub fn permission_reply(payload: &[u8]) -> Option<PermissionReply> {
    let m = PermissionsResponseMessage::decode(payload).ok()?;
    if m.request_status != PermissionsRequestStatus::Accepted as i32 {
        return None;
    }

    let camera = FeaturePermissions::Camera as i32;
    Some(PermissionReply {
        notifications: m.core_permissions.contains(&(CorePermissions::PostNotifications as i32)),
        camera: m.feature_permissions.contains(&camera),
        qr_code_uri: m.qr_code_uri.filter(|u| !u.is_empty()),
        camera_permanently_denied: m.permanently_denied_permissions.contains(&camera),
    })
}

/// What a tag-3 publication grants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    /// The camera (and with it the notification) permission.
    Camera,
    /// The notification permission alone.
    Core,
    Nothing,
}

/// Tag 3's `PubSubPayload.additional`.
pub fn permission_publication(additional: &[u8]) -> Result<Grant, prost::DecodeError> {
    let m = PermissionsResponseMessage::decode(additional)?;
    if m.request_status != PermissionsRequestStatus::Accepted as i32 {
        return Ok(Grant::Nothing);
    }

    let qr_empty = m.qr_code_uri.as_deref().unwrap_or("").is_empty();
    Ok(if m.feature_permissions.contains(&(FeaturePermissions::Camera as i32)) && qr_empty {
        Grant::Camera
    } else if m.core_permissions.contains(&(CorePermissions::PostNotifications as i32)) {
        Grant::Core
    } else {
        Grant::Nothing
    })
}

// ------------------------------------------------------------------------------------------------
// Control messages
// ------------------------------------------------------------------------------------------------

/// `CameraStreamAction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Unknown,
    Start,
    Pause,
    Resume,
    Stop,
    Update,
    Error,
    Initialize,
}

impl Action {
    pub fn wire(self) -> i32 {
        match self {
            Self::Unknown => 0,
            Self::Start => 1,
            Self::Pause => 2,
            Self::Resume => 3,
            Self::Stop => 4,
            Self::Update => 5,
            Self::Error => 6,
            Self::Initialize => 7,
        }
    }

    pub fn from_wire(v: i32) -> Option<Self> {
        Some(match v {
            0 => Self::Unknown,
            1 => Self::Start,
            2 => Self::Pause,
            3 => Self::Resume,
            4 => Self::Stop,
            5 => Self::Update,
            6 => Self::Error,
            7 => Self::Initialize,
            _ => return None,
        })
    }
}

/// `CameraStreamStatus`.
pub mod status {
    pub const TAP_NOTIF_TO_START: i32 = 4;
}

/// `CameraError`: the PC's local error states and the phone's reported errors. A value the PC does not
/// name is kept as it came.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CameraError {
    None,
    CameraDeviceFatal,
    CameraDisabled,
    CameraInUse,
    CameraServiceFatal,
    MaxCamerasInUse,
    Disconnected,
    NoCameraAvailable,
    ScreenLocked,
    Unknown,
    CameraPermissionNeeded,
    LtwDisabled,
    CallInProgress,
    MeteredConnection,
    DeviceHeat,
    ChannelNotCreated,
    CameraPermissionDenied,
    PostNotificationPermissionNeeded,
    ChannelDisconnected,
    SessionConfigurationFailed,
    CameraPermissionPermanentlyDenied,
    CameraPermissionNeededAutoprompt,
    CameraPermissionRevoked,
    Other(i32),
}

impl CameraError {
    const NAMED: &[(i32, CameraError)] = &[
        (0, Self::None),
        (1, Self::CameraDeviceFatal),
        (2, Self::CameraDisabled),
        (3, Self::CameraInUse),
        (4, Self::CameraServiceFatal),
        (5, Self::MaxCamerasInUse),
        (6, Self::Disconnected),
        (7, Self::NoCameraAvailable),
        (8, Self::ScreenLocked),
        (9, Self::Unknown),
        (10, Self::CameraPermissionNeeded),
        (11, Self::LtwDisabled),
        (12, Self::CallInProgress),
        (13, Self::MeteredConnection),
        (14, Self::DeviceHeat),
        (15, Self::ChannelNotCreated),
        (16, Self::CameraPermissionDenied),
        (18, Self::PostNotificationPermissionNeeded),
        (20, Self::ChannelDisconnected),
        (21, Self::SessionConfigurationFailed),
        (22, Self::CameraPermissionPermanentlyDenied),
        (23, Self::CameraPermissionNeededAutoprompt),
        (24, Self::CameraPermissionRevoked),
    ];

    pub fn wire(self) -> i32 {
        match self {
            Self::Other(v) => v,
            named => {
                Self::NAMED.iter().find(|(_, e)| *e == named).map(|(v, _)| *v).expect("every named error has a value")
            }
        }
    }

    pub fn from_wire(v: i32) -> Self {
        Self::NAMED.iter().find(|(w, _)| *w == v).map(|(_, e)| *e).unwrap_or(Self::Other(v))
    }

    /// The four camera-permission errors.
    pub fn is_permission(self) -> bool {
        matches!(
            self,
            Self::CameraPermissionNeeded
                | Self::CameraPermissionDenied
                | Self::CameraPermissionPermanentlyDenied
                | Self::CameraPermissionNeededAutoprompt
        )
    }
}

/// `CameraFace`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CameraFace {
    Front,
    Back,
}

impl CameraFace {
    pub fn wire(self) -> i32 {
        match self {
            Self::Front => 0,
            Self::Back => 1,
        }
    }

    pub fn from_wire(v: i32) -> Option<Self> {
        match v {
            0 => Some(Self::Front),
            1 => Some(Self::Back),
            _ => None,
        }
    }
}

/// The `DefaultCameraExtensions` flags, in flag order.
pub const DEFAULT_EXTENSIONS: &[i32] = &[1, 2, 4, 8, 16];

/// One localized text of an OEM effect's name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fallback {
    pub locale: String,
    pub value: String,
}

/// An OEM effect's name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExtensionName {
    pub resource: String,
    pub fallbacks: Vec<Fallback>,
}

/// An OEM effect.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OemExtension {
    pub id: i32,
    pub name: ExtensionName,
    pub icon: Option<String>,
}

/// `CameraConfigPayload`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CameraConfig {
    pub camera_face: i32,
    pub oem_extension_enabled: bool,
    pub supported_default_extensions: i32,
    pub selected_default_extension: i32,
    pub selected_oem_extension: Option<OemExtension>,
    pub supported_oem_extensions: Option<Vec<OemExtension>>,
}

/// `/streaming/action`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ActionMessage {
    pub scope: String,
    pub correlation_id: String,
    pub action: i32,
    pub camera_error: Option<i32>,
    pub camera_config_payload: Option<CameraConfig>,
}

/// `/streaming/status`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StatusMessage {
    pub scope: String,
    pub correlation_id: String,
    pub current_status: i32,
}

/// `/streaming/indicators/battery`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BatteryMessage {
    pub scope: String,
    pub correlation_id: String,
    pub is_charging: bool,
    pub battery_level: i32,
}

/// `/streaming/network`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NetworkMessage {
    pub scope: String,
    pub correlation_id: String,
    pub is_low_quality_network: bool,
}

/// `/streaming/configuration`: read and dropped.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ConfigurationMessage {
    pub scope: String,
    pub correlation_id: String,
    pub available_configurations: Vec<CameraConfig>,
    pub current_configuration: Option<CameraConfig>,
}

/// An action message to the phone.
pub fn action(action: Action, camera_error: Option<CameraError>, config: Option<CameraConfig>) -> Vec<u8> {
    let m = ActionMessage {
        scope: scope::ACTION.to_owned(),
        correlation_id: new_guid_lower(),
        action: action.wire(),
        camera_error: camera_error.map(CameraError::wire),
        camera_config_payload: config,
    };
    serde_json::to_vec(&m).expect("an action message serializes")
}

/// The network-quality message.
pub fn network(low: bool) -> Vec<u8> {
    let m = NetworkMessage {
        scope: scope::NETWORK.to_owned(),
        correlation_id: new_guid_lower(),
        is_low_quality_network: low,
    };
    serde_json::to_vec(&m).expect("a network message serializes")
}

/// The configuration of a camera switch: only the face.
pub fn face_config(face: CameraFace) -> CameraConfig {
    CameraConfig { camera_face: face.wire(), ..CameraConfig::default() }
}

/// A message the phone sent, by its channel scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inbound {
    Action(ActionMessage),
    Status(StatusMessage),
    Battery(BatteryMessage),
    Configuration(ConfigurationMessage),
    /// The phone's own network message (only the PC's is acted on).
    Network(NetworkMessage),
}

/// Why an inbound message is dropped.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InboundError {
    #[error("unknown scope {0}")]
    UnknownScope(String),
    #[error("empty body")]
    Empty,
    #[error("body does not parse: {0}")]
    Parse(String),
}

/// The message on channel scope `scope` (the prefix stripped).
pub fn inbound(scope: &str, body: &[u8]) -> Result<Inbound, InboundError> {
    if body.is_empty() {
        return Err(InboundError::Empty);
    }

    fn parse<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, InboundError> {
        serde_json::from_slice(body).map_err(|e| InboundError::Parse(e.to_string()))
    }

    Ok(match scope {
        scope::ACTION => Inbound::Action(parse(body)?),
        scope::STATUS => Inbound::Status(parse(body)?),
        scope::BATTERY => Inbound::Battery(parse(body)?),
        scope::CONFIGURATION => Inbound::Configuration(parse(body)?),
        scope::NETWORK => Inbound::Network(parse(body)?),
        other => return Err(InboundError::UnknownScope(other.to_owned())),
    })
}

#[cfg(test)]
mod tests;
