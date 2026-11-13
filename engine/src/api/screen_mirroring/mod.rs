//! Screen mirroring for the UI ([`crate::features::screen_mirroring`]).
//!
//! It covers the selected phone's mirroring state and settings, its windows (one macOS window per
//! session), the windows' actions and input, and the Mac keyboard's mapping to the keys the phone
//! receives.
//!
//! A window opens on [`ScreenMirroringChange::SessionOpened`] — for a window the UI asked for, too —
//! and closes on [`ScreenMirroringChange::SessionClosed`]; the UI reports the window ready to present
//! (`window_ready`) and its focus (`focus_changed`), and closing it (`close_session`).

use std::sync::Arc;

use crate::api::engine::Engine;

mod records;
use crate::features::screen_mirroring::{
    self as sm, Details, ErrorAction, ErrorKind, FocusableState, InputSourceKind, KeyAction, KeyMapping,
    MirroringError, MouseButton, NavButton, PhoneState, ScreenMirroring, Settings, TouchPhase, WindowState,
    errors::error_kind,
};
pub use records::{
    MirrorAppsFeatureType, MirrorAudioButton, MirrorDragEnter, MirrorDragFile, MirrorError, MirrorErrorAction,
    MirrorErrorKind, MirrorFocus, MirrorInputSource, MirrorKeyAction, MirrorKeyMapping, MirrorMouseButton,
    MirrorNavButton, MirrorNearbyPrompt, MirrorPhoneDragItem, MirrorPhoneState, MirrorRect, MirrorSessionView,
    MirrorSettings, MirrorSize, MirrorTouchPhase, MirrorWindowState, MirroringStatusView, ScreenMirroringChange,
};

use super::{FeatureApiError, core_of, features_of};

impl From<MirroringError> for FeatureApiError {
    fn from(e: MirroringError) -> Self {
        match e {
            MirroringError::NoPhone => FeatureApiError::NoPhone,
            MirroringError::UnknownSession(_) => FeatureApiError::UnknownItem { what: "mirroring session".into() },
            MirroringError::Store(e) => e.into(),
            other => FeatureApiError::NotAvailable { reason: other.to_string() },
        }
    }
}

fn error(code: i32) -> MirrorError {
    let (kind, action) = error_kind(code);
    MirrorError {
        code,
        kind: match kind {
            ErrorKind::AppNotAvailable => MirrorErrorKind::AppNotAvailable,
            ErrorKind::Overheated => MirrorErrorKind::Overheated,
            ErrorKind::PowerSaving => MirrorErrorKind::PowerSaving,
            ErrorKind::LaunchFailed => MirrorErrorKind::LaunchFailed,
            ErrorKind::PermissionOff => MirrorErrorKind::PermissionOff,
            ErrorKind::Disconnected => MirrorErrorKind::Disconnected,
            ErrorKind::TimedOut => MirrorErrorKind::TimedOut,
            ErrorKind::CannotConnectDirectly => MirrorErrorKind::CannotConnectDirectly,
            ErrorKind::PhoneUnreachable => MirrorErrorKind::PhoneUnreachable,
            ErrorKind::MeteredConnection => MirrorErrorKind::MeteredConnection,
            ErrorKind::FeaturesDisabled => MirrorErrorKind::FeaturesDisabled,
            ErrorKind::UpdateNeeded => MirrorErrorKind::UpdateNeeded,
            ErrorKind::ServiceIssue => MirrorErrorKind::ServiceIssue,
            ErrorKind::Generic => MirrorErrorKind::Generic,
        },
        action: match action {
            ErrorAction::Close => MirrorErrorAction::Close,
            ErrorAction::Retry => MirrorErrorAction::Retry,
            ErrorAction::Connect => MirrorErrorAction::Connect,
            ErrorAction::AllowAndRetry => MirrorErrorAction::AllowAndRetry,
            ErrorAction::Relaunch => MirrorErrorAction::Relaunch,
        },
    }
}

