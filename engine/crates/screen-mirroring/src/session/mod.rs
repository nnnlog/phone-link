//! One window (a session): its state, its channels once the phone created them, what the phone says
//! about its display, and the grouping of the phone's channels into a session's set.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::AudioButton;
use super::drag::WindowDrag;
use super::input::InputPipe;
use super::machine::{SessionMachine, WindowState};
use super::wire::{FocusableState, PHONE_SCREEN_PACKAGE, property};
use phonelink_core::sync::MutexExt;
use phonelink_nano::channel::blob::BlobChannel;
use phonelink_nano::channel::message::MessageChannel;
use phonelink_nano::channel::{ChannelType, RawChannel};

/// What the phone says about a window's display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Display {
    /// The phone record's device resolution, which the window starts with: its shape until a video
    /// size is known.
    pub initial_device_resolution: Option<(u32, u32)>,
    /// The last `deviceResolutionChanged` size, as the phone sent it.
    pub device_raw: Option<(u32, u32)>,
    /// `orientationInfo` (0–3).
    pub orientation: i32,
    /// The decoded video's size, as applied.
    pub video: Option<(u32, u32)>,
    /// A video size held until a `deviceResolutionChanged` that agrees with it.
    pending_video: Option<(u32, u32)>,
    /// `posture`: left, top, right, bottom inside the device resolution.
    pub content_area: Option<(i32, i32, i32, i32)>,
    /// `NavigationMode` (0 = gesture navigation, 1–3 = where the phone's own button bar is; −1 until
    /// the phone says) and `ButtonOrder`.
    pub navigation_mode: i32,
    pub button_order: Option<String>,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            initial_device_resolution: None,
            device_raw: None,
            orientation: 0,
            video: None,
            pending_video: None,
            content_area: None,
            navigation_mode: -1,
            button_order: None,
        }
    }
}

/// The phone record's `deviceResolution`, `"<width>,<height>"` with both positive; anything else
/// (another separator, a third part) is no size.
pub fn record_resolution(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once(',')?;
    let size = (w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?);
    (size.0 > 0 && size.1 > 0).then_some(size)
}

fn portrait((w, h): (u32, u32)) -> bool {
    h > w
}

impl Display {
    /// A window starts with the phone record's device resolution (`"<width>,<height>"`, both
    /// positive): its shape until a video size is known, and the size input is scaled to until the
    /// phone sends its own.
    pub fn start_with_record_resolution(&mut self, record: &str) {
        if let Some(size) = record_resolution(record) {
            self.initial_device_resolution = Some(size);
            self.device_resolution_changed(size);
        }
    }

    fn swapped(&self) -> bool {
        self.orientation == 1 || self.orientation == 3
    }

    /// The current device resolution: the phone's size, swapped while the orientation is 1 or 3.
    pub fn device_resolution(&self) -> Option<(u32, u32)> {
        self.device_raw.map(|(w, h)| if self.swapped() { (h, w) } else { (w, h) })
    }

    /// The rotation to present the video with, in degrees (0, −90, −180, −270).
    pub fn rotation(&self) -> i32 {
        match self.orientation {
            1 => -90,
            2 => -180,
            3 => -270,
            _ => 0,
        }
    }

    /// The window offers Back, Home and Recents: the phone navigates by gestures, which a mouse
    /// cannot make, while a phone with its own button bar shows it in the video.
    pub fn navigation_buttons(&self) -> bool {
        self.navigation_mode == 0
    }

    /// A decoded video size: applied, or held when its orientation disagrees with the last device
    /// resolution.
    pub fn video_size(&mut self, size: (u32, u32)) {
        match self.device_raw {
            Some(dev) if self.video.is_some() && portrait(size) != portrait(dev) => self.pending_video = Some(size),
            _ => {
                self.pending_video = None;
                self.video = Some(size);
            }
        }
    }

    /// A new device resolution; a held video size that agrees with it is applied, one that still
    /// disagrees is dropped.
    pub fn device_resolution_changed(&mut self, size: (u32, u32)) {
        self.device_raw = Some(size);
        if let Some(p) = self.pending_video.take()
            && portrait(p) == portrait(size)
        {
            self.video = Some(p);
        }
    }
}

/// The session's channels, once bound.
/// A launch past the touch-confirmation warning: available once the phone asked for the
/// confirmation, and taken by the first launch that bypasses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrueTouchBypass {
    #[default]
    Unavailable,
    Available,
    Claimed,
}

pub struct Bound {
    pub message: Arc<MessageChannel>,
    pub blob: Arc<BlobChannel>,
    /// Cancelled when the set is replaced or the session ends: its tasks stop.
    pub stop: CancellationToken,
}

