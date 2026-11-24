//! Where the calls' audio is — on the computer while a voice link is open, on the phone otherwise — and
//! what depends on it: the codec the phone selects, whether the computer takes the phone's link, and
//! the in-call operations that act only on a call heard on the computer (hold, mute, the keypad, the
//! transfer between computer and phone).
//!
//! The computer takes the link for a call placed or answered from it, or transferred to it; a call
//! placed or answered on the phone stays there until the user transfers it.

use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{info, warn};

use phonelink_core::sync::MutexExt;

use super::at::{self, command};
use super::calls::{Call, CallRole, CallStatus};
use super::link::Link;
use super::voice::{VoiceCodec, VoiceEvent};
use super::{CallControl, CallingEvent, ControlError};

/// The voice link's state for one connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VoiceState {
    /// The codecs the computer offers the phone on this connection; none when it carries no voice.
    offered: Vec<VoiceCodec>,
    /// The computer and the phone both negotiate the codec, so the phone opens every link.
    negotiated: bool,
    /// The codec of the next link: the one the phone selected last, narrow band before any.
    codec: VoiceCodec,
    /// The computer takes the phone's voice link.
    wanted: bool,
    /// A voice link is open: the calls' audio is on the computer.
    pub(super) open: bool,
    pub(super) muted: bool,
    /// The call window's main call when the calls last changed; a mute ends with it.
    primary: Option<u64>,
}

impl Default for VoiceState {
    fn default() -> Self {
        Self {
            offered: Vec::new(),
            negotiated: false,
            codec: VoiceCodec::Cvsd,
            wanted: false,
            open: false,
            muted: false,
            primary: None,
        }
    }
}

impl VoiceState {
    /// A new connection's state: a computer that carries `codecs`, a phone with `ag_features`.
    pub(super) fn for_connection(codecs: &[VoiceCodec], ag_features: u32) -> Self {
        let negotiated = codecs.contains(&VoiceCodec::Msbc) && ag_features & at::AG_CODEC_NEGOTIATION != 0;
        let offered = if negotiated { codecs.to_vec() } else { narrow_band_only(codecs) };
        Self { offered, negotiated, ..Self::default() }
    }
}

fn narrow_band_only(codecs: &[VoiceCodec]) -> Vec<VoiceCodec> {
    codecs.iter().copied().filter(|c| *c == VoiceCodec::Cvsd).collect()
}

/// The ids of `codecs` in ascending order, as `AT+BAC` lists them.
pub(super) fn codec_ids(codecs: &[VoiceCodec]) -> Vec<u8> {
    let mut ids: Vec<u8> = codecs.iter().map(|c| c.id()).collect();
    ids.sort_unstable();
    ids
}

/// Which in-call controls act on a call now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CallControls {
    /// Mute and unmute.
    pub mute: bool,
    /// Hold and resume.
    pub hold: bool,
    /// The keypad's keys.
    pub keypad: bool,
    /// The transfer to the other side: to the phone while the audio is on the computer, to the
    /// computer otherwise.
    pub transfer: bool,
}

/// The controls of `call`, one of `tracked` calls in the call window, with the audio on the computer or
/// not, on a computer that can take it or not, and a phone with three-way calling or not.
pub(super) fn controls(
    call: &Call,
    tracked: usize,
    on_computer: bool,
    can_take: bool,
    three_way: bool,
) -> CallControls {
    let in_window = call.role != CallRole::Untracked;
    let talking = call.status == CallStatus::Talking;
    let held_or_talking = matches!(call.status, CallStatus::Talking | CallStatus::Held);

    CallControls {
        mute: in_window && on_computer && talking,
        hold: in_window && on_computer && held_or_talking && tracked == 1 && three_way,
        keypad: in_window && on_computer && talking,
        transfer: if on_computer { true } else { can_take && call.status != CallStatus::Incoming },
    }
}

impl CallControl {
    // MARK: - Operations

    /// Puts call `id` on hold; a call already held stays held.
    pub async fn hold(&self, id: u64) -> Result<(), ControlError> {
        self.set_held(id, CallStatus::Talking).await
    }

    /// Resumes held call `id`; a call already talking stays so.
    pub async fn resume(&self, id: u64) -> Result<(), ControlError> {
        self.set_held(id, CallStatus::Held).await
    }

    /// Holds or resumes call `id` when its status is `from`, and does nothing when it already is the
    /// other.
    async fn set_held(&self, id: u64, from: CallStatus) -> Result<(), ControlError> {
        let (link, call, controls) = self.call_controls(id)?;
        if !controls.hold {
            return Err(ControlError::InvalidState);
        }
        if call.status != from {
            return Ok(());
        }

        self.send(&link, command::HOLD_AND_ACCEPT_OTHER).await?;
        self.refresh_calls(&link).await;
        Ok(())
    }

