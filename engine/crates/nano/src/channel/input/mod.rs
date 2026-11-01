//! The input source (the PC sends).
//!
//! It covers version negotiation, then frames that carry the difference between the current input
//! state and the state the phone last acknowledged, resent every 30 ms until acknowledged.
//!
//! Every user action after the handshake takes a frame id, but a frame goes out only when the action
//! changed the input state since the last frame sent; an action that changes nothing (releasing a key
//! already up, a move to where the pointer is) sends nothing, and the next frame sent skips its id, as
//! Phone Link on Windows does. Actions before the handshake change the state without a frame; the
//! first frame after it carries them.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::watch;
use tracing::{debug, trace, warn};

use super::{Announcement, ChannelError, ChannelSender, PT_CHANNEL, RawChannel, RawInbound, Reader, frame, unframe};
use crate::transport::SendMode;
use phonelink_core::sync::MutexExt;

mod kind {
    pub(super) const SERVER_HANDSHAKE_V3: u32 = 1;
    pub(super) const FRAME_ACK: u32 = 3;
    pub(super) const SERVER_HANDSHAKE_V4: u32 = 5;
    pub(super) const CLIENT_HANDSHAKE_V4: u32 = 6;
    pub(super) const FRAME_V4: u32 = 7;
}

/// Versions negotiation steps down past (Nano's blacklist).
const BLACKLIST: [u32; 6] = [0, 1, 2, 5, 6, 7];
const MAX_VERSION: u32 = 8;
/// The lowest version the PC encodes (versions 4–7 share one layout).
const MIN_VERSION: u32 = 4;
/// The resend period of an unacknowledged frame.
pub const RETRANSMIT: Duration = Duration::from_millis(30);
/// Touches the PC reports it can send.
pub const MAXIMUM_TOUCHES: u32 = 10;

/// Optional finger fields, each sent only when set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Finger {
    pub x: i32,
    pub y: i32,
    pub hover_count: u8,
    /// Odd = in contact.
    pub touch_count: u8,
    pub cancel_count: Option<u8>,
    pub width: Option<u16>,
    pub height: Option<u16>,
    pub pressure: Option<u8>,
    pub orientation: Option<u8>,
}

/// Where a finger is: its counters encode it — out of range while the hover counter is even, else
/// in contact while the touch counter is odd, else hovering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FingerState {
    OutOfRange,
    Hovering,
    InContact,
}

impl Finger {
    pub fn state(&self) -> FingerState {
        match (self.hover_count % 2 == 1, self.touch_count % 2 == 1) {
            (false, _) => FingerState::OutOfRange,
            (true, false) => FingerState::Hovering,
            (true, true) => FingerState::InContact,
        }
    }

    /// Moves the finger to `to`: entering or leaving range steps the hover counter, touching or
    /// lifting the touch counter, so a contact straight from out of range steps both.
    pub fn set_state(&mut self, to: FingerState) {
        use FingerState::{Hovering, InContact, OutOfRange};
        let (hover, touch) = match (self.state(), to) {
            (OutOfRange, Hovering) | (Hovering, OutOfRange) => (true, false),
            (Hovering, InContact) | (InContact, Hovering) => (false, true),
            (OutOfRange, InContact) | (InContact, OutOfRange) => (true, true),
            _ => (false, false),
        };

        if hover {
            self.hover_count = self.hover_count.wrapping_add(1);
        }
        if touch {
            self.touch_count = self.touch_count.wrapping_add(1);
        }
    }

