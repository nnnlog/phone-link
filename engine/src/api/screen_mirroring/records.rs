//! The records and enums the screen-mirroring object hands the UI, and their conversions from the feature's types.

use crate::features::screen_mirroring::{
    self as sm, AppsFeatureType, AudioButton, PhoneState, ScreenMirroringEvent, Settings, WindowState,
};

/// The phone's mirroring state: the connection, the nearby check, and the phone's lock and screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MirrorPhoneState {
    Disconnected,
    Connecting,
    Connected,
    Error,
    NearbyConnectionNeeded,
    /// "Check your phone": the phone asks its user to allow screen capture.
    ScreenCapturePermissionNeeded,
    FoldedCannotDraw,
    SecureLocked,
    Locked,
    Unlocked,
    ConnectionUnstable,
    ScreenOff,
}

impl From<PhoneState> for MirrorPhoneState {
    fn from(s: PhoneState) -> Self {
        match s {
            PhoneState::Disconnected => Self::Disconnected,
            PhoneState::Connecting => Self::Connecting,
            PhoneState::Connected => Self::Connected,
            PhoneState::Error => Self::Error,
            PhoneState::NearbyConnectionNeeded => Self::NearbyConnectionNeeded,
            PhoneState::ScreenScrapePermissionNeeded => Self::ScreenCapturePermissionNeeded,
            PhoneState::FoldedCannotDraw => Self::FoldedCannotDraw,
            PhoneState::SecureLocked => Self::SecureLocked,
            PhoneState::Locked => Self::Locked,
            PhoneState::Unlocked => Self::Unlocked,
            PhoneState::ConnectionUnstable => Self::ConnectionUnstable,
            PhoneState::ScreenOff => Self::ScreenOff,
        }
    }
}

/// What a window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MirrorWindowState {
    Loading,
    Connected,
    NearbyConnectionNeeded,
    ScreenCapturePermissionNeeded,
    Error,
    /// An error the phone-screen window can dismiss.
    DismissibleError,
    /// The phone asks to move the app to the Mac (`move_to_virtual_display`).
    Restart,
    FoldedCannotDraw,
    /// The phone is locked: Unlock (`wake`).
    Locked,
    /// The phone suspended the app: Resume.
    Suspended,
    /// The phone closed the app: Reopen.
    Closed,
    /// The phone asks for touch confirmation: Continue.
    TrueTouchWarning,
    DeviceOverheat,
    /// A spinner until the connection recovers.
    ConnectionUnstable,
    /// The phone's screen is off: Wake.
    ScreenOff,
    PowerSavingMode,
    AudioPermissionNeeded,
    AudioPermissionAlwaysDenied,
    SecureContent,
}

impl From<WindowState> for MirrorWindowState {
    fn from(s: WindowState) -> Self {
        use WindowState as W;
        match s {
            W::Loading => Self::Loading,
            W::Connected => Self::Connected,
            W::NearbyConnectionNeeded => Self::NearbyConnectionNeeded,
            W::ScreenScrapePermissionNeeded => Self::ScreenCapturePermissionNeeded,
            W::Error => Self::Error,
            W::DismissibleError => Self::DismissibleError,
            W::Restart => Self::Restart,
            W::FoldedCannotDraw => Self::FoldedCannotDraw,
            W::Locked => Self::Locked,
            W::Suspended => Self::Suspended,
            W::Closed => Self::Closed,
            W::TrueTouchWarning => Self::TrueTouchWarning,
            W::DeviceOverheat => Self::DeviceOverheat,
            W::ConnectionUnstable => Self::ConnectionUnstable,
            W::ScreenOff => Self::ScreenOff,
            W::PowerSavingMode => Self::PowerSavingMode,
            W::AudioStreamingPermissionNeeded => Self::AudioPermissionNeeded,
            W::AudioStreamingPermissionAlwaysDenied => Self::AudioPermissionAlwaysDenied,
            W::SecureContent => Self::SecureContent,
        }
    }
}

/// How phone apps open on this Mac: not at all, in the phone-screen window, or each in a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MirrorAppsFeatureType {
    Disabled,
    PhoneScreenOnly,
    /// One window per app.
    MultipleApps,
}

impl From<AppsFeatureType> for MirrorAppsFeatureType {
    fn from(t: AppsFeatureType) -> Self {
        match t {
            AppsFeatureType::Disabled => Self::Disabled,
            AppsFeatureType::PhoneScreenOnly => Self::PhoneScreenOnly,
            AppsFeatureType::MultipleApps => Self::MultipleApps,
        }
    }
}

/// The kind of a mirroring error, which picks the text the error window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MirrorErrorKind {
    AppNotAvailable,
    Overheated,
    PowerSaving,
    LaunchFailed,
    PermissionOff,
    Disconnected,
    TimedOut,
    CannotConnectDirectly,
    PhoneUnreachable,
    MeteredConnection,
    FeaturesDisabled,
    UpdateNeeded,
    ServiceIssue,
    Generic,
}

