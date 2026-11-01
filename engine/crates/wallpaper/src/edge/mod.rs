//! The Mac's desktop picture for the phone's `/wallpaper` request (phone → PC): the OS edge that reads
//! and encodes it, the size rule it encodes to, and the `WallpaperResponseMessage` the engine builds.

use std::sync::{Arc, Mutex, MutexGuard};

use tracing::{error, info, warn};

use phonelink_core::edge::EdgeError;
use phonelink_core::sync::MutexExt;
use phonelink_proto::device_info::v1::{
    WallpaperResponseMessage, WallpaperResponseMetadata, WallpaperResponseStatus, WallpaperType,
};

/// A picture taller than this many pixels is scaled down to it.
pub const MAX_PICTURE_HEIGHT: u32 = 512;

/// The Mac's desktop picture as the phone gets it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum PcWallpaper {
    /// The picture, decoded, scaled to [`wallpaper_scaled_size`] and encoded as JPEG.
    Picture { jpeg: Vec<u8> },
    /// A picture file is set and exists, but it does not decode or encode.
    Unreadable,
    /// No picture file: none is set, or its file does not exist.
    NotSet,
}

/// A picture's size in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct PictureSize {
    pub width: u32,
    pub height: u32,
}

/// Where the Mac's desktop picture comes from (macOS: `NSWorkspace.desktopImageURL(for:)` of the
/// primary screen, `ImageIO`). Synchronous: the receiver calls it off the async runtime.
#[uniffi::export(with_foreign)]
pub trait PcWallpaperSource: Send + Sync {
    /// The primary screen's desktop picture, encoded for the phone. A failure that is not about the
    /// picture itself (the screen list cannot be read) is an error.
    fn current_wallpaper(&self) -> Result<PcWallpaper, EdgeError>;
}

/// The size a desktop picture of `width` × `height` pixels is encoded at.
///
/// Unchanged up to [`MAX_PICTURE_HEIGHT`] pixels high; taller, `512` high and
/// `round(width × (512 ⁄ height))` wide, computed in `f64` and rounded half to even (a width that
/// rounds to `0` stays `0`).
#[uniffi::export]
pub fn wallpaper_scaled_size(width: u32, height: u32) -> PictureSize {
    if height <= MAX_PICTURE_HEIGHT {
        return PictureSize { width, height };
    }

    let factor = f64::from(MAX_PICTURE_HEIGHT) / f64::from(height);
    let scaled = (f64::from(width) * factor).round_ties_even();
    PictureSize { width: scaled as u32, height: MAX_PICTURE_HEIGHT }
}

/// The answers to `/wallpaper`, read from the desktop picture edge on every request.
///
/// The last picture answered is kept: a read that fails — macOS lists no screen while its displays
/// sleep — is answered with it, and without one as an unreadable picture, which the phone ignores;
/// never as no picture, which the phone stores over its copy and shows as its default picture.
pub struct PcWallpaperAnswers {
    source: Arc<dyn PcWallpaperSource>,
    last_picture: Mutex<Option<Vec<u8>>>,
}

impl PcWallpaperAnswers {
    pub fn new(source: Arc<dyn PcWallpaperSource>) -> Self {
        Self { source, last_picture: Mutex::new(None) }
    }

    /// The answer to one request ([`pc_wallpaper_response`]). Bytes that are not a JPEG are logged and
    /// answered as an unreadable picture.
    pub fn answer(&self) -> WallpaperResponseMessage {
        let wallpaper = match self.source.current_wallpaper() {
            Ok(PcWallpaper::Picture { jpeg }) if !is_jpeg(&jpeg) => {
                error!(
                    bytes = jpeg.len(),
                    "the desktop picture edge returned bytes that are not a JPEG; answered as unreadable"
                );
                PcWallpaper::Unreadable
            }
            Ok(w) => w,
            Err(e) => return self.answer_after_failure(&e),
        };

        *self.last() = match &wallpaper {
            PcWallpaper::Picture { jpeg } => Some(jpeg.clone()),
            PcWallpaper::Unreadable | PcWallpaper::NotSet => None,
        };

        info!(answer_kind = wallpaper.kind(), "the Mac's wallpaper answered");
        pc_wallpaper_response(wallpaper)
    }

