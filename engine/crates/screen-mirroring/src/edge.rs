//! Mirroring's OS edge: the Mac's display values the phone is told, its audio output and input
//! source, and the presentation of each window's video and each app's audio.
//!
//! The CLI uses [`NoPresenter`]; the tests use [`testing`].

/// The Mac's primary display (the one with the menu bar), in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Record)]
pub struct DisplayGeometry {
    /// The visible frame's width (without the menu bar and the Dock).
    pub usable_width: u32,
    /// The visible frame's height less a window's title bar.
    pub usable_height: u32,
    /// 96 × the backing scale factor.
    pub dpi: u32,
    pub width: u32,
    pub height: u32,
}

/// The audio a phone app streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct AudioStreamFormat {
    pub channels: u32,
    pub sample_rate: u32,
    /// 0 Opus, 1 PCM, 2 AAC.
    pub codec: u32,
    /// PCM only: bytes (not bits) per sample, 2–4.
    pub bytes_per_sample: u32,
    pub is_float: bool,
    /// The length of one frame.
    pub frame_length_ms: u32,
}

/// What mirroring needs from the Mac. Every method is called on an engine thread, never the main
/// thread, and may be called from several sessions at once.
#[uniffi::export(with_foreign)]
pub trait MirrorPresenter: Send + Sync {
    /// The primary display's values, read each time the phone is configured.
    fn display_geometry(&self) -> DisplayGeometry;
    /// An audio output device exists.
    fn has_audio_output(&self) -> bool;
    /// The current keyboard input source is an input method (not a plain layout).
    fn input_source_is_input_method(&self) -> bool;
    /// Decodes one H.264 access unit (Annex-B) of session `session_id`'s video and presents it in the
    /// session's window, scaled and rotated as the session's display state says. `true` when it
    /// decoded to a presented picture. Called in stream order, on a thread that may block.
    fn video_frame(&self, session_id: String, data: Vec<u8>, keyframe: bool) -> bool;
    /// Session `session_id`'s video ended: the next frames start a fresh decoder.
    fn reset_video(&self, session_id: String);
    /// App `package_name`'s audio starts with `format`.
    fn audio_started(&self, package_name: String, format: AudioStreamFormat);
    /// One encoded audio frame of `package_name`, in the order it arrived; `timestamp` is the phone's.
    fn audio_frame(&self, package_name: String, data: Vec<u8>, timestamp: u64);
    /// `package_name`'s audio ended.
    fn audio_ended(&self, package_name: String);
}

/// The CLI's presenter: a display of 1920 × 1080 at 96 dpi with nothing to present on.
#[derive(Debug, Default)]
pub struct NoPresenter;

impl MirrorPresenter for NoPresenter {
    fn display_geometry(&self) -> DisplayGeometry {
        DisplayGeometry { usable_width: 1920, usable_height: 1020, dpi: 96, width: 1920, height: 1080 }
    }
    fn has_audio_output(&self) -> bool {
        false
    }
    fn input_source_is_input_method(&self) -> bool {
        false
    }
    fn video_frame(&self, _session_id: String, _data: Vec<u8>, _keyframe: bool) -> bool {
        false
    }
    fn reset_video(&self, _session_id: String) {}
    fn audio_started(&self, _package_name: String, _format: AudioStreamFormat) {}
    fn audio_frame(&self, _package_name: String, _data: Vec<u8>, _timestamp: u64) {}
    fn audio_ended(&self, _package_name: String) {}
}

/// In-memory stand-ins for tests.
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use phonelink_core::sync::MutexExt;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use super::{AudioStreamFormat, DisplayGeometry, MirrorPresenter};

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum PresenterCall {
        Frame(String, Vec<u8>),
        Reset(String),
        AudioStarted(String, AudioStreamFormat),
        AudioFrame(String, Vec<u8>),
        AudioEnded(String),
    }

    /// A presenter that records every call; a frame decodes unless its bytes are empty.
    pub struct RecordingPresenter {
        pub geometry: Mutex<DisplayGeometry>,
        pub audio_output: AtomicBool,
        pub input_method: AtomicBool,
        pub calls: Mutex<Vec<PresenterCall>>,
    }

    impl RecordingPresenter {
        pub fn new() -> Arc<Self> {
            Arc::new(Self {
                geometry: Mutex::new(DisplayGeometry {
                    usable_width: 2880,
                    usable_height: 1700,
                    dpi: 192,
                    width: 2880,
                    height: 1800,
                }),
                audio_output: AtomicBool::new(true),
                input_method: AtomicBool::new(false),
                calls: Mutex::new(Vec::new()),
            })
        }

        pub fn calls(&self) -> Vec<PresenterCall> {
            self.calls.lock_or_panic().clone()
        }

        fn record(&self, c: PresenterCall) {
            self.calls.lock_or_panic().push(c);
        }
    }

    impl MirrorPresenter for RecordingPresenter {
        fn display_geometry(&self) -> DisplayGeometry {
            *self.geometry.lock_or_panic()
        }
        fn has_audio_output(&self) -> bool {
            self.audio_output.load(Ordering::SeqCst)
        }
        fn input_source_is_input_method(&self) -> bool {
            self.input_method.load(Ordering::SeqCst)
        }
        fn video_frame(&self, session_id: String, data: Vec<u8>, _keyframe: bool) -> bool {
            let decoded = !data.is_empty();
            self.record(PresenterCall::Frame(session_id, data));
            decoded
        }
        fn reset_video(&self, session_id: String) {
            self.record(PresenterCall::Reset(session_id));
        }
        fn audio_started(&self, package_name: String, format: AudioStreamFormat) {
            self.record(PresenterCall::AudioStarted(package_name, format));
        }
        fn audio_frame(&self, package_name: String, data: Vec<u8>, _timestamp: u64) {
            self.record(PresenterCall::AudioFrame(package_name, data));
        }
        fn audio_ended(&self, package_name: String) {
            self.record(PresenterCall::AudioEnded(package_name));
        }
    }
}