    /// Mutes or unmutes call `id`: while muted the computer sends silence.
    pub fn set_muted(&self, id: u64, muted: bool) -> Result<(), ControlError> {
        let (_, _, controls) = self.call_controls(id)?;
        if !controls.mute {
            return Err(ControlError::InvalidState);
        }

        self.inner.lock_or_recover().voice.muted = muted;
        self.voice.set_muted(muted);
        self.shared_calls_changed();
        Ok(())
    }

    /// Sends keypad key `key` (`0`–`9`, `*`, `#`) into call `id`, and plays its tone on the computer.
    pub async fn send_key(&self, id: u64, key: char) -> Result<(), ControlError> {
        let (link, _, controls) = self.call_controls(id)?;
        if !controls.keypad || !matches!(key, '0'..='9' | '*' | '#') {
            return Err(ControlError::InvalidState);
        }

        self.send(&link, &command::key(key)).await?;
        self.voice.play_key(key);
        Ok(())
    }

    /// Moves the audio of call `id` to the computer; a call that is not in the call window joins it.
    /// Fails when the voice link is not open in time.
    pub async fn transfer_to_computer(&self, id: u64) -> Result<(), ControlError> {
        let (link, _, controls) = self.call_controls(id)?;
        let (open, negotiated) = {
            let inner = self.inner.lock_or_recover();
            (inner.voice.open, inner.voice.negotiated)
        };
        if open {
            return Ok(());
        }
        if !controls.transfer {
            return Err(ControlError::InvalidState);
        }

        // Tracked first: the computer gives the audio back while no call is in the call window.
        let follow = self.inner.lock_or_recover().book.track(id);
        self.want_audio(true);
        self.shared_calls_changed();
        self.follow_up(&link, follow.into_iter().collect()).await;

        let asked =
            if negotiated { self.send(&link, command::CODEC_CONNECTION).await } else { self.connect_voice().await };
        if asked.is_ok() && self.wait_for_audio().await {
            return Ok(());
        }

        self.want_audio(false);
        asked.and(Err(ControlError::Refused("the voice link did not open".into())))
    }

    /// Moves the audio of call `id` to the phone.
    pub fn transfer_to_phone(&self, id: u64) -> Result<(), ControlError> {
        self.call(id)?;
        if !self.inner.lock_or_recover().voice.open {
            return Ok(());
        }

        info!("call audio transferred to the phone");
        self.want_audio(false);
        self.voice.close();
        Ok(())
    }

    // MARK: - State

    /// A call can be heard and spoken on the computer.
    pub(super) fn can_take_audio(&self) -> bool {
        !self.codecs.is_empty() && self.voice.can_carry()
    }

    /// The computer takes the audio of a call placed or answered from it, when it can.
    pub(super) fn take_audio(&self) {
        if self.can_take_audio() {
            self.want_audio(true);
        }
    }

    /// Sets whether the computer takes the phone's voice link, and tells the edge.
    fn want_audio(&self, wanted: bool) {
        let codec = {
            let mut inner = self.inner.lock_or_recover();
            inner.voice.wanted = wanted;
            inner.voice.codec
        };
        self.voice.expect(wanted.then_some(codec));
    }

    /// After the calls changed: a mute ends when another call becomes the main one, and with no call
    /// left in the call window the computer gives the audio back.
    pub(super) fn settle_audio(&self) {
        let (unmute, release) = {
            let mut inner = self.inner.lock_or_recover();
            let primary = inner.book.primary();
            let unmute = inner.voice.muted && inner.voice.primary != primary;
            if unmute {
                inner.voice.muted = false;
            }
            inner.voice.primary = primary;

            let idle = primary.is_none() && !inner.book.awaits_dialed();
            let release = idle && (inner.voice.wanted || inner.voice.open);
            if release {
                inner.voice.wanted = false;
            }
            (unmute, release)
        };

        if unmute {
            self.voice.set_muted(false);
        }
        if release {
            self.voice.expect(None);
            self.voice.close();
        }
    }

    fn call_controls(&self, id: u64) -> Result<(Arc<Link>, Call, CallControls), ControlError> {
        let (link, call) = self.call(id)?;
        let (tracked, open, three_way) = {
            let inner = self.inner.lock_or_recover();
            let tracked = inner.book.calls().iter().filter(|c| c.role != CallRole::Untracked).count();
            (tracked, inner.voice.open, inner.ag_features & at::AG_THREE_WAY != 0)
        };
        let controls = controls(&call, tracked, open, self.can_take_audio(), three_way);
        Ok((link, call, controls))
    }

    /// Opens the voice link from the computer, for a phone that opens none itself.
    async fn connect_voice(&self) -> Result<(), ControlError> {
        let voice = self.voice.clone();
        let codec = self.inner.lock_or_recover().voice.codec;
        let connected = tokio::task::spawn_blocking(move || voice.connect(codec)).await;
        match connected {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(ControlError::Refused(e.to_string())),
            Err(e) => Err(ControlError::Internal(e.to_string())),
        }
    }