    /// The answer when reading the picture failed: the last picture answered, else unreadable.
    fn answer_after_failure(&self, e: &EdgeError) -> WallpaperResponseMessage {
        match self.last().clone() {
            Some(jpeg) => {
                warn!(error = %e, "reading the Mac's desktop picture failed; answered with the last picture");
                pc_wallpaper_response(PcWallpaper::Picture { jpeg })
            }
            None => {
                warn!(error = %e, "reading the Mac's desktop picture failed and none was read yet; answered as unreadable");
                pc_wallpaper_response(PcWallpaper::Unreadable)
            }
        }
    }

    fn last(&self) -> MutexGuard<'_, Option<Vec<u8>>> {
        self.last_picture.lock_or_recover()
    }
}

impl PcWallpaper {
    fn kind(&self) -> &'static str {
        match self {
            PcWallpaper::Picture { .. } => "picture",
            PcWallpaper::Unreadable => "unreadable",
            PcWallpaper::NotSet => "no picture",
        }
    }
}

/// The answer to `/wallpaper`.
///
/// One entry (`id` never set) — the JPEG as `HOMESCREEN` with status `OK`; a picture that does not
/// read as `HOMESCREEN` without payload and no status; no picture as an empty entry and no status.
pub fn pc_wallpaper_response(wallpaper: PcWallpaper) -> WallpaperResponseMessage {
    match wallpaper {
        PcWallpaper::Picture { jpeg } => WallpaperResponseMessage {
            wallpapers: vec![WallpaperResponseMetadata {
                id: 0,
                wallpaper_type: WallpaperType::Homescreen as i32,
                payload: Some(jpeg),
            }],
            status: WallpaperResponseStatus::Ok as i32,
        },
        PcWallpaper::Unreadable => WallpaperResponseMessage {
            wallpapers: vec![WallpaperResponseMetadata {
                id: 0,
                wallpaper_type: WallpaperType::Homescreen as i32,
                payload: None,
            }],
            status: WallpaperResponseStatus::Unspecified as i32,
        },
        PcWallpaper::NotSet => WallpaperResponseMessage {
            wallpapers: vec![WallpaperResponseMetadata::default()],
            status: WallpaperResponseStatus::Unspecified as i32,
        },
    }
}

/// A JPEG starts with the SOI marker `FF D8` and a marker.
fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF
}

/// Stand-ins for this module's OS edges that the headless CLI runs with.
pub mod standins {
    use super::{PcWallpaper, PcWallpaperSource};
    use phonelink_core::edge::EdgeError;
    use phonelink_core::sync::MutexExt;
    use std::sync::Mutex;

    /// A desktop picture the test sets; `None` makes every read fail.
    #[derive(Debug)]
    pub struct FixedPcWallpaper(Mutex<Option<PcWallpaper>>);

    impl FixedPcWallpaper {
        pub fn new(wallpaper: PcWallpaper) -> std::sync::Arc<Self> {
            std::sync::Arc::new(Self(Mutex::new(Some(wallpaper))))
        }

        #[cfg(feature = "test-support")]
        /// Every read fails.
        pub fn failing() -> std::sync::Arc<Self> {
            std::sync::Arc::new(Self(Mutex::new(None)))
        }

        #[cfg(feature = "test-support")]
        pub fn set(&self, wallpaper: PcWallpaper) {
            *self.0.lock_or_panic() = Some(wallpaper);
        }

        #[cfg(feature = "test-support")]
        /// Every read fails from now on.
        pub fn fail(&self) {
            *self.0.lock_or_panic() = None;
        }
    }

    impl PcWallpaperSource for FixedPcWallpaper {
        fn current_wallpaper(&self) -> Result<PcWallpaper, EdgeError> {
            self.0
                .lock_or_panic()
                .clone()
                .ok_or(EdgeError::Unexpected { message: "desktop picture unavailable".into() })
        }
    }
}

/// In-memory stand-ins for the CLI and for tests.
#[cfg(any(test, feature = "test-support"))]
pub mod testing {

    /// The smallest JPEG framing the answer accepts: SOI, an APP0 marker start, EOI. Not a decodable
    /// image — the engine does not decode the picture.
    pub const TINY_JPEG: [u8; 6] = [0xFF, 0xD8, 0xFF, 0xE0, 0xFF, 0xD9];
}

#[cfg(test)]
mod tests;
