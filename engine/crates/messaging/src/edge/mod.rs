//! The messaging feature's OS edges.
//!
//! It covers attachment imaging (`ImageIO`) for the carrier limits and content-type detection, the
//! file-type lookups (`UTType`) behind the download table and `partNames`, and the Bluetooth
//! Classic proximity state (`IOBluetooth`) of the `0x80` gate.
//!
//! They are
//! synchronous `uniffi` foreign traits like [`phonelink_core::edge`]; the GUI implements them in Swift,
//! [`testing`] holds in-memory stand-ins.

use phonelink_core::edge::EdgeError;

/// An image re-encoded to fit the carrier limits.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FittedImage {
    pub bytes: Vec<u8>,
    /// The MIME type of `bytes` as produced (sent as `partContentTypes`).
    pub content_type: String,
}

/// Image decoding, scaling and re-encoding for outgoing attachments (macOS: `ImageIO` / Core
/// Graphics).
#[uniffi::export(with_foreign)]
pub trait AttachmentImaging: Send + Sync {
    /// The MIME type of `bytes` detected from their content (macOS: `CGImageSourceCreateWithData`,
    /// `CGImageSourceGetType`, then `UTType(identifier).preferredMIMEType`); `None` when the content
    /// is not recognised.
    fn sniff_content_type(&self, bytes: Vec<u8>) -> Option<String>;

    /// Fits an image to the carrier limits: scale it, keeping its aspect ratio and never enlarging,
    /// to fit within `max_width × max_height` pixels (macOS: `CGImageSourceCreateThumbnailAtIndex`
    /// with `kCGImageSourceThumbnailMaxPixelSize`), then re-encode it so it is at most `max_bytes`
    /// long (macOS: `CGImageDestination` with decreasing `kCGImageDestinationLossyCompressionQuality`),
    /// keeping the source format where the format allows it and reporting the MIME type of the bytes
    /// produced. An image already within both limits comes back unchanged. `Ok(None)`: the bytes are
    /// not an image the edge can transform (the attachment is then sent as it is).
    fn fit_image(
        &self,
        bytes: Vec<u8>,
        max_width: u32,
        max_height: u32,
        max_bytes: u64,
    ) -> Result<Option<FittedImage>, EdgeError>;
}

/// The family a MIME type belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum FileFamily {
    Image,
    Audio,
    Video,
    Document,
    VCard,
    Text,
    Other,
}

/// MIME-type lookups (macOS: `UTType`).
#[uniffi::export(with_foreign)]
pub trait MessagingFileTypes: Send + Sync {
    /// The family of `content_type` (macOS: `UTType(mimeType:)`, tested in this order: conforms to
    /// `.vCard` → `VCard`, `.image` → `Image`, `.audio` → `Audio`, `.movie` → `Video`, `.text` →
    /// `Text`, `.pdf` / `.presentation` / `.spreadsheet` / word-processing documents → `Document`, else
    /// `Other`; an unknown MIME type is `Other`).
    fn family(&self, content_type: String) -> FileFamily;

    /// The preferred file-name extension of `content_type` **with its leading dot** (macOS:
    /// `"." + UTType(mimeType:).preferredFilenameExtension`); `None` when there is none.
    fn file_extension(&self, content_type: String) -> Option<String>;
}

/// The Bluetooth Classic connection state of the phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum ProximityState {
    /// A Bluetooth Classic connection to the phone is active.
    Connected,
    /// The phone is paired but not connected.
    NotConnected,
    /// The paired phone is not reachable.
    PairedPhoneUnavailable,
    /// No Bluetooth device is known to be the phone.
    PhoneNotPaired,
    /// The PC's Bluetooth is off.
    BluetoothOff,
}

/// What identifies the phone to the proximity edge.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ProximityPhone {
    pub dcg_client_id: String,
    /// The phone's names (the DCG device-list name, the connect response's `attributes.displayName`).
    pub names: Vec<String>,
}

/// The proximity gate's Bluetooth state.
///
/// On macOS it comes from `IOBluetooth`: `IOBluetoothHostController.default()`'s `powerState` and
/// the paired `IOBluetoothDevice`'s `isConnected()`; which paired device is the phone is not yet
/// known.
#[uniffi::export(with_foreign)]
pub trait PhoneProximity: Send + Sync {
    fn state(&self, phone: ProximityPhone) -> ProximityState;
}

/// Stand-ins for this module's OS edges that the headless CLI runs with.
pub mod standins {
    use super::{
        AttachmentImaging, FileFamily, FittedImage, MessagingFileTypes, PhoneProximity, ProximityPhone, ProximityState,
    };
    use phonelink_core::edge::EdgeError;
    use phonelink_core::sync::MutexExt;
    use std::sync::Mutex;

    /// Detects PNG, JPEG, GIF and BMP by their magic bytes and transforms nothing: attachments are
    /// sent as they are (the CLI's stand-in for `ImageIO`).
    #[derive(Debug, Default)]
    pub struct PassThroughImaging;

    impl AttachmentImaging for PassThroughImaging {
        fn sniff_content_type(&self, bytes: Vec<u8>) -> Option<String> {
            sniff(&bytes).map(str::to_string)
        }