impl From<MirrorSettings> for Settings {
    fn from(s: MirrorSettings) -> Self {
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

fn size(s: Option<(u32, u32)>) -> Option<MirrorSize> {
    s.map(|(width, height)| MirrorSize { width, height })
}

fn session_view(s: sm::SessionView) -> MirrorSessionView {
    let (error_code, secure, vd, task_transfer) = match &s.details {
        Details::Error { code } => (Some(*code), false, false, false),
        Details::Locked { secure, virtual_device_streaming } => (None, *secure, *virtual_device_streaming, false),
        Details::Restart { task_transfer } => (None, false, false, *task_transfer),
        _ => (None, false, false, false),
    };
    let layout_switch = s.details == Details::LayoutSwitch;
    let shows_error = matches!(s.state, WindowState::Error | WindowState::DismissibleError);

    MirrorSessionView {
        id: s.id,
        package_name: s.package_name,
        phone_screen: s.phone_screen,
        phone_initiated: s.phone_initiated,
        state: s.state.into(),
        error: shows_error.then(|| error(error_code.unwrap_or(0))),
        secure_locked: secure,
        unlock_from_mac: vd,
        restart_task_transfer: task_transfer,
        restart_layout_switch: layout_switch,
        launched: s.launched,
        video_ready: s.video_ready,
        device_resolution: size(s.device_resolution),
        initial_device_resolution: size(s.initial_device_resolution),
        orientation: s.orientation,
        rotation: s.rotation,
        video_size: size(s.video_size),
        content_area: s.content_area.map(|(left, top, right, bottom)| MirrorRect { left, top, right, bottom }),
        navigation_buttons: s.navigation_buttons,
        button_order: s.button_order,
        recent_task_id: s.recent_task_id,
        screen_layout: s.screen_layout,
        last_screen_layout: s.last_screen_layout,
        move_to_virtual_display: s.move_to_virtual_display,
        volume: s.audio.volume,
        muted: s.audio.muted,
        silent: s.audio.silent,
        audio_capture_allowed: s.audio_capture_allowed,
        audio_button: s.audio_button.into(),
    }
}

impl From<MirrorFocus> for FocusableState {
    fn from(f: MirrorFocus) -> Self {
        match f {
            MirrorFocus::InFocus => Self::InFocus,
            MirrorFocus::NotInFocus => Self::NotInFocus,
            MirrorFocus::Suspended => Self::Suspended,
        }
    }
}

impl From<MirrorInputSource> for InputSourceKind {
    fn from(s: MirrorInputSource) -> Self {
        match s {
            MirrorInputSource::Layout => Self::Layout,
            MirrorInputSource::InputMethod => Self::InputMethod,
        }
    }
}

impl From<MirrorMouseButton> for MouseButton {
    fn from(b: MirrorMouseButton) -> Self {
        match b {
            MirrorMouseButton::Left => Self::Left,
            MirrorMouseButton::Middle => Self::Middle,
            MirrorMouseButton::Right => Self::Right,
            MirrorMouseButton::Other => Self::Other,
        }
    }
}

impl From<MirrorTouchPhase> for TouchPhase {
    fn from(p: MirrorTouchPhase) -> Self {
        match p {
            MirrorTouchPhase::Down => Self::Down,
            MirrorTouchPhase::Moved => Self::Moved,
            MirrorTouchPhase::Up => Self::Up,
            MirrorTouchPhase::Cancelled => Self::Cancelled,
        }
    }
}

impl From<MirrorKeyAction> for KeyAction {
    fn from(a: MirrorKeyAction) -> Self {
        match a {
            MirrorKeyAction::Down => Self::Down,
            MirrorKeyAction::Up => Self::Up,
            MirrorKeyAction::Repeat => Self::Repeat,
        }
    }
}

impl From<MirrorNavButton> for NavButton {
    fn from(b: MirrorNavButton) -> Self {
        match b {
            MirrorNavButton::Back => Self::Back,
            MirrorNavButton::Home => Self::Home,
            MirrorNavButton::Recents => Self::Recents,
        }
    }
}

/// The HID usage the phone receives for the Mac virtual key code `key_code` (`kVK_*`), if any.
#[uniffi::export]
pub fn mirror_mac_key(key_code: u16) -> MirrorKeyMapping {
    match sm::mac_key(key_code) {
        KeyMapping::Usage(usage) => MirrorKeyMapping::Usage { usage },
        KeyMapping::Tap(usage) => MirrorKeyMapping::Tap { usage },
        KeyMapping::NotSent => MirrorKeyMapping::NotSent,
    }
}

/// Screen mirroring of the selected phone.
#[derive(uniffi::Object)]
pub struct ScreenMirroringApi {
    engine: Arc<Engine>,
}

impl ScreenMirroringApi {
    pub(crate) fn new(engine: Arc<Engine>) -> Arc<Self> {
        Arc::new(Self { engine })
    }

    fn feature(&self) -> Result<Arc<ScreenMirroring>, FeatureApiError> {
        let core = core_of(&self.engine)?;
        Ok(features_of(&core)?.screen_mirroring.clone())
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl ScreenMirroringApi {
    /// The selected phone's mirroring: what it offers, its state, the nearby prompt, the settings.
    ///
    /// Fails with `NoPhone` when no phone is selected.
    pub fn status(&self) -> Result<MirroringStatusView, FeatureApiError> {
        let s = self.feature()?.status()?;
        Ok(MirroringStatusView {
            remoting_supported: s.remoting_supported,
            phone_screen_apps: s.phone_screen_apps,
            apps_feature_type: s.apps_feature_type.into(),
            input_supported: s.values.input_injector,
            audio_supported: s.values.audio(),
            adaptive_layout: s.values.adaptive_layout(),
            phone_state: s.phone_state.into(),
            phone_error: (s.phone_state == PhoneState::Error).then(|| error(s.phone_error)),
            nearby: s.nearby.map(|n| MirrorNearbyPrompt {
                device_id: n.device_id,
                expiry_token: n.expiry_token,
                token_expired: n.token_expired,
                pin_code_wrong: n.pin_code_wrong,
                skip_unlock: n.skip_unlock,
                skip_verification: n.skip_verification,
                china_market: n.china_market,
            }),
            settings: s.settings.into(),
            permission_accepted: s.permission_accepted,
        })
    }

    /// The windows, in the order they opened.
    pub fn sessions(&self) -> Result<Vec<MirrorSessionView>, FeatureApiError> {
        Ok(self.feature()?.sessions().into_iter().map(session_view).collect())
    }

    /// The window `session_id`; `None` when no open window has that id.
    pub fn session(&self, session_id: String) -> Result<Option<MirrorSessionView>, FeatureApiError> {
        Ok(self.feature()?.session(&session_id).map(session_view))
    }

    /// Opens (or raises) the phone-screen window; its session id.
    pub fn open_phone_screen(&self) -> Result<String, FeatureApiError> {
        Ok(self.feature()?.open_phone_screen()?)
    }

    /// Opens phone app `package_name` (a task transfer of `transfer_task_id` when set); the session id
    /// of the window it shows in.
    pub fn launch_app(&self, package_name: String, transfer_task_id: Option<i64>) -> Result<String, FeatureApiError> {
        Ok(self.feature()?.launch_app(&package_name, transfer_task_id)?)
    }

    /// Opens notification `key` on the phone; `action_index` −1 for its body.
    pub async fn launch_notification(&self, key: String, action_index: i32) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.launch_notification(&key, action_index).await?)
    }

    /// The window's keyboard-settings (language) button; `input_source` is the kind of the Mac's
    /// current keyboard input source, read on the main thread when the button is pressed.
    pub async fn request_keyboard_settings(&self, input_source: MirrorInputSource) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.request_keyboard_settings(input_source.into()).await?)
    }