    /// The optional fields go out only when non-zero (the phone reads an absent one as 0).
    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.x.to_le_bytes());
        out.extend_from_slice(&self.y.to_le_bytes());
        out.push(self.hover_count);
        out.push(self.touch_count);

        let (cancel, width, height, pressure, orientation) = (
            self.cancel_count.filter(|v| *v != 0),
            self.width.filter(|v| *v != 0),
            self.height.filter(|v| *v != 0),
            self.pressure.filter(|v| *v != 0),
            self.orientation.filter(|v| *v != 0),
        );

        let presence = cancel.map_or(0, |_| 0x01)
            | width.map_or(0, |_| 0x02)
            | height.map_or(0, |_| 0x04)
            | pressure.map_or(0, |_| 0x08)
            | orientation.map_or(0, |_| 0x10);
        out.push(presence);

        if let Some(c) = cancel {
            out.push(c);
        }
        if let Some(w) = width {
            out.extend_from_slice(&w.to_le_bytes());
        }
        if let Some(h) = height {
            out.extend_from_slice(&h.to_le_bytes());
        }
        if let Some(p) = pressure {
            out.push(p);
        }
        if let Some(o) = orientation {
            out.push(o);
        }
    }
}

/// Mouse buttons are press counters (index 0 left, 1 middle, 2 right; odd = pressed); the wheels are
/// totals (`wheel_x` horizontal, `wheel_y` vertical).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mouse {
    pub buttons: [u8; 8],
    pub x: i32,
    pub y: i32,
    pub wheel_x: i32,
    pub wheel_y: i32,
}

impl Mouse {
    /// Version 8 adds the relative position, which the PC never moves (0, 0).
    fn encode(&self, version: u32, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.buttons);
        for v in [self.x, self.y, self.wheel_x, self.wheel_y] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        if version >= 8 {
            out.extend_from_slice(&[0; 8]);
        }
    }

    /// Presses or releases `button`; returns whether the state changed.
    pub fn button(&mut self, button: usize, down: bool) -> bool {
        let pressed = self.buttons[button] % 2 == 1;
        if pressed == down {
            return false;
        }
        self.buttons[button] = self.buttons[button].wrapping_add(1);
        true
    }
}

/// 256 press counters indexed by USB HID usage (odd = pressed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keyboard {
    pub counters: [u8; 256],
}

impl Default for Keyboard {
    fn default() -> Self {
        Self { counters: [0; 256] }
    }
}

impl Keyboard {
    /// A key down or up; returns whether the state changed.
    pub fn key(&mut self, hid_usage: u8, down: bool) -> bool {
        let c = &mut self.counters[usize::from(hid_usage)];
        if (*c % 2 == 1) == down {
            return false;
        }
        *c = c.wrapping_add(1);
        true
    }
}

/// The whole input state. The phone applies mouse and keyboard records only at index 0.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputState {
    pub fingers: BTreeMap<u8, Finger>,
    pub mice: BTreeMap<u8, Mouse>,
    pub keyboards: BTreeMap<u8, Keyboard>,
}

impl InputState {
    /// Whether the phone sees the two states alike: an absent finger, mouse or keyboard is one at
    /// its default.
    pub fn same_as(&self, other: &InputState) -> bool {
        fn same<T: PartialEq + Default>(a: &BTreeMap<u8, T>, b: &BTreeMap<u8, T>) -> bool {
            let default = T::default();
            a.keys().chain(b.keys()).all(|i| a.get(i).unwrap_or(&default) == b.get(i).unwrap_or(&default))
        }

        same(&self.fingers, &other.fingers) && same(&self.mice, &other.mice) && same(&self.keyboards, &other.keyboards)
    }

    /// `FrameChanges` from `base` to `self` for `version` 4–8.
    pub fn changes_since(&self, base: &InputState, version: u32) -> Vec<u8> {
        /// The entries of one group that differ from `base`.
        fn changed<'a, T: PartialEq + Default>(
            current: &'a BTreeMap<u8, T>,
            base: &'a BTreeMap<u8, T>,
            default: &'a T,
        ) -> Vec<(u8, &'a T, &'a T)> {
            current
                .iter()
                .filter_map(|(i, v)| {
                    let b = base.get(i).unwrap_or(default);
                    (b != v).then_some((*i, v, b))
                })
                .collect()
        }

        let (no_finger, no_mouse, no_keyboard) = (Finger::default(), Mouse::default(), Keyboard::default());
        let fingers = changed(&self.fingers, &base.fingers, &no_finger);
        let mice = changed(&self.mice, &base.mice, &no_mouse);
        let keyboards = changed(&self.keyboards, &base.keyboards, &no_keyboard);