        fn fit_image(
            &self,
            _bytes: Vec<u8>,
            _max_width: u32,
            _max_height: u32,
            _max_bytes: u64,
        ) -> Result<Option<FittedImage>, EdgeError> {
            Ok(None)
        }
    }

    /// A small fixed MIME table.
    #[derive(Debug, Default)]
    pub struct FakeFileTypes;

    impl MessagingFileTypes for FakeFileTypes {
        fn family(&self, content_type: String) -> FileFamily {
            let t = content_type.to_ascii_lowercase();
            if t == "text/vcard" || t == "text/x-vcard" {
                FileFamily::VCard
            } else if t.starts_with("image/") {
                FileFamily::Image
            } else if t.starts_with("audio/") {
                FileFamily::Audio
            } else if t.starts_with("video/") {
                FileFamily::Video
            } else if t.starts_with("text/") {
                FileFamily::Text
            } else if t == "application/pdf" || t == "application/msword" {
                FileFamily::Document
            } else {
                FileFamily::Other
            }
        }

        fn file_extension(&self, content_type: String) -> Option<String> {
            let ext = match content_type.to_ascii_lowercase().as_str() {
                "image/jpeg" => ".jpg",
                "image/png" => ".png",
                "image/gif" => ".gif",
                "image/bmp" => ".bmp",
                "image/heic" => ".heic",
                "text/plain" => ".txt",
                "text/vcard" | "text/x-vcard" => ".vcf",
                "video/mp4" => ".mp4",
                "audio/mpeg" => ".mp3",
                "application/pdf" => ".pdf",
                _ => return None,
            };
            Some(ext.to_string())
        }
    }

    /// A proximity state the test sets; records the phones it was asked about.
    #[derive(Debug)]
    pub struct FixedProximity {
        pub state: Mutex<ProximityState>,
        pub asked: Mutex<Vec<ProximityPhone>>,
    }

    impl FixedProximity {
        pub fn new(state: ProximityState) -> std::sync::Arc<Self> {
            std::sync::Arc::new(Self { state: Mutex::new(state), asked: Mutex::new(Vec::new()) })
        }
        #[cfg(feature = "test-support")]
        pub fn set(&self, state: ProximityState) {
            *self.state.lock_or_panic() = state;
        }
    }

    impl PhoneProximity for FixedProximity {
        fn state(&self, phone: ProximityPhone) -> ProximityState {
            self.asked.lock_or_panic().push(phone);
            *self.state.lock_or_panic()
        }
    }

    /// Recognises PNG, JPEG, GIF and BMP by their magic bytes.
    pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
        if bytes.starts_with(b"\x89PNG") {
            Some("image/png")
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some("image/jpeg")
        } else if bytes.starts_with(b"GIF8") {
            Some("image/gif")
        } else if bytes.starts_with(b"BM") {
            Some("image/bmp")
        } else {
            None
        }
    }
}

/// In-memory stand-ins for the CLI and for tests.
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use super::standins::sniff;
    use super::{AttachmentImaging, FittedImage};
    use phonelink_core::edge::EdgeError;
    use phonelink_core::sync::MutexExt;
    use std::sync::Mutex;

    /// One `fit_image` call.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct FitCall {
        pub len: usize,
        pub max_width: u32,
        pub max_height: u32,
        pub max_bytes: u64,
    }

    /// Sniffs by magic bytes; "fits" an image by truncating it to `max_bytes` and reporting
    /// `output_type` (default: the sniffed type). Bytes it does not recognise are not transformed.
    #[derive(Debug, Default)]
    pub struct FakeImaging {
        pub calls: Mutex<Vec<FitCall>>,
        pub output_type: Mutex<Option<String>>,
        pub fail: std::sync::atomic::AtomicBool,
    }

    impl FakeImaging {
        pub fn new() -> std::sync::Arc<Self> {
            std::sync::Arc::new(Self::default())
        }
        pub fn calls(&self) -> Vec<FitCall> {
            self.calls.lock_or_panic().clone()
        }
        pub fn set_output_type(&self, t: Option<&str>) {
            *self.output_type.lock_or_panic() = t.map(str::to_string);
        }
        pub fn set_fail(&self, fail: bool) {
            self.fail.store(fail, std::sync::atomic::Ordering::SeqCst);
        }
    }

    impl AttachmentImaging for FakeImaging {
        fn sniff_content_type(&self, bytes: Vec<u8>) -> Option<String> {
            sniff(&bytes).map(str::to_string)
        }

        fn fit_image(
            &self,
            bytes: Vec<u8>,
            max_width: u32,
            max_height: u32,
            max_bytes: u64,
        ) -> Result<Option<FittedImage>, EdgeError> {
            self.calls.lock_or_panic().push(FitCall { len: bytes.len(), max_width, max_height, max_bytes });

            if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(EdgeError::Unexpected { message: "imaging failed".into() });
            }

            let Some(sniffed) = sniff(&bytes) else { return Ok(None) };
            let content_type = self.output_type.lock_or_panic().clone().unwrap_or_else(|| sniffed.to_string());
            let keep = usize::try_from(max_bytes).unwrap_or(usize::MAX).min(bytes.len());
            Ok(Some(FittedImage { bytes: bytes[..keep].to_vec(), content_type }))
        }
    }
}

#[cfg(test)]
mod tests;
