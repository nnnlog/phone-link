//! A window's input to the phone: the window's pointer and key events, gated by focus and the input
//! channel, scaled to the phone's current device resolution, paced, and released on focus loss.
//!
//! Each step below is one input frame — one press, one move, one scroll, each step of a release or a
//! tap — when it changes the input state; a step that changes nothing on the phone (releasing a key
//! already up, a move to where the pointer is) sends no frame (the channel decides).

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;
use tracing::{debug, trace};

use super::keymap::{KeyMapping, is_option, mac_key, usage};
use phonelink_nano::channel::ChannelError;
use phonelink_nano::channel::input::{Finger, FingerState, InputSource, InputState};

/// Moves of one pointer closer together than this are dropped.
pub const MOVE_INTERVAL: Duration = Duration::from_millis(8);
/// The finger index of the pen.
pub const PEN_FINGER: u8 = 200;
/// `NSEvent.ModifierFlags.capsLock`.
pub const FLAG_CAPS_LOCK: u64 = 1 << 16;
/// `NSEvent.ModifierFlags.option`.
pub const FLAG_OPTION: u64 = 1 << 19;

/// Where a window's input frames go: the session's input channel.
pub trait InputSink: Send + Sync {
    fn apply(&self, change: &mut dyn FnMut(&mut InputState)) -> Result<(), ChannelError>;
}

impl InputSink for InputSource {
    fn apply(&self, change: &mut dyn FnMut(&mut InputState)) -> Result<(), ChannelError> {
        InputSource::apply(self, |s| change(s))
    }
}

/// A mouse button of the Mac. The phone has counters for the first three; any other button only
/// moves the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    Other,
}

impl MouseButton {
    /// The phone's button counter: 0 left, 1 middle, 2 right.
    pub fn index(self) -> Option<usize> {
        match self {
            Self::Left => Some(0),
            Self::Middle => Some(1),
            Self::Right => Some(2),
            Self::Other => None,
        }
    }
}

/// A touch contact's phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TouchPhase {
    Down,
    Moved,
    Up,
    Cancelled,
}

/// A key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyAction {
    Down,
    Up,
    /// An auto-repeat of a held key.
    Repeat,
}

/// The window's navigation buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NavButton {
    Back,
    Home,
    Recents,
}