        // Every group's count and indices come first (gamepads, and from version 8 vibrations and
        // mouse feedback: none), then the records in the same order.
        let mut groups = vec![
            fingers.iter().map(|e| e.0).collect::<Vec<_>>(),
            mice.iter().map(|e| e.0).collect(),
            Vec::new(),
            keyboards.iter().map(|e| e.0).collect(),
        ];
        if version >= 8 {
            groups.extend([Vec::new(), Vec::new()]);
        }

        let mut out = Vec::new();
        for indices in groups {
            out.push(indices.len() as u8);
            out.extend_from_slice(&indices);
        }

        for (_, f, _) in &fingers {
            f.encode(&mut out);
        }
        for (_, m, _) in &mice {
            m.encode(version, &mut out);
        }
        for (_, k, b) in &keyboards {
            let keys: Vec<(u8, u8)> = (0..=255u8)
                .filter(|u| k.counters[usize::from(*u)] != b.counters[usize::from(*u)])
                .map(|u| (u, k.counters[usize::from(u)]))
                .collect();

            out.push(keys.len() as u8);
            for (u, c) in keys {
                out.push(u);
                out.push(c);
            }
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
        }

        out
    }
}

/// What the phone's handshake said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputHandshake {
    pub version: u32,
    pub desktop_width: u32,
    pub desktop_height: u32,
    pub maximum_touches: u32,
    pub initial_frame_id: u32,
}

/// The negotiated version: 3 if the phone's maximum is 3; otherwise from `min(max, 8)` down past every
/// blacklisted one, not below the phone's minimum.
pub fn negotiate(min: u32, max: u32) -> Option<u32> {
    if max == 3 {
        return Some(3);
    }
    if max < min {
        return None;
    }

    let mut v = max.min(MAX_VERSION);
    while BLACKLIST.contains(&v) {
        v = v.checked_sub(1)?;
    }
    (v >= min).then_some(v)
}

/// The Unix time in ms at which `clock` (the frames' time base) read 0. Frame times are in µs on the
/// same clock, so the two units differ, as Phone Link on Windows sends them; the phone uses them only
/// for its own timing.
fn reference_ms(clock: Instant) -> u64 {
    let unix = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    unix.saturating_sub(clock.elapsed().as_millis() as u64)
}

/// The channel's protocol state (sans-IO).
pub struct InputCore {
    handshake: Option<InputHandshake>,
    current: InputState,
    acked: InputState,
    /// The latest frame's id and time (µs on `started`); `None` before the first frame.
    latest: Option<(u32, u64)>,
    /// The state the latest frame carried (empty before the first): an action that leaves `current`
    /// equal to it sends nothing.
    latest_state: InputState,
    next_id: u32,
    acked_id: Option<u32>,
    sent: BTreeMap<u32, InputState>,
    /// The frames' time base; the handshake's reference is the Unix time at which it read 0.
    started: Instant,
    out: Vec<(SendMode, Vec<u8>)>,
    failed: Option<String>,
    /// A malformed or unknown inbound packet was logged at `warn`; later ones, which come with
    /// every acknowledgement while the condition lasts, are logged at `debug`.
    anomaly_noted: bool,
}

impl InputCore {
    pub fn new() -> Self {
        Self {
            handshake: None,
            current: InputState::default(),
            acked: InputState::default(),
            latest: None,
            latest_state: InputState::default(),
            next_id: 0,
            acked_id: None,
            sent: BTreeMap::new(),
            started: Instant::now(),
            out: Vec::new(),
            failed: None,
            anomaly_noted: false,
        }
    }

    /// `true` the first time an inbound anomaly is seen on the channel.
    fn first_anomaly(&mut self) -> bool {
        !std::mem::replace(&mut self.anomaly_noted, true)
    }

    fn log_unknown_kind(&mut self, kind: u32) {
        if self.first_anomaly() {
            warn!(kind, "input packet of an unknown kind dropped");
        } else {
            debug!(kind, "input packet of an unknown kind dropped");
        }
    }

