//! A scripted voice edge for call-control tests: the test plays the phone opening and closing the
//! link, and reads what call control asked of the computer.

use std::sync::{Arc, Mutex};

use phonelink_core::sync::MutexExt;
use tokio::sync::mpsc;

use super::{PhoneVoice, VoiceCodec, VoiceError, VoiceEvent};

#[derive(Default)]
struct Script {
    codecs: Vec<VoiceCodec>,
    can_carry: bool,
    events: Option<mpsc::UnboundedSender<VoiceEvent>>,
    attached: Option<String>,
    expected: Option<VoiceCodec>,
    open: Option<VoiceCodec>,
    failing: Option<VoiceCodec>,
    muted: bool,
    keys: Vec<char>,
    declined: u32,
    connects: Vec<VoiceCodec>,
}

impl Script {
    fn tell(&self, event: VoiceEvent) {
        if let Some(events) = &self.events {
            // No receiver means call control already closed the connection.
            let _ = events.send(event);
        }
    }
}

/// The scripted voice edge.
pub struct FakeVoice {
    script: Mutex<Script>,
}

impl FakeVoice {
    /// A computer with both codecs, an output and a microphone.
    pub fn new() -> Arc<Self> {
        let script = Script { codecs: vec![VoiceCodec::Msbc, VoiceCodec::Cvsd], can_carry: true, ..Script::default() };
        Arc::new(Self { script: Mutex::new(script) })
    }

    /// The computer gains or loses its output or microphone.
    pub fn set_can_carry(&self, can_carry: bool) {
        let mut s = self.script.lock_or_panic();
        s.can_carry = can_carry;
        s.tell(VoiceEvent::CarryChanged);
    }

    /// Links with `codec` fail to set up from now on.
    pub fn fail(&self, codec: VoiceCodec) {
        self.script.lock_or_panic().failing = Some(codec);
    }

    /// The phone opens a link with `codec`: `true` when the computer accepted it.
    pub fn phone_opens(&self, codec: VoiceCodec) -> bool {
        let mut s = self.script.lock_or_panic();
        if s.attached.is_none() || s.expected != Some(codec) {
            s.declined += 1;
            return false;
        }
        if s.failing == Some(codec) {
            s.tell(VoiceEvent::CodecFailed(codec));
            return false;
        }

        s.open = Some(codec);
        s.tell(VoiceEvent::Opened);
        true
    }

    /// The phone releases the link.
    pub fn phone_closes(&self) {
        let mut s = self.script.lock_or_panic();
        if s.open.take().is_some() {
            s.tell(VoiceEvent::Closed);
        }
    }

    /// The open link's codec.
    pub fn open(&self) -> Option<VoiceCodec> {
        self.script.lock_or_panic().open
    }

    /// The codec the computer accepts the phone's next link with.
    pub fn expected(&self) -> Option<VoiceCodec> {
        self.script.lock_or_panic().expected
    }

    /// The address of the phone the edge serves.
    pub fn attached(&self) -> Option<String> {
        self.script.lock_or_panic().attached.clone()
    }

    pub fn muted(&self) -> bool {
        self.script.lock_or_panic().muted
    }

    /// The keys whose tone the computer played, in order.
    pub fn keys(&self) -> Vec<char> {
        self.script.lock_or_panic().keys.clone()
    }

    /// How many of the phone's links the computer declined.
    pub fn declined(&self) -> u32 {
        self.script.lock_or_panic().declined
    }

    /// The codecs of the links the computer opened itself.
    pub fn connects(&self) -> Vec<VoiceCodec> {
        self.script.lock_or_panic().connects.clone()
    }
}

impl PhoneVoice for FakeVoice {
    fn codecs(&self) -> Vec<VoiceCodec> {
        self.script.lock_or_panic().codecs.clone()
    }
    fn can_carry(&self) -> bool {
        self.script.lock_or_panic().can_carry
    }
    fn attach(&self, address: String, events: mpsc::UnboundedSender<VoiceEvent>) {
        let mut s = self.script.lock_or_panic();
        s.attached = Some(address);
        s.events = Some(events);
        s.expected = None;
    }
    fn detach(&self) {
        let mut s = self.script.lock_or_panic();
        s.attached = None;
        s.open = None;
        s.events = None;
    }
    fn expect(&self, codec: Option<VoiceCodec>) {
        self.script.lock_or_panic().expected = codec;
    }
    fn connect(&self, codec: VoiceCodec) -> Result<(), VoiceError> {
        let mut s = self.script.lock_or_panic();
        s.connects.push(codec);
        if s.failing == Some(codec) {
            return Err(VoiceError { reason: "scripted failure".into() });
        }

        s.open = Some(codec);
        s.tell(VoiceEvent::Opened);
        Ok(())
    }
    fn close(&self) {
        let mut s = self.script.lock_or_panic();
        if s.open.take().is_some() {
            s.tell(VoiceEvent::Closed);
        }
    }
    fn set_muted(&self, muted: bool) {
        self.script.lock_or_panic().muted = muted;
    }
    fn play_key(&self, key: char) {
        self.script.lock_or_panic().keys.push(key);
    }
}