    /// The user closed window `session_id`; after the last window, mirroring stops.
    ///
    /// Fails with `UnknownItem` when no open window has that id.
    pub fn close_session(&self, session_id: String) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.close_session(&session_id)?)
    }

    /// The window can present video now.
    pub fn window_ready(&self, session_id: String) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.window_ready(&session_id)?)
    }

    /// The window's focus changed; `caps_lock` = the Mac's Caps Lock state now.
    pub fn focus_changed(
        &self,
        session_id: String,
        focus: MirrorFocus,
        caps_lock: bool,
    ) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.focus_changed(&session_id, focus.into(), caps_lock)?)
    }

    /// Resume a suspended window: `true` when the phone could resume it.
    pub async fn resume(&self, session_id: String) -> Result<bool, FeatureApiError> {
        Ok(self.feature()?.resume(&session_id).await?)
    }

    /// Reopen a closed window's app.
    pub fn reopen(&self, session_id: String) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.reopen(&session_id)?)
    }

    /// An error window's Retry / Connect: the set-up again for every window.
    pub fn retry(&self) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.retry()?)
    }

    /// The touch-confirmation warning's Continue: the window's app launches again past the warning.
    pub fn continue_true_touch(&self, session_id: String) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.continue_true_touch(&session_id)?)
    }

    /// Dismisses the error window `session_id` shows; the window returns to its connected state.
    ///
    /// Fails with `UnknownItem` when no open window has that id.
    pub fn dismiss_error(&self, session_id: String) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.dismiss_error(&session_id)?)
    }

    /// Unlock / Wake.
    pub fn wake(&self) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.wake()?)
    }

    /// "Resend" the screen-capture prompt.
    pub fn resend_permission(&self) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.resend_permission()?)
    }

    /// Answers the phone's request to move window `session_id`'s app to this Mac.
    ///
    /// `do_not_ask` accepts later requests of the same kind without asking. Fails with
    /// `NotAvailable` when the phone has asked nothing.
    pub fn answer_move_to_virtual_display(
        &self,
        session_id: String,
        accept: bool,
        do_not_ask: bool,
    ) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.answer_move_to_virtual_display(&session_id, accept, do_not_ask)?)
    }

    /// The audio-permission window's buttons: ask on the phone, or open the phone's settings.
    pub async fn request_audio_permission(&self, open_settings: bool) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.request_audio_permission(open_settings).await?)
    }

    /// The title bar's *enable audio* button of window `session_id`: the phone is asked again; the
    /// window then shows *audio permission needed*, *audio always denied*, or its connected state
    /// (with or without the button).
    pub async fn enable_audio(&self, session_id: String) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.enable_audio(&session_id).await?)
    }

    /// The audio-permission window's "continue without audio".
    pub fn continue_without_audio(&self, session_id: String) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.continue_without_audio(&session_id)?)
    }

    /// Saves the selected phone's mirroring settings; a change the phone acts on is sent at once.
    ///
    /// Turning `allow_apps` off closes every window.
    pub fn set_settings(&self, settings: MirrorSettings) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.set_settings(settings.into())?)
    }

    /// App `package_name`'s volume (0–100) and mute.
    pub fn set_volume(&self, package_name: String, volume: i32, muted: bool) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.set_volume(&package_name, volume, muted)?)
    }

    /// Ask the phone for screen layout `layout` (1 Compact, 2 Expanded, …) for an app window.
    pub fn switch_layout(&self, session_id: String, layout: i32) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.switch_layout(&session_id, layout)?)
    }

    /// The layout-switch confirmation; `do_not_ask` = do not ask again.
    pub fn confirm_layout_switch(&self, session_id: String, do_not_ask: bool) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.confirm_layout_switch(&session_id, do_not_ask)?)
    }

    /// The Mac's audio output devices changed.
    pub fn audio_output_changed(&self) -> Result<(), FeatureApiError> {
        self.feature()?.audio_output_changed();
        Ok(())
    }

    /// The nearby prompt's PIN option: asks the phone to show a code the user types on this Mac.
    ///
    /// Like the other nearby calls, it fails with `NotAvailable` while no nearby check runs.
    pub fn nearby_request_pin_code(&self) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.nearby_request_pin_code()?)
    }

    /// Sends the code the user read on the phone; a wrong one shows `pin_code_wrong`.
    pub fn nearby_send_pin_code(&self, pin_code: String) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.nearby_send_pin_code(&pin_code)?)
    }

    /// The nearby prompt's skip-unlock button: asks the phone to go on without being unlocked.
    pub fn nearby_skip_unlock(&self) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.nearby_skip_unlock()?)
    }

    /// The nearby prompt's skip-verification button: asks the phone to go on without the code.
    pub fn nearby_skip_verification(&self) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.nearby_skip_verification()?)
    }

    /// The nearby prompt's code was shown again: its expiry restarts.
    pub fn nearby_code_refreshed(&self) -> Result<(), FeatureApiError> {
        self.feature()?.nearby_code_refreshed();
        Ok(())
    }

    /// A drag of Mac `files` entered window `session_id`'s video view.
    pub fn drag_enter(
        &self,
        session_id: String,
        files: Vec<MirrorDragFile>,
    ) -> Result<MirrorDragEnter, FeatureApiError> {
        let files =
            files.into_iter().map(|f| sm::drag::MacDragFile { path: f.path.into(), mime_type: f.mime_type }).collect();
        Ok(self.feature()?.drag_enter(&session_id, files)?.into())
    }

    /// The drag is over the window at (`x`, `y`) normalized to the video: `true` while it shows copy.
    pub fn drag_over(&self, session_id: String, x: f64, y: f64) -> Result<bool, FeatureApiError> {
        Ok(self.feature()?.drag_over(&session_id, x, y)?)
    }

    /// The drag left the window, or was cancelled over it.
    pub fn drag_leave(&self, session_id: String) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.drag_leave(&session_id)?)
    }

    /// The drag was dropped on the window.
    pub fn drag_drop(&self, session_id: String) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.drag_drop(&session_id)?)
    }

    /// Called on every pointer move in the window: the phone's files to start a Mac drag of now, when
    /// the phone handed a drag over (`button_held` = a mouse button is down; without one the drag
    /// ends at once and nothing is returned).
    pub fn take_phone_drag(
        &self,
        session_id: String,
        button_held: bool,
    ) -> Result<Option<Vec<MirrorPhoneDragItem>>, FeatureApiError> {
        let items = self.feature()?.take_phone_drag(&session_id, button_held)?;
        Ok(items.map(|v| v.into_iter().map(Into::into).collect()))
    }

    /// The Mac drag of the phone's files ended: `dropped` when a destination took it.
    pub fn phone_drag_ended(&self, session_id: String, dropped: bool) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.phone_drag_ended(&session_id, dropped)?)
    }

    /// A destination asks for phone file `data_id` at `path`: waits for it to arrive, then puts it
    /// there.
    pub async fn write_phone_drag_item(
        &self,
        session_id: String,
        data_id: String,
        path: String,
    ) -> Result<(), FeatureApiError> {
        let feature = self.feature()?;
        Ok(feature.write_phone_drag_item(&session_id, &data_id, std::path::Path::new(&path)).await?)
    }

    /// The pointer at (`x`, `y`) normalized to the window's video rectangle (0–1).
    pub fn mouse_move(&self, session_id: String, x: f64, y: f64) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.mouse_move(&session_id, x, y)?)
    }

    /// A Mac mouse button went down or up at (`x`, `y`), normalized to the window's video (0–1).
    ///
    /// Of buttons held together, only the first one pressed reaches the phone; the others move the
    /// pointer only.
    pub fn mouse_button(
        &self,
        session_id: String,
        button: MirrorMouseButton,
        down: bool,
        x: f64,
        y: f64,
    ) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.mouse_button(&session_id, button.into(), down, x, y)?)
    }

    /// One scroll at (`x`, `y`) normalized to the video: wheel deltas in Windows units, ±120 per notch,
    /// positive away from the user / to the right; precise scrolling scaled into these units.
    pub fn mouse_wheel(
        &self,
        session_id: String,
        x: f64,
        y: f64,
        delta_x: i32,
        delta_y: i32,
    ) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.mouse_wheel(&session_id, x, y, delta_x, delta_y)?)
    }

    /// A direct-touch contact: `pointer` identifies it while it lasts; pressure 0–1, orientation in
    /// degrees, contact size in display points.
    #[expect(clippy::too_many_arguments, reason = "independent inputs of one operation; not yet grouped")]
    pub fn touch(
        &self,
        session_id: String,
        pointer: u64,
        phase: MirrorTouchPhase,
        x: f64,
        y: f64,
        pressure: f64,
        orientation: f64,
        width: f64,
        height: f64,
    ) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.touch(&session_id, pointer, phase.into(), x, y, pressure, orientation, width, height)?)
    }

    /// A tablet pen: pressure 0–1, rotation in degrees.
    #[expect(clippy::too_many_arguments, reason = "independent inputs of one operation; not yet grouped")]
    pub fn pen(
        &self,
        session_id: String,
        x: f64,
        y: f64,
        pressure: f64,
        rotation: f64,
        in_range: bool,
        in_contact: bool,
    ) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.pen(&session_id, x, y, pressure, rotation, in_range, in_contact)?)
    }

    /// The window started (`true`) or stopped taking input to the phone (its video shows and the phone
    /// takes input): the input focus is taken anew, or lost with everything held released.
    pub fn input_focus(&self, session_id: String, focused: bool, caps_lock: bool) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.input_focus(&session_id, focused, caps_lock)?)
    }

    /// A Mac key (`kVK_*`); Caps Lock as a down and an up on each of its toggles. `flags` are the
    /// event's modifier flags (`NSEvent.ModifierFlags.rawValue`, after the event).
    pub fn key(
        &self,
        session_id: String,
        key_code: u16,
        action: MirrorKeyAction,
        flags: u64,
    ) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.key(&session_id, key_code, action.into(), flags)?)
    }

    /// The window's Back, Home or Recents button, pressed on the phone while connected.
    pub fn nav_button(&self, session_id: String, button: MirrorNavButton) -> Result<(), FeatureApiError> {
        Ok(self.feature()?.nav_button(&session_id, button.into())?)
    }
}