    pub fn handshake(&self) -> Option<InputHandshake> {
        self.handshake
    }

    pub fn failed(&self) -> Option<&str> {
        self.failed.as_deref()
    }

    pub fn take_output(&mut self) -> Vec<(SendMode, Vec<u8>)> {
        std::mem::take(&mut self.out)
    }

    /// One inbound packet. A handshake the PC cannot take fails the channel ([`InputCore::failed`]),
    /// which its task logs as it closes the channel.
    pub fn on_packet(&mut self, payload: &[u8]) {
        if let Err(e) = self.handle(payload) {
            if self.failed.is_some() {
                return;
            }
            if self.first_anomaly() {
                warn!(error = %e, "malformed input packet dropped");
            } else {
                debug!(error = %e, "malformed input packet dropped");
            }
        }
    }

    fn handle(&mut self, payload: &[u8]) -> Result<(), ChannelError> {
        let (k, body) = unframe(payload)?;
        let mut r = Reader::new(body);
        match k {
            kind::SERVER_HANDSHAKE_V4 | kind::SERVER_HANDSHAKE_V3 => {
                let (min, max) = if k == kind::SERVER_HANDSHAKE_V4 {
                    (r.u32()?, r.u32()?)
                } else {
                    let v = r.u32()?;
                    (v, v)
                };
                let (w, h, touches, initial) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?);

                let version = match negotiate(min, max) {
                    Some(v) if v >= MIN_VERSION => v,
                    other => {
                        let why = format!("input versions {min}–{max} give {other:?}, which the PC does not send");
                        self.failed = Some(why.clone());
                        return Err(ChannelError::Handshake(why));
                    }
                };

                self.handshake = Some(InputHandshake {
                    version,
                    desktop_width: w,
                    desktop_height: h,
                    maximum_touches: touches,
                    initial_frame_id: initial,
                });

                // The phone applies only ids newer than its initial one, so the first action takes the
                // next. Phone Link on Windows gives the initial id itself to its first action, so a
                // first frame of its that changes something is dropped.
                self.next_id = initial.wrapping_add(1);

                let mut b = MIN_VERSION.to_le_bytes().to_vec();
                b.extend_from_slice(&version.to_le_bytes());
                b.extend_from_slice(&MAXIMUM_TOUCHES.max(touches).to_le_bytes());
                b.extend_from_slice(&reference_ms(self.started).to_le_bytes());
                self.out.push((SendMode::ReliableFence, frame(kind::CLIENT_HANDSHAKE_V4, &b)));
                debug!(version, width = w, height = h, "Nano input handshake done");
            }
            kind::FRAME_ACK => self.on_ack(r.u32()?),
            other => self.log_unknown_kind(other),
        }

