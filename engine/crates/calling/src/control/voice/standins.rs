//! The voice edge of a computer that carries no call.

use tokio::sync::mpsc;

use super::{PhoneVoice, VoiceCodec, VoiceError, VoiceEvent};

/// No voice link: every call's audio stays on the phone.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoVoice;

impl PhoneVoice for NoVoice {
    fn codecs(&self) -> Vec<VoiceCodec> {
        Vec::new()
    }
    fn can_carry(&self) -> bool {
        false
    }
    fn attach(&self, _address: String, _events: mpsc::UnboundedSender<VoiceEvent>) {}
    fn detach(&self) {}
    fn expect(&self, _codec: Option<VoiceCodec>) {}
    fn connect(&self, _codec: VoiceCodec) -> Result<(), VoiceError> {
        Err(VoiceError { reason: "this computer carries no voice link".into() })
    }
    fn close(&self) {}
    fn set_muted(&self, _muted: bool) {}
    fn play_key(&self, _key: char) {}
}
