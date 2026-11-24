//! The engine's voice edge on `BlueZ`: the phone's voice links answered on a listening socket, the
//! link run on a thread of its own, and the call's sound on the audio server.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use phonelink_calling::control::voice::{PhoneVoice, VoiceCodec, VoiceError, VoiceEvent};
use phonelink_core::sync::MutexExt;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, info, warn};

use super::carry::{Sound, exchange_packets};
use super::exchange::Exchange;
use super::sbc::Msbc;
use super::sco::{self, Address, Asked, Link, Listener, Voice};
use super::sound::{self, Microphone};
use crate::audio::{self, Player};
use crate::bluetooth::BluezBluetooth;

/// How long the listener and a link wait before they look whether they were stopped.
const STOP_POLL: Duration = Duration::from_millis(100);
/// How long a link may take to connect once it is accepted or asked for.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The length the far end's sound is handed to its player in, for the player's queue (one wide-band
/// frame, rounded up).
const HEARD_FRAME_MS: u32 = 8;
/// Room for a packet of an adapter that reports no packet length.
const PACKET_ROOM: usize = 256;

/// The phone the edge serves.
#[derive(Default)]
struct Serving {
    events: Option<UnboundedSender<VoiceEvent>>,
    adapter: Option<Address>,
    phone: Option<Address>,
    /// The codec the phone's next link is accepted with; `None` declines it.
    expected: Option<VoiceCodec>,
    /// Set to stop the listener's thread.
    listener_stopped: Option<Arc<AtomicBool>>,
    /// Set to end the open link.
    link_stopped: Option<Arc<AtomicBool>>,
}

impl Serving {
    fn tell(&self, event: VoiceEvent) {
        if let Some(events) = &self.events {
            // No receiver means the engine closed the connection.
            let _ = events.send(event);
        }
    }
}

#[derive(Default)]
struct Shared {
    serving: Mutex<Serving>,
    muted: AtomicBool,
}

/// The voice edge.
pub(crate) struct BluezVoice {
    bluetooth: Arc<BluezBluetooth>,
    shared: Arc<Shared>,
}

impl BluezVoice {
    /// The edge on the adapter `bluetooth` uses. The audio server's devices are followed from here on,
    /// so the engine hears when a call can or can no longer be taken.
    pub(crate) fn new(bluetooth: Arc<BluezBluetooth>) -> Self {
        let shared = Arc::new(Shared::default());

        audio::watch_outputs();
        let told = Arc::downgrade(&shared);
        audio::on_devices_changed(move || {
            if let Some(shared) = told.upgrade() {
                shared.serving.lock_or_recover().tell(VoiceEvent::CarryChanged);
            }
        });
        Self { bluetooth, shared }
    }
}

impl PhoneVoice for BluezVoice {
    fn codecs(&self) -> Vec<VoiceCodec> {
        vec![VoiceCodec::Msbc, VoiceCodec::Cvsd]
    }

    fn can_carry(&self) -> bool {
        audio::has_output() && audio::has_microphone()
    }

    fn attach(&self, address: String, events: UnboundedSender<VoiceEvent>) {
        self.detach();

        let adapter = self.bluetooth.adapter_address().as_deref().and_then(Address::parse);
        let phone = Address::parse(&address);
        let stopped = Arc::new(AtomicBool::new(false));
        {
            let mut serving = self.shared.serving.lock_or_recover();
            *serving = Serving {
                events: Some(events),
                adapter,
                phone,
                listener_stopped: Some(stopped.clone()),
                ..Serving::default()
            };
        }

        let listening = adapter
            .ok_or_else(|| io::Error::other("no Bluetooth adapter address"))
            .and_then(Listener::bind)
            .and_then(|listener| {
                let shared = self.shared.clone();
                std::thread::Builder::new()
                    .name("call-voice-listen".to_owned())
                    .spawn(move || listen(&shared, &listener, &stopped))
            });
        match listening {
            Ok(_) => info!("the phone's voice links are answered"),
            Err(e) => warn!(error = %e, "the phone's voice links are not answered; calls stay on the phone"),
        }
    }

    fn detach(&self) {
        let serving = std::mem::take(&mut *self.shared.serving.lock_or_recover());
        for stopped in [serving.listener_stopped, serving.link_stopped].into_iter().flatten() {
            stopped.store(true, Ordering::SeqCst);
        }
        self.shared.muted.store(false, Ordering::SeqCst);
    }

    fn expect(&self, codec: Option<VoiceCodec>) {
        self.shared.serving.lock_or_recover().expected = codec;
    }

    fn connect(&self, codec: VoiceCodec) -> Result<(), VoiceError> {
        let (adapter, phone) = {
            let serving = self.shared.serving.lock_or_recover();
            (serving.adapter, serving.phone)
        };
        let (Some(adapter), Some(phone)) = (adapter, phone) else {
            return Err(VoiceError { reason: "no phone is served".into() });
        };

        let exchange =
            exchange_for(codec).ok_or_else(|| VoiceError { reason: "the voice codec did not start".into() })?;
        let link = sco::connect(adapter, phone, voice_of(codec), CONNECT_TIMEOUT)
            .map_err(|e| VoiceError { reason: e.to_string() })?;
        self.shared.carry(link, exchange);
        Ok(())
    }