        Ok(())
    }

    fn on_ack(&mut self, id: u32) {
        if self.acked_id.is_some_and(|a| (id.wrapping_sub(a) as i32) <= 0) {
            return;
        }
        let Some((latest, _)) = self.latest else {
            return;
        };

        // An id never sent acknowledges the nearest sent frame at or before it.
        let id = if (id.wrapping_sub(latest) as i32) > 0 { latest } else { id };
        let base = self
            .sent
            .iter()
            .filter(|(s, _)| (id.wrapping_sub(**s) as i32) >= 0)
            .min_by_key(|(s, _)| id.wrapping_sub(**s));

        if let Some((sent_id, state)) = base {
            let sent_id = *sent_id;
            self.acked = state.clone();
            self.acked_id = Some(sent_id);
            self.sent.retain(|s, _| (s.wrapping_sub(sent_id) as i32) > 0);
        }
    }

    /// Sends frame `id` begun at `us` (µs on `started`); a resend keeps the frame's own time.
    fn send_frame(&mut self, id: u32, us: u64) {
        let version = self.handshake.expect("frames only after the handshake").version;
        let mut b = id.to_le_bytes().to_vec();
        b.extend_from_slice(&us.to_le_bytes());
        b.extend_from_slice(&self.latest_state.changes_since(&self.acked, version));

        trace!(frame_id = id, data = %hex::encode(&b), "input frame queued");
        self.out.push((SendMode::Unreliable, frame(kind::FRAME_V4, &b)));
    }

    /// Applies one user action to the state and sends its frame when the state changed since the
    /// latest frame. After the handshake every action takes a frame id, sent or not; before it the
    /// change is only kept.
    pub fn apply(&mut self, change: impl FnOnce(&mut InputState)) -> Result<(), ChannelError> {
        if let Some(why) = &self.failed {
            return Err(ChannelError::Handshake(why.clone()));
        }

        let us = self.started.elapsed().as_micros() as u64;
        change(&mut self.current);
        if self.handshake.is_none() {
            return Ok(());
        }

        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        if self.current.same_as(&self.latest_state) {
            return Ok(());
        }

        self.latest = Some((id, us));
        self.latest_state = self.current.clone();
        self.sent.insert(id, self.current.clone());
        self.send_frame(id, us);
        Ok(())
    }

    /// The 30 ms tick: resend the latest frame while it is not acknowledged.
    pub fn on_tick(&mut self) {
        if let Some((latest, us)) = self.latest {
            if self.acked_id != Some(latest) {
                self.send_frame(latest, us);
            }
        }
    }
}

impl Default for InputCore {
    fn default() -> Self {
        Self::new()
    }
}

/// An open input source.
pub struct InputSource {
    pub announcement: Announcement,
    sender: ChannelSender,
    core: Arc<Mutex<InputCore>>,
    ready: watch::Receiver<Option<InputHandshake>>,
}

impl InputSource {
    pub async fn open(mut raw: RawChannel) -> Result<Self, ChannelError> {
        let early = raw.open().await?;
        let (sender, announcement, mut inbound) = raw.split();

        let core = Arc::new(Mutex::new(InputCore::new()));
        let (ready_tx, ready) = watch::channel(None);

        let (c, s) = (core.clone(), sender.clone());
        tokio::spawn(async move {
            let mut queued = early.into_iter();
            let mut tick = tokio::time::interval(RETRANSMIT);
            loop {
                let item = match queued.next() {
                    Some(i) => Some(i),
                    None => tokio::select! {
                        i = inbound.recv() => Some(i.unwrap_or(RawInbound::Closed)),
                        _ = tick.tick() => None,
                    },
                };

                let out = {
                    let mut core = c.lock_or_panic();
                    match item {
                        Some(RawInbound::Data { payload, .. }) => core.on_packet(&payload),
                        Some(RawInbound::Opened) => {}
                        Some(RawInbound::Closed) => return,
                        None => core.on_tick(),
                    }

                    if let Some(why) = core.failed() {
                        warn!(why, "input handshake refused; the channel closes");
                        s.close();
                        return;
                    }

                    let _ = ready_tx.send(core.handshake());
                    core.take_output()
                };
                flush(&s, out);
            }
        });

        Ok(Self { announcement, sender, core, ready })
    }

    /// Waits for the phone's handshake.
    pub async fn ready(&mut self) -> Result<InputHandshake, ChannelError> {
        let r = self.ready.wait_for(Option::is_some).await.map_err(|_| ChannelError::Closed)?;
        Ok(r.expect("waited for Some"))
    }

    /// Applies one user action and sends its frame if it changed anything ([`InputCore::apply`]).
    pub fn apply(&self, change: impl FnOnce(&mut InputState)) -> Result<(), ChannelError> {
        let out = {
            let mut c = self.core.lock_or_panic();
            c.apply(change)?;
            c.take_output()
        };
        flush(&self.sender, out);
        Ok(())
    }

    pub fn close(&self) {
        self.sender.close();
    }
}

fn flush(sender: &ChannelSender, out: Vec<(SendMode, Vec<u8>)>) {
    for (mode, p) in out {
        if let Err(e) = sender.send(mode, PT_CHANNEL, p) {
            super::log_not_sent("input", &e);
        }
    }
}

#[cfg(test)]
mod tests;
