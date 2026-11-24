//! The voice link's edge: the synchronous Bluetooth link a call's voice runs on, and its sound on the
//! computer.
//!
//! A computer that can carry a call implements [`PhoneVoice`]; one that cannot uses
//! [`standins::NoVoice`], and call control then leaves every call's audio on the phone. The edge is
//! synchronous; call control calls what can block on the blocking pool.

pub mod standins;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

use tokio::sync::mpsc;

/// A voice link's codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceCodec {
    /// Narrow band, 8 kHz; the adapter codes it.
    Cvsd,
    /// Wide band, 16 kHz; the computer codes it.
    Msbc,
}

impl VoiceCodec {
    /// The codec's Hands-Free id, as `AT+BAC` and `+BCS` carry it.
    pub fn id(self) -> u8 {
        match self {
            VoiceCodec::Cvsd => 1,
            VoiceCodec::Msbc => 2,
        }
    }

    pub fn from_id(id: u8) -> Option<Self> {
        match id {
            1 => Some(VoiceCodec::Cvsd),
            2 => Some(VoiceCodec::Msbc),
            _ => None,
        }
    }
}

/// What the edge tells call control about the phone it is attached to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceEvent {
    /// A voice link is open and carries the call.
    Opened,
    /// The open voice link ended, from either side.
    Closed,
    /// A link with this codec could not be set up on the computer.
    CodecFailed(VoiceCodec),
    /// What [`PhoneVoice::can_carry`] reports changed.
    CarryChanged,
}

/// Why the computer could not open a voice link itself.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the voice link did not open: {reason}")]
pub struct VoiceError {
    pub reason: String,
}

/// The computer's side of the voice link to one phone at a time.
pub trait PhoneVoice: Send + Sync {
    /// The codecs the computer carries, the preferred one first; none when it carries no voice.
    fn codecs(&self) -> Vec<VoiceCodec>;

    /// Whether a call can be heard and spoken on the computer now: it has a sound output and a
    /// microphone.
    fn can_carry(&self) -> bool;

    /// Serves the phone at `address` until [`PhoneVoice::detach`]: its voice links are answered as
    /// [`PhoneVoice::expect`] last said (declined at first), and `events` hears what happens.
    fn attach(&self, address: String, events: mpsc::UnboundedSender<VoiceEvent>);

    /// Stops serving the phone; an open link is released.
    fn detach(&self);

    /// A link the phone opens from now on is accepted with `codec`; with `None` it is declined.
    fn expect(&self, codec: Option<VoiceCodec>);

    /// Opens a link to the phone from the computer, for a phone that does not negotiate codecs.
    /// Blocks until the link is open or failed.
    fn connect(&self, codec: VoiceCodec) -> Result<(), VoiceError>;

    /// Releases the open link, if any.
    fn close(&self);

    /// While muted, the computer sends silence in place of the microphone.
    fn set_muted(&self, muted: bool);

    /// Plays the tone of a keypad key (`0`–`9`, `*`, `#`) on the computer's output.
    fn play_key(&self, key: char);
}
