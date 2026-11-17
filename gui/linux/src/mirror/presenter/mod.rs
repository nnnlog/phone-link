//! The mirroring presenter: each window's video decoded to the picture it paints, the primary screen's
//! values the phone is told, and each mirrored app's audio played.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use openh264::decoder::Decoder;
use openh264::formats::YUVSource;
use phonelink_core::sync::MutexExt;
use phonelink_screen_mirroring::MirrorPresenter;
use phonelink_screen_mirroring::edge::{AudioStreamFormat, DisplayGeometry};

use super::decode_access_unit;
use crate::audio::{self, Player};

/// A decoded picture of a window's video, in RGBA.
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

type Repaint = Arc<dyn Fn() + Send + Sync>;

/// Reads the primary screen's values (`primaryDisplayGeometry` in desktop.h): its usable width and
/// height, dpi, width and height in device pixels, as last read on the GUI thread; empty before then.
pub type ScreenReader = fn() -> Vec<i32>;

/// What the phone is told while no screen was read: a display of 1920 × 1080 at 96 dpi.
const UNREAD_DISPLAY: DisplayGeometry =
    DisplayGeometry { usable_width: 1920, usable_height: 1020, dpi: 96, width: 1920, height: 1080 };

/// The display values in `values` (usable width and height, dpi, width, height).
///
/// `None` unless there are five of them, none negative.
pub fn display_geometry(values: &[i32]) -> Option<DisplayGeometry> {
    let [usable_width, usable_height, dpi, width, height] = values else { return None };
    let pixels = |value: &i32| u32::try_from(*value).ok();
    Some(DisplayGeometry {
        usable_width: pixels(usable_width)?,
        usable_height: pixels(usable_height)?,
        dpi: pixels(dpi)?,
        width: pixels(width)?,
        height: pixels(height)?,
    })
}

/// One session's video: its decoder, the last picture, and the window that repaints it.
#[derive(Default)]
struct Stream {
    decoder: Mutex<Option<Decoder>>,
    frame: Mutex<Option<Arc<Frame>>>,
    repaint: Mutex<Option<(u64, Repaint)>>,
    frames: AtomicU64,
    failures: AtomicU64,
}

impl Stream {
    fn decode(&self, data: &[u8]) -> Option<Frame> {
        let count = self.frames.fetch_add(1, Ordering::Relaxed);
        let mut decoder = self.decoder.lock_or_recover();
        if decoder.is_none() {
            *decoder = Decoder::new().inspect_err(|e| tracing::warn!(error = %e, "no H.264 decoder")).ok();
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
                    tracing::warn!(index = count, error = %e, "a mirroring frame did not decode");
                }
                None
            }
        }
    }
}

/// The engine's mirroring presenter on this computer: the sessions' video and the apps' audio.
#[derive(Default)]
pub struct LinuxMirrorPresenter {
    screen: OnceLock<ScreenReader>,
    streams: Mutex<HashMap<String, Arc<Stream>>>,
    players: Mutex<HashMap<String, Player>>,
}

/// The app's one presenter.
pub fn presenter() -> &'static Arc<LinuxMirrorPresenter> {
    static PRESENTER: OnceLock<Arc<LinuxMirrorPresenter>> = OnceLock::new();
    PRESENTER.get_or_init(|| {
        // Whether an output device exists is known before the phone's audio starts.
        audio::watch_outputs();
        Arc::default()
    })
}

impl LinuxMirrorPresenter {
    /// The stream of `session_id`, made when missing.
    fn stream(&self, session_id: &str) -> Arc<Stream> {
        let mut streams = self.streams.lock_or_recover();
        streams.entry(session_id.to_owned()).or_default().clone()
    }

    /// The stream of `session_id` if it has one: a closed session's is not made again.
    fn existing(&self, session_id: &str) -> Option<Arc<Stream>> {
        self.streams.lock_or_recover().get(session_id).cloned()
    }

    /// The phone is told the primary screen's values as `reader` gives them, from the first call on.
    pub fn read_screen_with(&self, reader: ScreenReader) {
        // Later calls give the same reader.
        let _ = self.screen.set(reader);
    }

    /// Has `repaint` called with each new picture of `session_id`, for the window `token` names.
    pub fn attach(&self, session_id: &str, token: u64, repaint: Repaint) {
        *self.stream(session_id).repaint.lock_or_recover() = Some((token, repaint));
    }

    /// The window `token` names no longer paints `session_id`; a newer window's repaint stays.
    pub fn detach(&self, session_id: &str, token: u64) {
        let Some(stream) = self.existing(session_id) else { return };
        let mut repaint = stream.repaint.lock_or_recover();
        if repaint.as_ref().is_some_and(|(attached, _)| *attached == token) {
            *repaint = None;
        }
    }

    /// The last picture of `session_id`.
    pub fn frame(&self, session_id: &str) -> Option<Arc<Frame>> {
        self.existing(session_id)?.frame.lock_or_recover().clone()
    }

    /// Drops the stream of a closed session.
    pub fn forget(&self, session_id: &str) {
        self.streams.lock_or_recover().remove(session_id);
    }
}

impl MirrorPresenter for LinuxMirrorPresenter {
    /// The primary screen's values as the GUI thread read them last (desktop.h reads them again when the
    /// screen changes, and the windows have them read once they are framed).
    fn display_geometry(&self) -> DisplayGeometry {
        let read = self.screen.get().map(|reader| reader()).unwrap_or_default();
        display_geometry(&read).unwrap_or_else(|| {
            tracing::warn!("no primary screen was read; the phone is told of a 1920 × 1080 display");
            UNREAD_DISPLAY
        })
    }

    fn has_audio_output(&self) -> bool {
        audio::has_output()
    }

    fn video_frame(&self, session_id: String, data: Vec<u8>, _keyframe: bool) -> bool {
        let stream = self.stream(&session_id);
        let Some(frame) = stream.decode(&data) else { return false };

        let size = (frame.width, frame.height);
        let previous = stream.frame.lock_or_recover().replace(Arc::new(frame));
        if previous.is_none_or(|previous| (previous.width, previous.height) != size) {
            tracing::info!(
                width = size.0,
                height = size.1,
                frame_count = stream.frames.load(Ordering::Relaxed),
                "mirroring picture size"
            );
        }

        let repaint = stream.repaint.lock_or_recover().as_ref().map(|(_, r)| r.clone());
        let painted = repaint.inspect(|repaint| repaint()).is_some();
        if !painted {
            tracing::debug!("mirroring frame decoded with no window attached");
        }
        painted
    }

    fn reset_video(&self, session_id: String) {
        if let Some(stream) = self.existing(&session_id) {
            *stream.decoder.lock_or_recover() = None;
        }
    }

    fn audio_started(&self, package_name: String, format: AudioStreamFormat) {
        let player = Player::start(&package_name, format);
        // A player it replaces drops what it still holds.
        let replaced = self.players.lock_or_recover().insert(package_name, player);
        drop(replaced);
    }

    fn audio_frame(&self, package_name: String, data: Vec<u8>, _timestamp: u64) {
        if let Some(player) = self.players.lock_or_recover().get(&package_name) {
            player.play(data);
        }
    }

    fn audio_ended(&self, package_name: String) {
        self.players.lock_or_recover().remove(&package_name);
    }
}

#[cfg(test)]
mod tests;