impl NavButton {
    pub fn usage(self) -> u8 {
        match self {
            Self::Back => usage::F19,
            Self::Home => usage::F15,
            Self::Recents => usage::F18,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Pointer {
    Mouse,
    Touch(u64),
    Pen,
}

/// One window's input state on the PC side.
pub struct InputPipe {
    sink: Option<Arc<dyn InputSink>>,
    focused: bool,
    /// The phone's current device resolution (width and height as the video shows them).
    device: Option<(u32, u32)>,
    last_move: HashMap<Pointer, Instant>,
    /// A position went to the phone since the last focus loss.
    mouse_moved: bool,
    /// The Mac's mouse buttons held since a press inside the video: the pointer is captured.
    buttons_held: BTreeSet<MouseButton>,
    /// The button whose press went to the phone (the first of a chord); `Some(Other)` for a button the
    /// phone has no counter for.
    active_button: Option<MouseButton>,
    touches: BTreeMap<u64, u8>,
    /// Keys whose press was sent (by usage). Releasing everything on focus loss sends their releases
    /// but keeps them, as their key-up still comes.
    keys_held: BTreeSet<u8>,
    /// The Caps Lock state the phone has: off when the channel opens, the Mac's at each focus loss.
    phone_caps: bool,
    /// Focus was gained or lost and no key was pressed since: the next key press brings the phone's
    /// Caps Lock to the Mac's.
    lock_resync: bool,
    /// An Option key went down; the next key pressed without Option releases every held key.
    option_pressed: bool,
    /// A phone → Mac drag is in progress: events outside the video are sent.
    drag_from_phone: bool,
    /// The pen's last range and contact state.
    pen_in_range: bool,
    pen_in_contact: bool,
    /// Input has failed to send since the channel opened or last sent: the failure was logged, and
    /// the next ones are not.
    send_failing: Cell<bool>,
}

impl Default for InputPipe {
    fn default() -> Self {
        Self::new()
    }
}

impl InputPipe {
    pub fn new() -> Self {
        Self {
            sink: None,
            focused: false,
            device: None,
            last_move: HashMap::new(),
            mouse_moved: false,
            buttons_held: BTreeSet::new(),
            active_button: None,
            touches: BTreeMap::new(),
            keys_held: BTreeSet::new(),
            phone_caps: false,
            lock_resync: false,
            option_pressed: false,
            drag_from_phone: false,
            pen_in_range: false,
            pen_in_contact: false,
            send_failing: Cell::new(false),
        }
    }

    /// The input channel opened (`Some`) or closed. An opened channel starts from a phone with
    /// nothing held and Caps Lock off, and takes the window's input at once.
    pub fn set_sink(&mut self, sink: Option<Arc<dyn InputSink>>) {
        self.buttons_held.clear();
        self.active_button = None;
        self.touches.clear();
        self.keys_held.clear();
        self.last_move.clear();
        self.mouse_moved = false;
        self.pen_in_range = false;
        self.pen_in_contact = false;
        self.phone_caps = false;
        self.option_pressed = false;
        self.send_failing.set(false);

        let opened = sink.is_some();
        self.sink = sink;
        if opened {
            self.gain_focus();
        }
    }

    /// The size pointer positions scale to. A missing or non-positive size stops pointer input until a
    /// positive one arrives; the session already ignores a non-positive size from the phone, so the
    /// last positive one stays (Phone Link on Windows scales by whatever size the phone sends).
    pub fn set_device_resolution(&mut self, size: Option<(u32, u32)>) {
        self.device = size.filter(|(w, h)| *w > 0 && *h > 0);
    }

    pub fn set_drag_from_phone(&mut self, on: bool) {
        self.drag_from_phone = on;
    }

    pub fn focused(&self) -> bool {
        self.focused
    }

    /// A drag entering the window takes the input focus, as a press inside the video does.
    pub fn take_focus(&mut self) {
        self.gain_focus();
    }

    /// The left button's press went to the phone and is not released.
    pub fn left_pressed(&self) -> bool {
        self.active_button == Some(MouseButton::Left)
    }

    /// A drag's end releases the pointers, not the keys: the held mouse button at (0, 0), the mouse
    /// to (0, 0), every finger lifted.
    pub fn release_pointers(&mut self) {
        if self.sink.is_none() {
            return;
        }
        self.release_mouse_and_fingers();
    }

    fn send(&self, mut change: impl FnMut(&mut InputState)) {
        let Some(sink) = &self.sink else { return };
        // Input goes per event: a failure is logged when sending starts failing, not per event.
        match sink.apply(&mut change) {
            Ok(()) => self.send_failing.set(false),
            Err(e) if !self.send_failing.replace(true) => {
                debug!(error = %e, "input not sent; later failures are not logged until one is sent");
            }
            Err(e) => trace!(error = %e, "input not sent"),
        }
    }

    /// (`x`, `y`) normalized to the video, in the phone's pixels. A point outside the video goes only
    /// for a `captured` pointer (pressed inside the video and still held) or during a phone → Mac drag;
    /// nothing goes before a device resolution is known.
    fn scale(&self, x: f64, y: f64, captured: bool) -> Option<(i32, i32)> {
        let (w, h) = self.device?;
        let inside = (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y);
        if !inside && !captured && !self.drag_from_phone {
            return None;
        }
        Some(((x * f64::from(w)) as i32, (y * f64::from(h)) as i32))
    }

    /// Whether a move of `pointer` now is paced out.
    fn paced_out(&mut self, pointer: Pointer) -> bool {
        let now = Instant::now();
        if self.last_move.get(&pointer).is_some_and(|t| now.duration_since(*t) < MOVE_INTERVAL) {
            return true;
        }
        self.last_move.insert(pointer, now);
        false
    }

    fn gain_focus(&mut self) {
        if !self.focused {
            self.focused = true;
            self.lock_resync = true;
        }
    }

    /// The window gained or lost the keyboard focus; `caps_lock` is the Mac's Caps Lock state now.
    pub fn focus(&mut self, focused: bool, caps_lock: bool) {
        if focused {
            self.gain_focus();
        } else if self.focused {
            self.release_all();
            self.phone_caps = caps_lock;
            self.lock_resync = true;
            self.focused = false;
        }
    }

    /// The window started (`on`) or stopped taking input: the focus is lost, releasing what is held,
    /// and taken again when it started.
    pub fn take_input(&mut self, on: bool, caps_lock: bool) {
        self.focus(false, caps_lock);
        if on {
            self.focus(true, caps_lock);
        }
    }

    /// Focus loss: the held mouse button (left, middle or right) released at (0, 0), the mouse moved to
    /// (0, 0), every finger lifted at (0, 0), every held key released — each its own frame, and none
    /// when it changes nothing (the move after a release at (0, 0)). The pen is left as it is.
    fn release_all(&mut self) {
        if self.sink.is_none() {
            return;
        }
        self.release_mouse_and_fingers();
        self.release_all_keys();
    }

    fn release_mouse_and_fingers(&mut self) {
        if let Some(i) = self.active_button.take().and_then(MouseButton::index) {
            self.send(|s| {
                let m = s.mice.entry(0).or_default();
                m.x = 0;
                m.y = 0;
                m.button(i, false);
            });
        }

        self.mouse_moved = false;
        self.send(|s| {
            let m = s.mice.entry(0).or_default();
            m.x = 0;
            m.y = 0;
        });

        let fingers: Vec<u8> = std::mem::take(&mut self.touches).into_values().collect();
        for i in fingers {
            self.send(|s| {
                let f = s.fingers.entry(i).or_default();
                *f = Finger { hover_count: f.hover_count, touch_count: f.touch_count, ..Finger::default() };
                f.set_state(FingerState::OutOfRange);
            });
        }

        self.last_move.clear();
    }

    /// Sends a release for every key whose press was sent; they stay held until their key-up.
    fn release_all_keys(&mut self) {
        let keys: Vec<u8> = self.keys_held.iter().copied().collect();
        for u in keys {
            self.send(|s| {
                s.keyboards.entry(0).or_default().key(u, false);
            });
        }
        self.option_pressed = false;
    }

    fn mouse_captured(&self) -> bool {
        !self.buttons_held.is_empty()
    }

    /// The pointer at (`x`, `y`) normalized to the video.
    pub fn mouse_move(&mut self, x: f64, y: f64) {
        if !self.focused || self.sink.is_none() {
            return;
        }

        let Some((px, py)) = self.scale(x, y, self.mouse_captured()) else { return };
        if self.paced_out(Pointer::Mouse) {
            return;
        }

        self.mouse_moved = true;
        self.send(|s| {
            let m = s.mice.entry(0).or_default();
            m.x = px;
            m.y = py;
        });
    }

    /// A Mac mouse button went down or up at (`x`, `y`). The first button pressed inside the video
    /// is the one the phone gets; the others of a chord only move the pointer, and the press is
    /// released when the last held button goes up.
    pub fn mouse_button(&mut self, button: MouseButton, down: bool, x: f64, y: f64) {
        if self.sink.is_none() {
            return;
        }

        if down {
            if self.mouse_captured() {
                self.buttons_held.insert(button);
                self.mouse_move(x, y);
                return;
            }

            let Some((px, py)) = self.scale(x, y, false) else { return };
            self.buttons_held.insert(button);
            self.gain_focus();
            if !self.mouse_moved {
                self.mouse_moved = true;
                self.send(|s| {
                    let m = s.mice.entry(0).or_default();
                    m.x = px;
                    m.y = py;
                });
            }

            self.active_button = Some(button);
            self.send(|s| {
                let m = s.mice.entry(0).or_default();
                m.x = px;
                m.y = py;
                if let Some(i) = button.index() {
                    m.button(i, true);
                }
            });
            return;
        }

        if !self.buttons_held.remove(&button) {
            return;
        }
        if self.mouse_captured() {
            self.mouse_move(x, y);
            return;
        }

        self.last_move.remove(&Pointer::Mouse);
        if !self.focused {
            return;
        }

        let Some((px, py)) = self.scale(x, y, true) else { return };
        let released = self.active_button.take();
        self.send(|s| {
            let m = s.mice.entry(0).or_default();
            m.x = px;
            m.y = py;
            if let Some(i) = released.and_then(MouseButton::index) {
                m.button(i, false);
            }
        });
    }

    /// A scroll at (`x`, `y`): wheel deltas in Windows units (±120 per notch; positive = away from the
    /// user / to the right), added to the phone's wheel totals in one frame with the position. A
    /// scroll takes the input focus, as a press does.
    pub fn mouse_wheel(&mut self, x: f64, y: f64, delta_x: i32, delta_y: i32) {
        if self.sink.is_none() {
            return;
        }

        let Some((px, py)) = self.scale(x, y, self.mouse_captured()) else { return };
        self.gain_focus();
        self.send(|s| {
            let m = s.mice.entry(0).or_default();
            m.x = px;
            m.y = py;
            m.wheel_x = m.wheel_x.wrapping_add(delta_x);
            m.wheel_y = m.wheel_y.wrapping_add(delta_y);
        });
    }

    /// A direct-touch contact `pointer` (an id the window keeps for the contact); pressure 0–1,
    /// orientation in degrees, contact size in display points.
    #[expect(clippy::too_many_arguments, reason = "independent inputs of one operation; not yet grouped")]
    pub fn touch(
        &mut self,
        pointer: u64,
        phase: TouchPhase,
        x: f64,
        y: f64,
        pressure: f64,
        orientation: f64,
        width: f64,
        height: f64,
    ) {
        if self.sink.is_none() {
            return;
        }

        let pressure = (pressure.clamp(0.0, 1.0) * 255.0) as u8;
        let orientation = (orientation.clamp(0.0, 359.0) / 359.0 * 255.0) as u8;
        let (w, h) = (width.clamp(0.0, 65535.0) as u16, height.clamp(0.0, 65535.0) as u16);
        let known = self.touches.contains_key(&pointer);

        let (index, p, to) = match phase {
            TouchPhase::Down => {
                let Some(p) = self.scale(x, y, known) else { return };
                self.gain_focus();
                (self.finger_for(pointer), p, Some(FingerState::InContact))
            }
            TouchPhase::Moved => {
                if !self.focused {
                    return;
                }
                let Some(p) = self.scale(x, y, known) else { return };
                if self.paced_out(Pointer::Touch(pointer)) {
                    return;
                }
                (self.finger_for(pointer), p, None)
            }
            TouchPhase::Up | TouchPhase::Cancelled => {
                self.last_move.remove(&Pointer::Touch(pointer));
                if !self.focused {
                    return;
                }
                let Some(index) = self.touches.remove(&pointer) else { return };
                let Some(p) = self.scale(x, y, true) else { return };
                (index, p, Some(FingerState::OutOfRange))
            }
        };

        self.send(|s| {
            let f = s.fingers.entry(index).or_default();
            (f.x, f.y) = p;
            (f.width, f.height, f.pressure, f.orientation) = (Some(w), Some(h), Some(pressure), Some(orientation));
            if let Some(to) = to {
                f.set_state(to);
            }
        });
    }

    /// The finger index of contact `pointer`: the lowest free index, kept while the contact lasts.
    fn finger_for(&mut self, pointer: u64) -> u8 {
        if let Some(i) = self.touches.get(&pointer) {
            return *i;
        }

        let used: BTreeSet<u8> = self.touches.values().copied().collect();
        let i = (0..PEN_FINGER).find(|i| !used.contains(i)).expect("fewer than 200 touches");
        self.touches.insert(pointer, i);
        i
    }

    /// The pen: position, pressure 0–1, rotation in degrees, and whether it is in range (hovering)
    /// and in contact. A change of contact is a press or release, a change of range an enter or exit;
    /// anything else is a move.
    pub fn pen(&mut self, x: f64, y: f64, pressure: f64, rotation: f64, in_range: bool, in_contact: bool) {
        if self.sink.is_none() {
            return;
        }

        let pressure = (pressure.clamp(0.0, 1.0) * 255.0) as u8;
        let rotation = (rotation.clamp(0.0, 359.0) / 359.0 * 255.0) as u8;

        // A change of contact or range sets the pen's state; a move only its fields.
        let (p, state_change) = match (self.pen_in_contact, in_contact) {
            (false, true) => {
                let Some(p) = self.scale(x, y, false) else { return };
                self.gain_focus();
                (p, true)
            }
            (true, false) => {
                self.last_move.remove(&Pointer::Pen);
                self.pen_in_contact = false;
                self.pen_in_range = in_range;
                if !self.focused {
                    return;
                }
                let Some(p) = self.scale(x, y, true) else { return };
                (p, true)
            }
            _ => {
                if !self.focused {
                    return;
                }
                let Some(p) = self.scale(x, y, in_contact) else { return };
                let entered_or_left = in_range != self.pen_in_range;
                if !entered_or_left && self.paced_out(Pointer::Pen) {
                    return;
                }
                (p, entered_or_left)
            }
        };

        self.pen_in_contact = in_contact;
        self.pen_in_range = in_range;
        let to = match (in_range, in_contact) {
            (_, true) => FingerState::InContact,
            (true, false) => FingerState::Hovering,
            (false, false) => FingerState::OutOfRange,
        };

        self.send(|s| {
            let f = s.fingers.entry(PEN_FINGER).or_default();
            (f.x, f.y) = p;
            (f.width, f.height, f.pressure, f.orientation) = (None, None, Some(pressure), Some(rotation));
            if state_change {
                f.set_state(to);
            }
        });
    }

    /// A press and a release of `u`, each its own frame.
    fn tap(&mut self, u: u8) {
        self.send(|s| {
            s.keyboards.entry(0).or_default().key(u, true);
        });
        self.send(|s| {
            s.keyboards.entry(0).or_default().key(u, false);
        });
    }

    /// A key of the Mac keyboard (its virtual key code); `flags` are the event's modifier flags
    /// (`NSEvent.ModifierFlags.rawValue`, after the event).
    pub fn key(&mut self, key_code: u16, action: KeyAction, flags: u64) {
        if self.sink.is_none() {
            return;
        }

        let mapping = mac_key(key_code);
        if action == KeyAction::Up {
            if let KeyMapping::Usage(u) = mapping {
                if self.keys_held.remove(&u) {
                    self.send(|s| {
                        s.keyboards.entry(0).or_default().key(u, false);
                    });
                }
            }
            return;
        }

        let repeat = action == KeyAction::Repeat;
        let caps_on = flags & FLAG_CAPS_LOCK != 0;

        if is_option(key_code) {
            self.option_pressed = true;
        }
        if flags & FLAG_OPTION == 0 && self.option_pressed {
            self.release_all_keys();
        }

        match mapping {
            KeyMapping::Usage(u) if u == usage::CAPS_LOCK || u == usage::NUM_LOCK => {
                // A lock key does not repeat; right after a focus gain or loss, a Caps Lock toggle that
                // leaves the Mac's state as the phone has it is not sent.
                if repeat || (self.lock_resync && u == usage::CAPS_LOCK && caps_on == self.phone_caps) {
                    return;
                }
                if u == usage::CAPS_LOCK {
                    self.phone_caps = caps_on;
                }
            }
            _ if self.lock_resync => {
                self.lock_resync = false;
                if caps_on != self.phone_caps {
                    self.phone_caps = caps_on;
                    self.tap(usage::CAPS_LOCK);
                }
            }
            _ => {}
        }

        match mapping {
            KeyMapping::Tap(u) => {
                if repeat {
                    return;
                }
                self.tap(u);
            }
            KeyMapping::Usage(u) => {
                self.keys_held.insert(u);
                self.send(|s| {
                    let k = s.keyboards.entry(0).or_default();
                    if repeat {
                        k.key(u, false);
                    }
                    k.key(u, true);
                });
            }
            KeyMapping::NotSent => {}
        }

        self.lock_resync = false;
    }

    /// A navigation button of the window: a press and a release of its usage, then the input focus
    /// taken anew (everything held released, as on a focus loss).
    pub fn nav_button(&mut self, button: NavButton) {
        if self.sink.is_none() {
            return;
        }

        self.tap(button.usage());
        if self.focused {
            self.release_all();
            self.focused = false;
        }
        self.gain_focus();
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::Mutex;

    use phonelink_core::sync::MutexExt;

    use super::*;

    /// A sink that keeps the input state and, as the frames the channel would send, every state a
    /// call changed it to (a call that changes nothing sends no frame).
    #[derive(Default)]
    pub(crate) struct RecordingSink {
        pub state: Mutex<InputState>,
        pub frames: Mutex<Vec<InputState>>,
    }

    impl InputSink for RecordingSink {
        fn apply(&self, change: &mut dyn FnMut(&mut InputState)) -> Result<(), ChannelError> {
            let mut s = self.state.lock_or_panic();
            let before = s.clone();
            change(&mut s);
            if !s.same_as(&before) {
                self.frames.lock_or_panic().push(s.clone());
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;