/// The error window's button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MirrorErrorAction {
    /// Close the window (or, in the phone screen, dismiss).
    Close,
    /// `retry`.
    Retry,
    /// `retry`, labelled Connect.
    Connect,
    /// Turn "allow apps" on (`set_settings`), then `retry`.
    AllowAndRetry,
    /// Launch the window's app again (`reopen`).
    Relaunch,
}

/// An error a window or the phone state shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct MirrorError {
    pub code: i32,
    pub kind: MirrorErrorKind,
    pub action: MirrorErrorAction,
}

/// The phone's mirroring settings on this Mac.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct MirrorSettings {
    pub allow_apps: bool,
    pub black_screen: bool,
    pub audio_streaming: bool,
    pub always_expanded: bool,
    pub do_not_ask_restart: bool,
    pub do_not_ask_task_transfer: bool,
    pub do_not_show_true_touch: bool,
    pub do_not_ask_skip_unlock: bool,
    pub do_not_ask_skip_verification: bool,
    pub do_not_ask_switch_layout: bool,
}

impl From<Settings> for MirrorSettings {
    fn from(s: Settings) -> Self {
        Self {
            allow_apps: s.allow_apps,
            black_screen: s.black_screen,
            audio_streaming: s.audio_streaming,
            always_expanded: s.always_expanded,
            do_not_ask_restart: s.do_not_ask_restart,
            do_not_ask_task_transfer: s.do_not_ask_task_transfer,
            do_not_show_true_touch: s.do_not_show_true_touch,
            do_not_ask_skip_unlock: s.do_not_ask_skip_unlock,
            do_not_ask_skip_verification: s.do_not_ask_skip_verification,
            do_not_ask_switch_layout: s.do_not_ask_switch_layout,
        }
    }
}

/// The nearby check's prompt.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MirrorNearbyPrompt {
    pub device_id: Option<String>,
    /// The code the Mac shows for the phone to scan or type.
    pub expiry_token: i32,
    pub token_expired: bool,
    pub pin_code_wrong: bool,
    pub skip_unlock: bool,
    pub skip_verification: bool,
    pub china_market: bool,
}

/// The selected phone's mirroring.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MirroringStatusView {
    /// Mirroring is offered at all.
    pub remoting_supported: bool,
    /// The phone screen and app launches.
    pub phone_screen_apps: bool,
    pub apps_feature_type: MirrorAppsFeatureType,
    /// Mouse, touch and keys reach the phone.
    pub input_supported: bool,
    pub audio_supported: bool,
    pub adaptive_layout: bool,
    pub phone_state: MirrorPhoneState,
    /// With phone state `Error`.
    pub phone_error: Option<MirrorError>,
    pub nearby: Option<MirrorNearbyPrompt>,
    pub settings: MirrorSettings,
    pub permission_accepted: bool,
}

/// The one audio button a window's title bar shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MirrorAudioButton {
    None,
    /// *Enable audio*: the window's app audio is refused on the phone; a click is `enable_audio`.
    EnableAudio,
    /// The volume control (`volume`, `muted`; changes go to `set_volume`).
    Volume,
}

impl From<AudioButton> for MirrorAudioButton {
    fn from(b: AudioButton) -> Self {
        match b {
            AudioButton::None => Self::None,
            AudioButton::EnableAudio => Self::EnableAudio,
            AudioButton::Volume => Self::Volume,
        }
    }
}

/// A width and height in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct MirrorSize {
    pub width: u32,
    pub height: u32,
}

/// A rectangle inside the device resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct MirrorRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// One window.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MirrorSessionView {
    pub id: String,
    pub package_name: String,
    pub phone_screen: bool,
    pub phone_initiated: bool,
    pub state: MirrorWindowState,
    /// With state `Error` or `DismissibleError`.
    pub error: Option<MirrorError>,
    /// With state `Locked`: the phone has a secure lock.
    pub secure_locked: bool,
    /// With state `Locked` or `ScreenOff`: the phone can unlock from the Mac.
    pub unlock_from_mac: bool,
    /// With state `Restart`: the move is a task transfer.
    pub restart_task_transfer: bool,
    /// With state `Restart`: the user asked for another layout (`confirm_layout_switch`).
    pub restart_layout_switch: bool,
    pub launched: bool,
    pub video_ready: bool,
    /// The phone's current device resolution (input coordinates are scaled to it).
    pub device_resolution: Option<MirrorSize>,
    /// The phone record's device resolution the window started with: the window's shape until a
    /// video size is known.
    pub initial_device_resolution: Option<MirrorSize>,
    /// `orientationInfo` (0–3).
    pub orientation: i32,
    /// Present the video rotated by this many degrees (0, −90, −180, −270).
    pub rotation: i32,
    pub video_size: Option<MirrorSize>,
    pub content_area: Option<MirrorRect>,
    pub navigation_buttons: bool,
    pub button_order: Option<String>,
    pub recent_task_id: i64,
    pub screen_layout: Option<i32>,
    /// The phone asks to move the app here: `true` for a task transfer.
    pub move_to_virtual_display: Option<bool>,
    pub volume: i32,
    pub muted: bool,
    pub silent: bool,
    /// The window's app lets the phone capture its audio, as the phone last said while the window
    /// was open (`true` until it says); a change to `false` is shown in the window for a moment.
    pub audio_capture_allowed: bool,
    /// The audio button to show in the title bar (it already follows the window's state).
    pub audio_button: MirrorAudioButton,
}