    /// Waits for the voice link to open, at most as long as a transfer waits.
    async fn wait_for_audio(&self) -> bool {
        let open = || self.inner.lock_or_recover().voice.open;
        tokio::time::timeout(self.shared.config.control_voice_wait, async {
            loop {
                let changed = self.calls_changed.notified();
                if open() {
                    return;
                }
                changed.await;
            }
        })
        .await
        .is_ok()
    }

    // MARK: - The edge

    /// Has the edge serve the phone at `address` for this connection, when the computer carries voice.
    pub(super) fn attach_voice(self: &Arc<Self>, address: String, generation: u64) {
        if self.codecs.is_empty() {
            return;
        }

        let (events, received) = mpsc::unbounded_channel();
        self.voice.attach(address, events);
        spawn_voice_events(self, received, generation);
    }

    /// The connection ended: the edge releases the link and stops serving the phone.
    pub(super) fn detach_voice(&self) {
        if !self.codecs.is_empty() {
            self.voice.detach();
        }
    }

    /// The phone selected codec `id` for the next link: confirmed when the computer offered it,
    /// otherwise answered with the codecs it offers.
    pub(super) async fn codec_selected(&self, link: &Link, generation: u64, id: u8) {
        let (reply, expect) = {
            let mut inner = self.inner.lock_or_recover();
            if inner.generation != generation {
                return;
            }
            match VoiceCodec::from_id(id).filter(|c| inner.voice.offered.contains(c)) {
                Some(codec) => {
                    inner.voice.codec = codec;
                    (command::confirm_codec(id), inner.voice.wanted.then_some(codec))
                }
                None => (command::available_codecs(&codec_ids(&inner.voice.offered)), None),
            }
        };

        // The edge knows the codec before the phone, once confirmed, opens the link.
        if expect.is_some() {
            self.voice.expect(expect);
        }
        if let Err(e) = self.send(link, &reply).await {
            warn!(error = %e, "the phone's codec selection was not answered");
        }
    }

    async fn voice_event(&self, generation: u64, event: VoiceEvent) {
        match event {
            VoiceEvent::Opened => {
                if !self.update_voice(generation, |voice| voice.open = true) {
                    return;
                }
                info!("call audio is on the computer");
            }
            VoiceEvent::Closed => {
                let closed = self.update_voice(generation, |voice| {
                    voice.open = false;
                    voice.wanted = false;
                    voice.muted = false;
                });
                if !closed {
                    return;
                }
                info!("call audio is on the phone");
                self.voice.expect(None);
                self.voice.set_muted(false);
            }
            VoiceEvent::CodecFailed(codec) => return self.codec_failed(generation, codec).await,
            VoiceEvent::CarryChanged => {
                let phone = self.inner.lock_or_recover().phone.clone();
                if let Some(phone) = phone {
                    self.shared.emit(CallingEvent::ControlChanged { phone });
                }
            }
        }
        self.shared_calls_changed();
    }

    /// Applies `change` to the voice state of connection `generation`; `false` when that connection is
    /// gone.
    fn update_voice(&self, generation: u64, change: impl FnOnce(&mut VoiceState)) -> bool {
        let mut inner = self.inner.lock_or_recover();
        if inner.generation != generation {
            return false;
        }
        change(&mut inner.voice);
        true
    }

    /// A wide-band link could not be set up on the computer: the phone is offered narrow band only for
    /// the rest of the connection, and asked for the link again when the computer wants the audio.
    async fn codec_failed(&self, generation: u64, codec: VoiceCodec) {
        let (link, wanted, offered) = {
            let mut inner = self.inner.lock_or_recover();
            let fallback = narrow_band_only(&inner.voice.offered);
            if inner.generation != generation || codec != VoiceCodec::Msbc || fallback == inner.voice.offered {
                return;
            }
            inner.voice.offered = fallback.clone();
            inner.voice.codec = VoiceCodec::Cvsd;
            (inner.link.clone(), inner.voice.wanted, fallback)
        };
        warn!("wide-band voice could not be set up; the phone is offered narrow band only");

        let Some(link) = link else { return };
        if wanted {
            self.voice.expect(Some(VoiceCodec::Cvsd));
        }
        let mut asked = self.send(&link, &command::available_codecs(&codec_ids(&offered))).await;
        if wanted && asked.is_ok() {
            asked = self.send(&link, command::CODEC_CONNECTION).await;
        }
        if let Err(e) = asked {
            warn!(error = %e, "the phone was not asked for a narrow-band voice link");
        }
    }
}

/// Applies the edge's events to the connection until the edge stops serving the phone.
fn spawn_voice_events(control: &Arc<CallControl>, mut events: mpsc::UnboundedReceiver<VoiceEvent>, generation: u64) {
    let control = Arc::downgrade(control);
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            let Some(c) = control.upgrade() else { break };
            c.voice_event(generation, event).await;
        }
    });
}

#[cfg(test)]
mod tests;
