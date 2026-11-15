mod keys;
pub mod views;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use openh264::decoder::{DecodedYUV, Decoder};
use openh264::formats::YUVSource;
use phonelink_screen_mirroring::MirrorPresenter;
use phonelink_screen_mirroring::edge::{AudioStreamFormat, DisplayGeometry};

use crate::audio::{self, Player};

pub use keys::Keyboard;

pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

type Repaint = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Stream {
    decoder: Mutex<Option<Decoder>>,
    frame: Mutex<Option<Arc<Frame>>>,
    repaint: Mutex<Option<(u64, Repaint)>>,
    frames: AtomicU64,
    failures: AtomicU64,
}

const H264_NAL_SPS: u8 = 7;
const OPENH264_MAX_LEVEL: u8 = 51;

// OpenH264 refuses the phone's sequence parameter set at level 6.0 with dsNoParamSets and takes it
// at 5.1; the level only bounds decoder resources, so it is lowered in place.
fn with_supported_level(data: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    let levels: Vec<usize> = data
        .windows(7)
        .enumerate()
        .filter(|(_, w)| w[..3] == [0, 0, 1] && w[3] & 0x1F == H264_NAL_SPS && w[6] > OPENH264_MAX_LEVEL + 1)
        .map(|(i, _)| i + 6)
        .collect();
    if levels.is_empty() {
        return std::borrow::Cow::Borrowed(data);
    }
    let mut patched = data.to_vec();
    for index in levels {
        patched[index] = OPENH264_MAX_LEVEL;
    }
    std::borrow::Cow::Owned(patched)
}

// OpenH264 keeps the phone's parameter sets only when each NAL unit is decoded on its own.
pub(crate) fn decode_access_unit<T>(
    decoder: &mut Decoder,
    data: &[u8],
    convert: impl FnOnce(&DecodedYUV<'_>) -> T,
) -> Result<Option<T>, openh264::Error> {
    let data = with_supported_level(data);
    let units: Vec<&[u8]> = openh264::nal_units(&data).collect();
    let Some((last, leading)) = units.split_last() else { return Ok(None) };
    for unit in leading {
        decoder.decode(unit)?;
    }
    Ok(decoder.decode(last)?.map(|picture| convert(&picture)))
}

impl Stream {
    fn decode(&self, data: &[u8]) -> Option<Frame> {
        let count = self.frames.fetch_add(1, Ordering::Relaxed);
        let mut decoder = self.decoder.lock().unwrap_or_else(PoisonError::into_inner);
        if decoder.is_none() {
            *decoder = Decoder::new().inspect_err(|e| eprintln!("no H.264 decoder: {e}")).ok();
        }
        let decoded = decode_access_unit(decoder.as_mut()?, data, |picture| {
            let (width, height) = picture.dimensions();
            let mut rgba = vec![0; width * height * 4];
            picture.write_rgba8(&mut rgba);
            Frame { width, height, rgba }
        });
        match decoded {
            Ok(frame) => frame,
            Err(e) => {
                if self.failures.fetch_add(1, Ordering::Relaxed) < 8 {
                    eprintln!("mirroring frame {count} did not decode: {e}");
                }
                None
            }
        }
    }
}

pub struct LinuxMirrorPresenter {
    geometry: Mutex<DisplayGeometry>,
    streams: Mutex<HashMap<String, Arc<Stream>>>,
    players: Mutex<HashMap<String, Player>>,
}

pub fn presenter() -> &'static Arc<LinuxMirrorPresenter> {
    static PRESENTER: OnceLock<Arc<LinuxMirrorPresenter>> = OnceLock::new();
    PRESENTER.get_or_init(|| {
        Arc::new(LinuxMirrorPresenter {
            geometry: Mutex::new(DisplayGeometry {
                usable_width: 1920,
                usable_height: 1020,
                dpi: 96,
                width: 1920,
                height: 1080,
            }),
            streams: Mutex::default(),
            players: Mutex::default(),
        })
    })
}

impl LinuxMirrorPresenter {
    fn stream(&self, session_id: &str) -> Arc<Stream> {
        let mut streams = self.streams.lock().unwrap_or_else(PoisonError::into_inner);
        streams.entry(session_id.to_owned()).or_default().clone()
    }

    pub fn set_geometry(&self, geometry: DisplayGeometry) {
        *self.geometry.lock().unwrap_or_else(PoisonError::into_inner) = geometry;
    }

    pub fn attach(&self, session_id: &str, token: u64, repaint: Repaint) {
        *self.stream(session_id).repaint.lock().unwrap_or_else(PoisonError::into_inner) = Some((token, repaint));
    }

    pub fn detach(&self, session_id: &str, token: u64) {
        let stream = self.stream(session_id);
        let mut repaint = stream.repaint.lock().unwrap_or_else(PoisonError::into_inner);
        if repaint.as_ref().is_some_and(|(attached, _)| *attached == token) {
            *repaint = None;
        }
    }

    pub fn frame(&self, session_id: &str) -> Option<Arc<Frame>> {
        self.stream(session_id).frame.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn forget(&self, session_id: &str) {
        self.streams.lock().unwrap_or_else(PoisonError::into_inner).remove(session_id);
    }
}

impl MirrorPresenter for LinuxMirrorPresenter {
    fn display_geometry(&self) -> DisplayGeometry {
        *self.geometry.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn has_audio_output(&self) -> bool {
        audio::has_output()
    }

    fn video_frame(&self, session_id: String, data: Vec<u8>, _keyframe: bool) -> bool {
        let stream = self.stream(&session_id);
        let Some(frame) = stream.decode(&data) else { return false };
        let size = (frame.width, frame.height);
        let previous = stream.frame.lock().unwrap_or_else(PoisonError::into_inner).replace(Arc::new(frame));
        if previous.is_none_or(|previous| (previous.width, previous.height) != size) {
            eprintln!(
                "mirroring picture size {}x{} at frame {}",
                size.0,
                size.1,
                stream.frames.load(Ordering::Relaxed)
            );
        }
        let repaint = stream.repaint.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(|(_, r)| r.clone());
        let painted = repaint.inspect(|repaint| repaint()).is_some();
        if !painted {
            eprintln!("mirroring frame decoded with no window attached");
        }
        painted
    }

    fn reset_video(&self, session_id: String) {
        *self.stream(&session_id).decoder.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    fn audio_started(&self, package_name: String, format: AudioStreamFormat) {
        let player = Player::start(&package_name, format);
        self.players.lock().unwrap_or_else(PoisonError::into_inner).insert(package_name, player);
    }

    fn audio_frame(&self, package_name: String, data: Vec<u8>, _timestamp: u64) {
        if let Some(player) = self.players.lock().unwrap_or_else(PoisonError::into_inner).get(&package_name) {
            player.play(data);
        }
    }

    fn audio_ended(&self, package_name: String) {
        self.players.lock().unwrap_or_else(PoisonError::into_inner).remove(&package_name);
    }
}
