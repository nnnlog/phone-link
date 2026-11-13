//! Screen mirroring: the phone's screen (the phone-screen window) or one phone app per window on the
//! Mac, with the phone's video and app audio, and the Mac's mouse, touch, pen and keyboard sent back.
//!
//! It runs on its own Nano connection, opened with the App layer's `/nanohandshake`; the phone
//! creates every channel, and **workflows** started on its `launcher` channel drive the session.
//!
//! | Module | Implements |
//! |---|---|
//! | [`gates`] | the phone's values from its record and the gates derived from them |
//! | [`store`] | the per-phone settings, the per-package layout and volume |
//! | [`wire`] | the scopes and the JSON bodies both ways |
//! | [`link`] | the connection, its `launcher` channel, workflow start and stop, idle release, loss |
//! | [`machine`] | the phone state and the window state, with their tables and rules |
//! | [`session`] | one window: its channel set, display state, and the grouping of the phone's channels |
//! | [`input`], [`keymap`] | the window's input to the phone, and the Mac keys as HID usages |
//! | [`drag`] | drag and drop of files between the Mac and a window, both ways |
//! | [`errors`] | status codes and what an error window shows |
//! | [`edge`] | the Mac's display values, audio output and input source; video and audio presentation |
//! | `runtime` | the set-up, the workflows' messages, the windows' lifecycle and teardown |
//!
//! Only the path where mirroring opens its own connection (the "legacy" path) is implemented; the
//! path over the device connection's platform channels is not.

#![cfg_attr(feature = "test-support", warn(unreachable_pub))]

uniffi::setup_scaffolding!();

pub mod drag;
pub mod edge;
pub mod errors;
pub mod gates;
mod handshake;
pub mod input;
pub mod keymap;
pub mod link;
pub mod machine;
pub mod probe;
mod runtime;
pub mod session;
pub mod store;
pub mod wire;