    fn close(&self) {
        if let Some(stopped) = &self.shared.serving.lock_or_recover().link_stopped {
            stopped.store(true, Ordering::SeqCst);
        }
    }

    fn set_muted(&self, muted: bool) {
        self.shared.muted.store(muted, Ordering::SeqCst);
    }

    fn play_key(&self, key: char) {
        sound::play_key(key);
    }
}

fn voice_of(codec: VoiceCodec) -> Voice {
    match codec {
        VoiceCodec::Cvsd => Voice::Coded,
        VoiceCodec::Msbc => Voice::Transparent,
    }
}

/// The exchange of a link with `codec`; `None` when the wide-band codec does not start.
fn exchange_for(codec: VoiceCodec) -> Option<Exchange> {
    match codec {
        VoiceCodec::Cvsd => Some(Exchange::narrow_band()),
        VoiceCodec::Msbc => Msbc::new().map(|codec| Exchange::wide_band(Box::new(codec))),
    }
}

/// Answers the links devices ask for until `stopped`.
fn listen(shared: &Arc<Shared>, listener: &Listener, stopped: &AtomicBool) {
    while !stopped.load(Ordering::SeqCst) {
        match listener.next(STOP_POLL) {
            Ok(None) => {}
            Ok(Some(asked)) => shared.answer(asked),
            Err(e) => {
                warn!(error = %e, "the phone's voice links are no longer answered; calls stay on the phone");
                return;
            }
        }
    }
}

impl Shared {
    /// Accepts the link `asked` when it is the phone's and the engine expects one; otherwise declines
    /// it by dropping it.
    fn answer(self: &Arc<Self>, asked: Asked) {
        let (expected, busy, ours) = {
            let serving = self.serving.lock_or_recover();
            (serving.expected, serving.link_stopped.is_some(), serving.phone == Some(asked.peer()))
        };
        let Some(codec) = expected.filter(|_| ours && !busy) else {
            debug!(is_expected = expected.is_some(), is_phone = ours, is_busy = busy, "a voice link was declined");
            return;
        };

        let accepted = exchange_for(codec)
            .ok_or_else(|| io::Error::other("the voice codec did not start"))
            .and_then(|exchange| Ok((asked.accept(voice_of(codec), CONNECT_TIMEOUT)?, exchange)));
        match accepted {
            Ok((link, exchange)) => self.carry(link, exchange),
            Err(e) => {
                warn!(error = %e, codec = ?codec, "the phone's voice link was not set up");
                self.serving.lock_or_recover().tell(VoiceEvent::CodecFailed(codec));
            }
        }
    }

    /// Runs `link` on a thread of its own until it ends or is stopped, telling the engine when it
    /// opens and closes.
    fn carry(self: &Arc<Self>, link: Link, exchange: Exchange) {
        let stopped = Arc::new(AtomicBool::new(false));
        {
            let mut serving = self.serving.lock_or_recover();
            serving.link_stopped = Some(stopped.clone());
            serving.tell(VoiceEvent::Opened);
        }

        let shared = self.clone();
        let running = stopped.clone();
        let spawned = std::thread::Builder::new().name("call-voice".to_owned()).spawn(move || {
            match carry_call(link, exchange, &running, &shared.muted) {
                Ok(()) => info!("the voice link ended"),
                Err(e) => warn!(error = %e, "the voice link failed"),
            }
            shared.ended(&running);
        });
        if let Err(e) = spawned {
            warn!(error = %e, "the voice link has no thread to run on");
            self.ended(&stopped);
        }
    }

    /// The link whose stop flag is `stopped` ended: the engine is told, unless it serves another
    /// phone or link by now.
    fn ended(&self, stopped: &Arc<AtomicBool>) {
        let mut serving = self.serving.lock_or_recover();
        if serving.link_stopped.as_ref().is_some_and(|current| Arc::ptr_eq(current, stopped)) {
            serving.link_stopped = None;
            serving.tell(VoiceEvent::Closed);
        }
    }
}

/// Carries the call on `link` until it ends or `stopped` is set: what the phone sends plays on the
/// default output, and the microphone's samples (silence while `muted`) go back.
fn carry_call(mut link: Link, mut exchange: Exchange, stopped: &AtomicBool, muted: &AtomicBool) -> io::Result<()> {
    let rate = exchange.sample_rate();
    let speaker = Player::start_voice(sound::stream_name(), rate, HEARD_FRAME_MS);
    let microphone = Microphone::start(rate);
    let room = link.packet_len().max(PACKET_ROOM);
    info!(sample_rate = rate, packet_len = link.packet_len(), "the voice link carries the call");

    let sound = Sound {
        heard: &mut |samples| speaker.play(samples.iter().flat_map(|sample| sample.to_le_bytes()).collect()),
        spoken: &mut |count| microphone.take(count, muted.load(Ordering::SeqCst)),
    };
    exchange_packets(&mut link, room, &mut exchange, sound, stopped, STOP_POLL)
}