/// One window.
pub struct Session {
    pub id: String,
    pub package: String,
    /// Creation order.
    pub order: u64,
    /// The window asked for it (`launchApp` is sent); not when the phone asked.
    pub pc_initiated: bool,
    /// The recent task this window continues (a task transfer) until the phone disconnects it.
    pub transfer_task_id: Option<i64>,
    /// Whether a launch past the phone's touch-confirmation warning may be sent.
    pub true_touch: TrueTouchBypass,
    pub machine: SessionMachine,
    /// Bumped on every state change (the watchdog's epoch).
    pub epoch: u64,
    pub bound: Option<Bound>,
    pub input: Arc<Mutex<InputPipe>>,
    /// Drag and drop with the Mac; set when the runtime creates the window.
    pub drag: Option<Arc<WindowDrag>>,
    pub display: Display,
    pub recent_task_id: i64,
    pub focus: FocusableState,
    /// The window can present video.
    pub window_ready: bool,
    pub window_ready_notify: Arc<Notify>,
    /// The phone asked to move the app to the Mac: its transaction and whether it is a task transfer.
    pub move_to_virtual_display: Option<(String, bool)>,
    pub current_layout: Option<i32>,
    /// The layout a switch waits to be confirmed with.
    pub target_layout: Option<i32>,
    /// Phone screen only: a package to open in it once it is connected.
    pub pending_app: Option<String>,
    /// The audio button the window's title bar is given (shown as [`Session::shown_audio_button`]).
    pub audio_button: AudioButton,
    /// The phone's last `captureAllowed` for the window's app while the window is open (`true` until
    /// it says).
    pub audio_capture_allowed: bool,
    /// The app's stored mute was applied to the button (once per window, with the audio workflow
    /// open).
    pub initial_volume_set: bool,
    /// Cancelled when the session ends.
    pub stop: CancellationToken,
}

impl Session {
    pub fn new(id: String, package: String, order: u64, pc_initiated: bool, do_not_show_true_touch: bool) -> Self {
        Self {
            id,
            package,
            order,
            pc_initiated,
            transfer_task_id: None,
            true_touch: TrueTouchBypass::Unavailable,
            machine: SessionMachine::new(do_not_show_true_touch),
            epoch: 0,
            bound: None,
            input: Arc::new(Mutex::new(InputPipe::new())),
            drag: None,
            display: Display::default(),
            recent_task_id: -1,
            focus: FocusableState::NotInFocus,
            window_ready: false,
            window_ready_notify: Arc::new(Notify::new()),
            move_to_virtual_display: None,
            current_layout: None,
            target_layout: None,
            pending_app: None,
            audio_button: AudioButton::None,
            audio_capture_allowed: true,
            initial_volume_set: false,
            stop: CancellationToken::new(),
        }
    }

    pub fn is_phone_screen(&self) -> bool {
        self.package == PHONE_SCREEN_PACKAGE
    }

    /// The audio button the title bar shows now: *enable audio* only while the window is connected;
    /// the volume button in every state.
    pub fn shown_audio_button(&self) -> AudioButton {
        match self.audio_button {
            AudioButton::EnableAudio if self.machine.state != WindowState::Connected => AudioButton::None,
            b => b,
        }
    }

    /// Closes the bound channels.
    pub fn unbind(&mut self) {
        if let Some(b) = self.bound.take() {
            b.stop.cancel();
            b.message.close();
            b.blob.close();
        }
        if let Some(d) = &self.drag {
            d.set_channels(None);
        }
        self.input.lock_or_panic().set_sink(None);
    }
}

/// The four channels of one session's set, as the phone created them.
#[derive(Default)]
pub struct ChannelSet {
    pub package: Option<String>,
    pub message: Option<RawChannel>,
    pub video: Option<RawChannel>,
    pub input: Option<RawChannel>,
    pub blob: Option<RawChannel>,
}

impl ChannelSet {
    fn complete(&self) -> bool {
        self.message.is_some() && self.video.is_some() && self.input.is_some() && self.blob.is_some()
    }

    pub fn close(self) {
        for c in [self.message, self.video, self.input, self.blob].into_iter().flatten() {
            c.close();
        }
    }
}

/// Groups the `remote` workflow's window channels by `sessionId`.
#[derive(Default)]
pub struct Grouping {
    sets: HashMap<String, ChannelSet>,
}

/// What one channel did to the grouping.
pub enum Grouped {
    /// Kept until its set is complete.
    Held,
    /// A complete set for session `session_id`.
    Complete { session_id: String, set: Box<ChannelSet> },
    /// Closed (no session id, a type the window does not use, or another package).
    Closed,
}

impl Grouping {
    pub fn offer(&mut self, channel: RawChannel) -> Grouped {
        let Some(session_id) = channel.announcement.property(property::SESSION_ID).map(str::to_string) else {
            warn!(raw_announcement = ?channel.announcement, "window channel without a sessionId; closed");
            channel.close();
            return Grouped::Closed;
        };

        let package = channel.announcement.property(property::PACKAGE_NAME).map(str::to_string);
        let set = self.sets.entry(session_id.clone()).or_default();
        match (&set.package, &package) {
            (Some(a), Some(b)) if a != b => {
                warn!(session = %session_id, "window channel of another package than its set; closed");
                channel.close();
                return Grouped::Closed;
            }
            (None, Some(_)) => set.package = package,
            _ => {}
        }

        let slot = match channel.announcement.channel_type {
            Some(ChannelType::Message) => &mut set.message,
            Some(ChannelType::VideoTarget) => &mut set.video,
            Some(ChannelType::InputSource) => &mut set.input,
            Some(ChannelType::Blob) => &mut set.blob,
            other => {
                warn!(channel_type = ?other, session = %session_id, "window channel of a type the window does not use; closed");
                channel.close();
                return Grouped::Closed;
            }
        };

        if let Some(earlier) = slot.replace(channel) {
            earlier.close();
        }

        if set.complete() && set.package.is_some() {
            let set = self.sets.remove(&session_id).expect("present");
            return Grouped::Complete { session_id, set: Box::new(set) };
        }
        Grouped::Held
    }

    /// Closes every held channel.
    pub fn clear(&mut self) {
        for (_, s) in self.sets.drain() {
            s.close();
        }
    }
}

#[cfg(test)]
mod tests;