/// Why an operation could not run.
#[derive(Debug, thiserror::Error)]
pub enum MirroringError {
    #[error("no phone is selected")]
    NoPhone,
    #[error("the phone does not mirror: {0}")]
    NotSupported(&'static str),
    #[error("no session {0}")]
    UnknownSession(String),
    #[error("not now: {0}")]
    NotNow(&'static str),
    #[error("the phone answered with status {0}")]
    Status(i32),
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("not sent: {0}")]
    NotSent(#[from] ChannelError),
    #[error("phone drag file not written: {0}")]
    DragWrite(#[from] drag::DragWriteError),
}
/// The nearby check's prompt (phone state *nearby connection needed*).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NearbyPrompt {
    pub device_id: Option<String>,
    pub expiry_token: i32,
    pub token_expired: bool,
    pub pin_code_wrong: bool,
    /// Offer "skip unlocking" (the phone is on the Mac's network).
    pub skip_unlock: bool,
    /// Offer "continue without verifying" (verified).
    pub skip_verification: bool,
    pub china_market: bool,
}
/// The mirroring state of the selected phone.
#[derive(Debug, Clone, PartialEq)]
pub struct MirroringStatus {
    pub values: PhoneValues,
    pub remoting_supported: bool,
    pub phone_screen_apps: bool,
    pub apps_feature_type: AppsFeatureType,
    pub phone_state: PhoneState,
    /// The phone state's error code (phone state `Error`).
    pub phone_error: i32,
    pub nearby: Option<NearbyPrompt>,
    pub settings: Settings,
    /// The phone accepted the screen-capture consent in this set-up.
    pub permission_accepted: bool,
}
/// One window, as the UI shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionView {
    pub id: String,
    pub package_name: String,
    pub phone_screen: bool,
    pub phone_initiated: bool,
    pub state: WindowState,
    pub details: Details,
    pub launched: bool,
    pub video_ready: bool,
    pub device_resolution: Option<(u32, u32)>,
    /// The phone record's device resolution the window started with.
    pub initial_device_resolution: Option<(u32, u32)>,
    pub orientation: i32,
    pub rotation: i32,
    pub video_size: Option<(u32, u32)>,
    pub content_area: Option<(i32, i32, i32, i32)>,
    pub navigation_buttons: bool,
    pub button_order: Option<String>,
    pub recent_task_id: i64,
    pub screen_layout: Option<i32>,
    /// The app's last screen layout on this Mac, kept per package from the phone's layout statuses
    /// (`0` for the phone screen, which keeps none).
    pub last_screen_layout: i32,
    /// The phone asks to move the app here (a task transfer when `true`).
    pub move_to_virtual_display: Option<bool>,
    pub audio: PackageAudio,
    /// The window's app lets its audio be captured, as the phone last said while the window was open
    /// (`true` until it says).
    pub audio_capture_allowed: bool,
    /// The audio button the title bar shows.
    pub audio_button: AudioButton,
}
/// The audio button of a window's title bar: one at a time, the last status given wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioButton {
    #[default]
    None,
    /// The window's audio permission ended refused for good (`disabled`): its click is
    /// [`ScreenMirroring::enable_audio`]. Shown only while the window is connected.
    EnableAudio,
    /// The app plays sound, or is muted: the volume control.
    Volume,
}
/// An app's audio on the Mac.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackageAudio {
    pub volume: i32,
    pub muted: bool,
    pub silent: bool,
}
impl Default for PackageAudio {
    fn default() -> Self {
        // Silent until the phone reports the app's sound: the window offers no volume control before.
        Self { volume: 50, muted: false, silent: true }
    }
}
/// What the UI hears.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenMirroringEvent {
    /// Re-read the status: phone state, settings, nearby prompt.
    Changed,
    /// Open a window for this session.
    SessionOpened { session_id: String, package_name: String, phone_initiated: bool },
    /// Re-read this session.
    SessionChanged { session_id: String },
    /// Close this session's window.
    SessionClosed { session_id: String },
    /// Raise this session's window.
    BringToFront { session_id: String },
    /// A notification launch finished: `opened` when the phone reported it opened.
    NotificationLaunched { key: String, opened: bool },
    /// The phone app took none of the files dragged onto this window: show the *not accepted* notice.
    DragNotAccepted { session_id: String },
    /// This window's drag of phone files ended (completed or cancelled).
    DragEnded { session_id: String },
    /// The phone asked for a launch that brings the main window forward instead.
    ShowMainWindow,
    /// The phone asked for a launch that shows the Apps view instead.
    ShowApps,
}
/// The screen-mirroring feature.
pub struct ScreenMirroring {
    inner: Arc<runtime::Inner>,
}
impl ScreenMirroring {
    /// Installs the feature on Phone Link's `env`, connecting with `connector` and presenting through
    /// `presenter`. Must run inside a tokio runtime.
    pub fn install(
        env: &FeatureEnv,
        config: ScreenMirroringConfig,
        connector: Arc<dyn MirrorConnector>,
        presenter: Arc<dyn MirrorPresenter>,
    ) -> Result<Arc<Self>, FeatureError> {
        let inner = runtime::Inner::install(env, config, connector, presenter)?;
        Ok(Arc::new(Self { inner }))
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ScreenMirroringEvent> {
        self.inner.events.subscribe()
    }

    pub fn status(&self) -> Result<MirroringStatus, MirroringError> {
        self.inner.status()
    }

    /// The windows, in the order they were opened.
    pub fn sessions(&self) -> Vec<SessionView> {
        self.inner.sessions()
    }

    pub fn session(&self, id: &str) -> Option<SessionView> {
        self.inner.sessions().into_iter().find(|s| s.id == id)
    }

    /// Opens (or raises) the phone-screen window; its session id.
    pub fn open_phone_screen(&self) -> Result<String, MirroringError> {
        self.inner.open_phone_screen(None)
    }

    /// Opens phone app `package`, as a task transfer of `transfer_task_id` when set; the session id
    /// of its window (the phone-screen window without app windows).
    pub fn launch_app(&self, package: &str, transfer_task_id: Option<i64>) -> Result<String, MirroringError> {
        self.inner.launch_app(package, transfer_task_id)
    }

    /// Opens notification `key` on the phone (`action_index` −1 for its body).
    pub async fn launch_notification(&self, key: &str, action_index: i32) -> Result<(), MirroringError> {
        self.inner.clone().launch_notification(key, action_index).await
    }

    /// The window's keyboard-settings button, with the kind of the Mac's input source when it was
    /// pressed.
    pub async fn request_keyboard_settings(&self, input_source: InputSourceKind) -> Result<(), MirroringError> {
        self.inner.clone().request_keyboard_settings(input_source).await
    }

    /// The user closed session `id`'s window.
    pub fn close_session(&self, id: &str) -> Result<(), MirroringError> {
        self.inner.close_session(id)
    }

    /// The window of session `id` can present video.
    pub fn window_ready(&self, id: &str) -> Result<(), MirroringError> {
        self.inner.window_ready(id)
    }

    /// Session `id`'s window focus changed; `caps_lock` is the Mac's Caps Lock state.
    pub fn focus_changed(&self, id: &str, state: FocusableState, caps_lock: bool) -> Result<(), MirroringError> {
        self.inner.focus_changed(id, state, caps_lock)
    }

    /// Resume a suspended window.
    pub async fn resume(&self, id: &str) -> Result<bool, MirroringError> {
        self.inner.clone().resume(id).await
    }

    /// Open a closed window's app again.
    pub fn reopen(&self, id: &str) -> Result<(), MirroringError> {
        self.inner.reopen(id)
    }

    /// Retry / Connect: the set-up again for every window.
    pub fn retry(&self) -> Result<(), MirroringError> {
        self.inner.retry()
    }

    /// Continue past the phone's touch-confirmation warning.
    pub fn continue_true_touch(&self, id: &str) -> Result<(), MirroringError> {
        self.inner.continue_true_touch(id)
    }

    pub fn dismiss_error(&self, id: &str) -> Result<(), MirroringError> {
        self.inner.dismiss_error(id)
    }

    /// The window's Unlock / Wake button.
    pub fn wake(&self) -> Result<(), MirroringError> {
        self.inner.wake()
    }

    /// "Resend" the screen-capture prompt to the phone.
    pub fn resend_permission(&self) -> Result<(), MirroringError> {
        self.inner.resend_permission()
    }

    /// The answer to the phone's "move to the Mac" request.
    pub fn answer_move_to_virtual_display(
        &self,
        id: &str,
        accept: bool,
        do_not_ask: bool,
    ) -> Result<(), MirroringError> {
        self.inner.answer_move_to_virtual_display(id, accept, do_not_ask)
    }

    /// The audio-permission window's "continue without audio" for session `id`.
    pub fn continue_without_audio(&self, id: &str) -> Result<(), MirroringError> {
        self.inner.continue_without_audio(id)
    }

    /// Another phone was selected (or none): the connection and every window close.
    pub fn selected_phone_changed(&self) {
        self.inner.selected_phone_changed();
    }

    /// Session `id`'s *enable audio* title-bar button: the phone is asked again whether the
    /// window's app audio may stream — and, when it is refused but can still be allowed, prompts its
    /// user; the window shows the outcome (*audio permission needed* or *always denied*, or its
    /// connected state with or without the button).
    pub async fn enable_audio(&self, id: &str) -> Result<(), MirroringError> {
        self.inner.clone().enable_audio(id).await
    }

    /// The phone's device connection became disabled or unavailable (not merely disconnected): the
    /// mirroring connection closes and every window shows error 81.
    pub fn device_connection_lost(&self) {
        self.inner.device_connection_lost();
    }

    /// The window's audio-permission buttons: ask on the phone, or open the phone's settings.
    pub async fn request_audio_permission(&self, open_settings: bool) -> Result<(), MirroringError> {
        self.inner.clone().request_audio_permission(open_settings).await
    }

    pub fn set_settings(&self, settings: Settings) -> Result<(), MirroringError> {
        self.inner.set_settings(settings)
    }

    /// App `package`'s volume (0–100) and mute.
    pub fn set_volume(&self, package: &str, volume: i32, muted: bool) -> Result<(), MirroringError> {
        self.inner.set_volume(package, volume, muted)
    }

    /// The window asks for screen layout `layout`; confirmed with [`Self::confirm_layout_switch`]
    /// unless "do not ask" is on.
    pub fn switch_layout(&self, id: &str, layout: i32) -> Result<(), MirroringError> {
        self.inner.switch_layout(id, layout)
    }

    pub fn confirm_layout_switch(&self, id: &str, do_not_ask: bool) -> Result<(), MirroringError> {
        self.inner.confirm_layout_switch(id, do_not_ask)
    }

    /// An audio output device appeared or went away.
    pub fn audio_output_changed(&self) {
        self.inner.audio_output_changed();
    }

    /// The Mac's session locked: the connection closes.
    pub fn screen_lock_changed(&self, locked: bool) {
        self.inner.screen_lock_changed(locked);
    }

    // Nearby.
    pub fn nearby_request_pin_code(&self) -> Result<(), MirroringError> {
        self.inner.nearby_send(wire::scope::NEARBY_REQUEST_PIN_CODE, wire::empty_nearby())
    }

    pub fn nearby_send_pin_code(&self, pin: &str) -> Result<(), MirroringError> {
        self.inner.nearby_send(wire::scope::NEARBY_SEND_PIN_CODE, wire::nearby_send_pin_code(pin))
    }

    pub fn nearby_skip_unlock(&self) -> Result<(), MirroringError> {
        self.inner.nearby_send(wire::scope::NEARBY_SKIP_UNLOCK, wire::empty_nearby())
    }

    pub fn nearby_skip_verification(&self) -> Result<(), MirroringError> {
        self.inner.nearby_send(wire::scope::NEARBY_SKIP_VERIFICATION, wire::empty_nearby())
    }

    /// The prompt's code was shown again: its expiry restarts.
    pub fn nearby_code_refreshed(&self) {
        self.inner.nearby_code_refreshed();
    }

    // Input.
    pub fn mouse_move(&self, id: &str, x: f64, y: f64) -> Result<(), MirroringError> {
        self.inner.with_input(id, |p| p.mouse_move(x, y))?;
        self.inner.drag(id)?.pointer_at(x, y);
        Ok(())
    }

    pub fn mouse_button(
        &self,
        id: &str,
        button: MouseButton,
        down: bool,
        x: f64,
        y: f64,
    ) -> Result<(), MirroringError> {
        self.inner.with_input(id, |p| p.mouse_button(button, down, x, y))?;
        self.inner.drag(id)?.pointer_at(x, y);
        Ok(())
    }

    pub fn mouse_wheel(&self, id: &str, x: f64, y: f64, delta_x: i32, delta_y: i32) -> Result<(), MirroringError> {
        self.inner.with_input(id, |p| p.mouse_wheel(x, y, delta_x, delta_y))
    }

    #[expect(clippy::too_many_arguments, reason = "independent inputs of one operation; not yet grouped")]
    pub fn touch(
        &self,
        id: &str,
        pointer: u64,
        phase: TouchPhase,
        x: f64,
        y: f64,
        pressure: f64,
        orientation: f64,
        width: f64,
        height: f64,
    ) -> Result<(), MirroringError> {
        self.inner.with_input(id, |p| p.touch(pointer, phase, x, y, pressure, orientation, width, height))?;
        self.inner.drag(id)?.pointer_at(x, y);
        Ok(())
    }

    #[expect(clippy::too_many_arguments, reason = "independent inputs of one operation; not yet grouped")]
    pub fn pen(
        &self,
        id: &str,
        x: f64,
        y: f64,
        pressure: f64,
        rotation: f64,
        in_range: bool,
        in_contact: bool,
    ) -> Result<(), MirroringError> {
        self.inner.with_input(id, |p| p.pen(x, y, pressure, rotation, in_range, in_contact))?;
        self.inner.drag(id)?.pointer_at(x, y);
        Ok(())
    }

    /// The window started (`true`) or stopped taking input to the phone: the input focus is taken
    /// anew, or lost with everything held released; `caps_lock` is the Mac's Caps Lock state.
    pub fn input_focus(&self, id: &str, focused: bool, caps_lock: bool) -> Result<(), MirroringError> {
        let mut left_released = false;
        self.inner.with_input(id, |p| {
            left_released = p.focused() && p.left_pressed();
            p.take_input(focused, caps_lock);
        })?;

        if left_released {
            self.inner.drag(id)?.focus_lost(true);
        }
        Ok(())
    }

    /// A Mac key; `flags` are the event's modifier flags (`NSEvent.ModifierFlags.rawValue`).
    pub fn key(&self, id: &str, key_code: u16, action: KeyAction, flags: u64) -> Result<(), MirroringError> {
        self.inner.with_input(id, |p| p.key(key_code, action, flags))
    }

    // Drag and drop.

    /// A drag of `files` entered window `id`.
    pub fn drag_enter(&self, id: &str, files: Vec<drag::MacDragFile>) -> Result<drag::EnterOutcome, MirroringError> {
        Ok(self.inner.drag(id)?.enter(files))
    }

    /// The drag is over window `id` at (`x`, `y`) normalized to the video: `true` while it shows copy.
    pub fn drag_over(&self, id: &str, x: f64, y: f64) -> Result<bool, MirroringError> {
        Ok(self.inner.drag(id)?.over(x, y))
    }

    pub fn drag_leave(&self, id: &str) -> Result<(), MirroringError> {
        self.inner.drag(id)?.leave();
        Ok(())
    }

    pub fn drag_drop(&self, id: &str) -> Result<(), MirroringError> {
        self.inner.drag(id)?.drop_on_window();
        Ok(())
    }

    /// A pointer move in window `id`: the phone's files to drag out of it now, if the phone handed a
    /// drag over. `button_held` = a mouse button is down (the Mac drag can start).
    pub fn take_phone_drag(
        &self,
        id: &str,
        button_held: bool,
    ) -> Result<Option<Vec<drag::PhoneDragItem>>, MirroringError> {
        Ok(self.inner.drag(id)?.take_pending(button_held))
    }

    /// The Mac drag of window `id`'s phone files ended; `dropped` on a destination that took it.
    pub fn phone_drag_ended(&self, id: &str, dropped: bool) -> Result<(), MirroringError> {
        self.inner.drag(id)?.mac_drag_ended(dropped);
        Ok(())
    }

    /// Puts the phone file `data_id` of window `id`'s drag at `destination` once it arrived.
    pub async fn write_phone_drag_item(
        &self,
        id: &str,
        data_id: &str,
        destination: &std::path::Path,
    ) -> Result<(), MirroringError> {
        let drag = self.inner.drag(id)?;
        Ok(drag.write_item(data_id, destination).await?)
    }

    /// The window's Back, Home or Recents button (only while connected).
    pub fn nav_button(&self, id: &str, button: NavButton) -> Result<(), MirroringError> {
        self.inner.nav_button(id, button)
    }
}

#[cfg(test)]
mod tests;

use phonelink_core::config::features::ScreenMirroringConfig;
use std::sync::Arc;

use tokio::sync::broadcast;

#[cfg(feature = "test-support")]
pub use edge::{AudioStreamFormat, DisplayGeometry};
pub use edge::{MirrorPresenter, NoPresenter};
pub use errors::{ErrorAction, ErrorKind};
pub use gates::{AppsFeatureType, PhoneValues};
pub use input::{KeyAction, MouseButton, NavButton, TouchPhase};
pub use keymap::{KeyMapping, mac_key};
pub use link::MirrorConnector;
pub use link::NanoMirrorConnector;
pub use machine::{Details, PhoneState, WindowState};
#[cfg(feature = "test-support")]
pub use store::FEATURE;
pub use store::Settings;
pub use wire::FocusableState;
pub use wire::InputSourceKind;
#[cfg(feature = "test-support")]
pub use wire::PHONE_SCREEN_PACKAGE;

use phonelink_features::{FeatureEnv, FeatureError, StoreError};
use phonelink_nano::channel::ChannelError;