/// A window's focus: focused, not focused, or minimized or hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MirrorFocus {
    InFocus,
    NotInFocus,
    /// Minimized or hidden.
    Suspended,
}

/// A Mac mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MirrorMouseButton {
    Left,
    Middle,
    Right,
    /// Any further button: it moves the pointer but presses nothing on the phone.
    Other,
}

/// Where a touch contact is: down, moved, up, or cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MirrorTouchPhase {
    Down,
    Moved,
    Up,
    Cancelled,
}

/// A Mac key going down, up, or repeating while held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MirrorKeyAction {
    Down,
    Up,
    /// An auto-repeat of a held key.
    Repeat,
}

/// A navigation button of a window: Back, Home or Recents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MirrorNavButton {
    Back,
    Home,
    Recents,
}

/// What a Mac key sends to the phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MirrorKeyMapping {
    /// This HID usage, down and up with the key.
    Usage {
        usage: u8,
    },
    /// A press and release of this usage when the key goes down.
    Tap {
        usage: u8,
    },
    NotSent,
}

/// A change to mirroring: re-read the status or a window, or open, close or raise a window.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ScreenMirroringChange {
    /// Re-read `status`.
    Changed,
    /// Open a window for this session.
    SessionOpened { session_id: String, package_name: String, phone_initiated: bool },
    /// Re-read this session.
    SessionChanged { session_id: String },
    /// Close this session's window.
    SessionClosed { session_id: String },
    /// Raise this session's window.
    BringToFront { session_id: String },
    /// A notification launch finished.
    NotificationLaunched { key: String, opened: bool },
    /// The phone app took none of the files dragged onto this window: show the *not accepted* notice.
    DragNotAccepted { session_id: String },
    /// This window's drag of phone files ended (completed or cancelled).
    DragEnded { session_id: String },
    /// Bring the main window forward (a launch the phone asked for that opens no window of its own).
    ShowMainWindow,
    /// Show the main window's Apps section (a launch the phone asked for that the phone's apps
    /// answer).
    ShowApps,
}

impl From<&ScreenMirroringEvent> for ScreenMirroringChange {
    fn from(e: &ScreenMirroringEvent) -> Self {
        match e.clone() {
            ScreenMirroringEvent::Changed => Self::Changed,
            ScreenMirroringEvent::SessionOpened { session_id, package_name, phone_initiated } => {
                Self::SessionOpened { session_id, package_name, phone_initiated }
            }
            ScreenMirroringEvent::SessionChanged { session_id } => Self::SessionChanged { session_id },
            ScreenMirroringEvent::SessionClosed { session_id } => Self::SessionClosed { session_id },
            ScreenMirroringEvent::BringToFront { session_id } => Self::BringToFront { session_id },
            ScreenMirroringEvent::NotificationLaunched { key, opened } => Self::NotificationLaunched { key, opened },
            ScreenMirroringEvent::DragNotAccepted { session_id } => Self::DragNotAccepted { session_id },
            ScreenMirroringEvent::DragEnded { session_id } => Self::DragEnded { session_id },
            ScreenMirroringEvent::ShowMainWindow => Self::ShowMainWindow,
            ScreenMirroringEvent::ShowApps => Self::ShowApps,
        }
    }
}

/// A file dragged from the Mac onto a window.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MirrorDragFile {
    pub path: String,
    /// The MIME type of its extension, when the Mac knows one.
    pub mime_type: Option<String>,
}

/// A window's answer to a drag of Mac files entering it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MirrorDragEnter {
    /// Copy: the files go to the phone app the drop lands on.
    Accepted,
    /// Nothing to send, or the window cannot take files now.
    Refused,
    /// Too many files or a file too large: dropping it shows the over-limit notice.
    OverLimit,
}

impl From<sm::drag::EnterOutcome> for MirrorDragEnter {
    fn from(o: sm::drag::EnterOutcome) -> Self {
        match o {
            sm::drag::EnterOutcome::Accepted => Self::Accepted,
            sm::drag::EnterOutcome::Refused => Self::Refused,
            sm::drag::EnterOutcome::OverLimit => Self::OverLimit,
        }
    }
}

/// One file the phone drags out of a window: a file promise of the Mac drag.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MirrorPhoneDragItem {
    /// What `write_phone_drag_item` names it by.
    pub data_id: String,
    /// The file's name on the Mac.
    pub name: String,
    pub size: u64,
    /// The phone's MIME type for it (may be empty).
    pub mime_type: String,
}

impl From<sm::drag::PhoneDragItem> for MirrorPhoneDragItem {
    fn from(i: sm::drag::PhoneDragItem) -> Self {
        Self { data_id: i.data_id, name: i.name, size: i.size, mime_type: i.mime_type }
    }
}
