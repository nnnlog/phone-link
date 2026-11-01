//! The connected camera's OS edge: the Mac's system camera that shows the phone.
//!
//! The edge publishes one camera per registered phone, shows still images on it, and decodes the
//! phone's H.264 frames onto it. It reports the camera's clients starting and stopping through
//! [`super::ConnectedCamera::client_changed`]. The CLI uses [`NoCamera`]; the tests use
//! [`testing`].

use phonelink_core::edge::EdgeError;

/// A still image the camera shows while no video does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum CameraPlaceholder {
    Black,
    /// The connecting animation: 16 images, one every 200 ms, repeated.
    Connecting,
    Paused,
    PhoneLocked,
    Disconnected,
    /// "Tap the notification on your phone to start".
    StartStreamNotification,
    Error,
}

/// The Mac's camera for phones.
#[uniffi::export(with_foreign)]
pub trait PhoneCamera: Send + Sync {
    /// Cameras can be published: the camera extension is installed and running on a macOS that
    /// supports it.
    fn is_available(&self) -> bool;
    /// Publishes camera `device_id` named `name`: NV12, 1280 × 720 at 30 fps, showing black.
    fn publish(&self, device_id: String, name: String) -> Result<(), EdgeError>;
    /// Removes camera `device_id`; nothing when it is not published.
    fn remove(&self, device_id: String);
    /// Shows `placeholder` on camera `device_id` until the next image or frame.
    fn show_placeholder(&self, device_id: String, placeholder: CameraPlaceholder);
    /// Decodes one H.264 access unit (Annex-B) of `device_id`'s stream and, when `show`, puts the
    /// picture on the camera at 1280 × 720. `true` when the unit decoded to a picture. Called in
    /// stream order, on a thread that may block.
    fn video_frame(&self, device_id: String, data: Vec<u8>, show: bool) -> bool;
    /// `device_id`'s stream ended: the next stream starts with a fresh decoder.
    fn reset_decoder(&self, device_id: String);
}

/// The CLI's camera: none.
#[derive(Debug, Default)]
pub struct NoCamera;

impl PhoneCamera for NoCamera {
    fn is_available(&self) -> bool {
        false
    }
    fn publish(&self, _device_id: String, _name: String) -> Result<(), EdgeError> {
        Err(EdgeError::Unexpected { message: "no camera on this host".into() })
    }
    fn remove(&self, _device_id: String) {}
    fn show_placeholder(&self, _device_id: String, _placeholder: CameraPlaceholder) {}
    fn video_frame(&self, _device_id: String, _data: Vec<u8>, _show: bool) -> bool {
        false
    }
    fn reset_decoder(&self, _device_id: String) {}
}

/// In-memory stand-ins for tests.
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use phonelink_core::sync::MutexExt;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use super::{CameraPlaceholder, PhoneCamera};

    use phonelink_core::edge::EdgeError;

    /// What the feature did to the camera.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum CameraCall {
        Register(String, String),
        Unregister(String),
        Placeholder(String, CameraPlaceholder),
        /// A frame's bytes and whether it was shown.
        Frame(Vec<u8>, bool),
        Reset(String),
    }

    /// A camera that records every call; frames decode unless their bytes are empty.
    pub struct RecordingCamera {
        pub available: AtomicBool,
        pub fail_register: AtomicBool,
        pub calls: Mutex<Vec<CameraCall>>,
    }

    impl RecordingCamera {
        pub fn new() -> Arc<Self> {
            Arc::new(Self {
                available: AtomicBool::new(true),
                fail_register: AtomicBool::new(false),
                calls: Mutex::new(Vec::new()),
            })
        }

        pub fn calls(&self) -> Vec<CameraCall> {
            self.calls.lock_or_panic().clone()
        }

        pub fn take(&self) -> Vec<CameraCall> {
            std::mem::take(&mut *self.calls.lock_or_panic())
        }

        fn record(&self, call: CameraCall) {
            self.calls.lock_or_panic().push(call);
        }
    }

    impl PhoneCamera for RecordingCamera {
        fn is_available(&self) -> bool {
            self.available.load(Ordering::SeqCst)
        }
        fn publish(&self, device_id: String, name: String) -> Result<(), EdgeError> {
            if self.fail_register.load(Ordering::SeqCst) {
                return Err(EdgeError::Unexpected { message: "refused".into() });
            }
            self.record(CameraCall::Register(device_id, name));
            Ok(())
        }
        fn remove(&self, device_id: String) {
            self.record(CameraCall::Unregister(device_id));
        }
        fn show_placeholder(&self, device_id: String, placeholder: CameraPlaceholder) {
            self.record(CameraCall::Placeholder(device_id, placeholder));
        }
        fn video_frame(&self, _device_id: String, data: Vec<u8>, show: bool) -> bool {
            let decoded = !data.is_empty();
            self.record(CameraCall::Frame(data, show));
            decoded
        }
        fn reset_decoder(&self, device_id: String) {
            self.record(CameraCall::Reset(device_id));
        }
    }
}
