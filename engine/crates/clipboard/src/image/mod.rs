//! The image codec OS edge and the image rules.
//!
//! It covers re-encoding the PNG sent to the phone (at most 1 MiB, with the downscale loop), the
//! PNG the announcement's dedupe hash is computed over, and the handling of a received `IMAGE`
//! (decode, then write `public.png` and `public.tiff`).
//!
//! The codec works on encoded bytes (any format the platform decodes in, PNG or TIFF out), so the
//! algorithm — passes, `round_half_to_even`, the limit — stays here, testable with a fake codec.

use phonelink_core::edge::EdgeError;

/// Pixel dimensions of a decoded image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct ClipboardImageSize {
    pub width: u32,
    pub height: u32,
}

/// Image decoding, encoding and resampling (macOS: `ImageIO` and Core Graphics).
///
/// Called off the async runtime; every method decodes `image` itself (`CGImageSourceCreateWithData`
/// + `CGImageSourceCreateImageAtIndex(_, 0, _)`) and fails when it cannot.
#[uniffi::export(with_foreign)]
pub trait ClipboardImageCodec: Send + Sync {
    /// The pixel size of the decoded image.
    fn size(&self, image: Vec<u8>) -> Result<ClipboardImageSize, EdgeError>;
    /// The decoded image encoded as PNG at full size, alpha preserved (`CGImageDestination`, `public.png`).
    fn encode_png(&self, image: Vec<u8>) -> Result<Vec<u8>, EdgeError>;
    /// The decoded image redrawn at `width` × `height` into a new 32-bit RGBA bitmap context with
    /// high-quality interpolation (`CGContext.interpolationQuality = .high`), encoded as PNG.
    fn resize_png(&self, image: Vec<u8>, width: u32, height: u32) -> Result<Vec<u8>, EdgeError>;
    /// The decoded image encoded as TIFF (`CGImageDestination`, `public.tiff`), for the pasteboard.
    fn encode_tiff(&self, image: Vec<u8>) -> Result<Vec<u8>, EdgeError>;
}

/// The 8-byte PNG signature.
pub const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// A size ratio above this divides both dimensions by about `ratio / RATIO_STEP` more per pass.
pub const RATIO_STEP: f64 = 8.5;

/// The bytes start with the PNG signature.
pub fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(&PNG_SIGNATURE)
}

/// Rounds to the nearest integer, a tie to the even one.
pub fn round_half_to_even(x: f64) -> f64 {
    x.round_ties_even()
}

/// The dimensions one downscale pass draws `width` × `height` at, for the size ratio `s > 1`:
/// `k = round_half_to_even(s > 8.5 ? s / 8.5 : 1)`, then `trunc(W · k / s)` × `trunc(H · k / s)`.
pub fn pass_dimensions(width: u32, height: u32, s: f64) -> (u32, u32) {
    let k = round_half_to_even(if s > RATIO_STEP { s / RATIO_STEP } else { 1.0 });
    let scale = |d: u32| (f64::from(d) * k / s).trunc() as u32;
    (scale(width), scale(height))
}

/// Why no PNG within the limit could be produced (the content answer is `INVALID_CONTENT`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReencodeError {
    #[error("the image does not decode: {0}")]
    Decode(EdgeError),
    /// Answered `INVALID_CONTENT` (Phone Link on Windows sends the bytes written so far instead).
    #[error("the full-size PNG encode failed: {0}")]
    Encode(EdgeError),
    #[error("downscale pass {pass} failed: {error}")]
    Resize { pass: u32, error: EdgeError },
    #[error("downscale pass {pass} computed {width}x{height}")]
    ZeroDimension { pass: u32, width: u32, height: u32 },
    #[error("the PNG is still {size} bytes after {passes} passes")]
    TooLarge { passes: u32, size: usize },
}

/// Image re-encoding: the PNG of `image` at most `limit` bytes, downscaling at most `max_passes`
/// times. A zero width or height gives an empty PNG (an `IMAGE` item whose `image_bytes` is empty).
pub fn reencode_png(
    codec: &dyn ClipboardImageCodec,
    image: &[u8],
    limit: usize,
    max_passes: u32,
) -> Result<Vec<u8>, ReencodeError> {
    let size = codec.size(image.to_vec()).map_err(ReencodeError::Decode)?;
    if size.width == 0 || size.height == 0 {
        return Ok(Vec::new());
    }

    let ratio = |png: &[u8]| png.len() as f64 / limit as f64;
    let mut png = codec.encode_png(image.to_vec()).map_err(ReencodeError::Encode)?;
    let mut s = ratio(&png);
    let (mut width, mut height) = (size.width, size.height);
    let mut pass = 0;

    while s > 1.0 {
        if pass == max_passes {
            return Err(ReencodeError::TooLarge { passes: pass, size: png.len() });
        }

        pass += 1;
        let (w, h) = pass_dimensions(width, height, s);
        if w == 0 || h == 0 {
            return Err(ReencodeError::ZeroDimension { pass, width: w, height: h });
        }

        // The resized image is the next pass's input; its PNG carries it losslessly.
        png = codec.resize_png(png, w, h).map_err(|error| ReencodeError::Resize { pass, error })?;
        s = ratio(&png);
        (width, height) = (w, h);
    }

    Ok(png)
}

/// The bytes the announcement hash is computed over: the PNG of the full-size image; a zero-sized
/// image or a PNG encoder failure gives no bytes.
///
/// `Err` only when the image does not decode (a read failure: no announcement).
pub fn announcement_png(codec: &dyn ClipboardImageCodec, image: &[u8]) -> Result<Vec<u8>, EdgeError> {
    let size = codec.size(image.to_vec())?;
    if size.width == 0 || size.height == 0 {
        return Ok(Vec::new());
    }
    Ok(codec.encode_png(image.to_vec()).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "PNG encode of the copied image failed; the hash of no bytes is announced");
        Vec::new()
    }))
}

/// The representations written for an image received from the phone.
///
/// `public.png` (the received bytes when they are PNG, else re-encoded) and `public.tiff`; `Err`
/// when the bytes do not decode or cannot be converted (nothing is written then).
///
/// No size limit and no resize.
pub fn pasteboard_image(codec: &dyn ClipboardImageCodec, image: &[u8]) -> Result<(Vec<u8>, Vec<u8>), EdgeError> {
    codec.size(image.to_vec())?;
    let png = if is_png(image) { image.to_vec() } else { codec.encode_png(image.to_vec())? };
    let tiff = codec.encode_tiff(image.to_vec())?;
    Ok((png, tiff))
}

/// Stand-ins for this module's OS edges that the headless CLI runs with.
pub mod standins {
    use super::{ClipboardImageCodec, ClipboardImageSize, PNG_SIGNATURE};
    use phonelink_core::edge::EdgeError;
    use phonelink_core::sync::MutexExt;
    use std::sync::{Arc, Mutex};

    /// The encodings of the fake codec's synthetic images.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum FakeFormat {
        /// Starts with the PNG signature.
        Png,
        /// Starts with the big-endian TIFF magic `MM\0*`.
        Tiff,
        /// Any other format the platform decodes (`BM`).
        Bmp,
    }

    impl FakeFormat {
        fn magic(self) -> &'static [u8] {
            match self {
                FakeFormat::Png => &PNG_SIGNATURE,
                FakeFormat::Tiff => b"MM\0*",
                FakeFormat::Bmp => b"BM",
            }
        }
    }

    /// A codec call, for assertions.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum CodecCall {
        Size,
        EncodePng { width: u32, height: u32 },
        ResizePng { width: u32, height: u32 },
        EncodeTiff,
    }

    /// A codec over synthetic images: `magic | width (u32 LE) | height (u32 LE) | padding`. The PNG it
    /// produces for `w` × `h` has the length [`FakeCodec::set_png_size`] computes (default `16 + w·h`).
    pub struct FakeCodec {
        png_size: Mutex<SizeFn>,
        calls: Mutex<Vec<CodecCall>>,
        fail_png: Mutex<bool>,
        fail_resize: Mutex<bool>,
        fail_tiff: Mutex<bool>,
    }

    impl FakeCodec {
        pub fn new() -> Arc<Self> {
            Arc::new(Self {
                png_size: Mutex::new(Box::new(|w, h| 16 + w as usize * h as usize)),
                calls: Mutex::new(Vec::new()),
                fail_png: Mutex::new(false),
                fail_resize: Mutex::new(false),
                fail_tiff: Mutex::new(false),
            })
        }

        /// A synthetic image of `width` × `height` in `format`, `len` bytes long (at least the header).
        pub fn image_with_len(format: FakeFormat, width: u32, height: u32, len: usize) -> Vec<u8> {
            let mut bytes = format.magic().to_vec();
            bytes.extend_from_slice(&width.to_le_bytes());
            bytes.extend_from_slice(&height.to_le_bytes());
            if bytes.len() < len {
                bytes.resize(len, 0xAB);
            }
            bytes
        }

        /// A synthetic image of `width` × `height` in `format`.
        pub fn image(format: FakeFormat, width: u32, height: u32) -> Vec<u8> {
            Self::image_with_len(format, width, height, 0)
        }

        #[cfg(feature = "test-support")]
        /// The PNG length for `w` × `h`.
        pub fn set_png_size(&self, f: impl Fn(u32, u32) -> usize + Send + Sync + 'static) {
            *self.png_size.lock_or_panic() = Box::new(f);
        }

        #[cfg(feature = "test-support")]
        pub fn fail_png(&self, fail: bool) {
            *self.fail_png.lock_or_panic() = fail;
        }

        #[cfg(feature = "test-support")]
        pub fn fail_resize(&self, fail: bool) {
            *self.fail_resize.lock_or_panic() = fail;
        }

        #[cfg(feature = "test-support")]
        pub fn fail_tiff(&self, fail: bool) {
            *self.fail_tiff.lock_or_panic() = fail;
        }

        #[cfg(feature = "test-support")]
        pub fn calls(&self) -> Vec<CodecCall> {
            self.calls.lock_or_panic().clone()
        }

        /// The dimensions of a synthetic image, `None` when the bytes are not one.
        pub fn decode(image: &[u8]) -> Option<(FakeFormat, u32, u32)> {
            [FakeFormat::Png, FakeFormat::Tiff, FakeFormat::Bmp].into_iter().find_map(|format| {
                let rest = image.strip_prefix(format.magic())?;
                let width = u32::from_le_bytes(rest.get(0..4)?.try_into().ok()?);
                let height = u32::from_le_bytes(rest.get(4..8)?.try_into().ok()?);
                Some((format, width, height))
            })
        }

        fn decoded(image: &[u8]) -> Result<(u32, u32), EdgeError> {
            Self::decode(image)
                .map(|(_, w, h)| (w, h))
                .ok_or_else(|| EdgeError::Unexpected { message: "not an image".into() })
        }

        fn png(&self, width: u32, height: u32) -> Vec<u8> {
            let len = (self.png_size.lock_or_panic())(width, height);
            Self::image_with_len(FakeFormat::Png, width, height, len)
        }

        fn failure(flag: &Mutex<bool>, what: &str) -> Result<(), EdgeError> {
            if *flag.lock_or_panic() {
                Err(EdgeError::Unexpected { message: format!("{what} failed") })
            } else {
                Ok(())
            }
        }
    }

    impl ClipboardImageCodec for FakeCodec {
        fn size(&self, image: Vec<u8>) -> Result<ClipboardImageSize, EdgeError> {
            self.calls.lock_or_panic().push(CodecCall::Size);
            let (width, height) = Self::decoded(&image)?;
            Ok(ClipboardImageSize { width, height })
        }

        fn encode_png(&self, image: Vec<u8>) -> Result<Vec<u8>, EdgeError> {
            let (width, height) = Self::decoded(&image)?;
            self.calls.lock_or_panic().push(CodecCall::EncodePng { width, height });
            Self::failure(&self.fail_png, "PNG encode")?;
            Ok(self.png(width, height))
        }

        fn resize_png(&self, image: Vec<u8>, width: u32, height: u32) -> Result<Vec<u8>, EdgeError> {
            Self::decoded(&image)?;
            self.calls.lock_or_panic().push(CodecCall::ResizePng { width, height });
            Self::failure(&self.fail_resize, "resize")?;
            Ok(self.png(width, height))
        }

        fn encode_tiff(&self, image: Vec<u8>) -> Result<Vec<u8>, EdgeError> {
            let (width, height) = Self::decoded(&image)?;
            self.calls.lock_or_panic().push(CodecCall::EncodeTiff);
            Self::failure(&self.fail_tiff, "TIFF encode")?;
            Ok(Self::image(FakeFormat::Tiff, width, height))
        }
    }

    type SizeFn = Box<dyn Fn(u32, u32) -> usize + Send + Sync>;
}

#[cfg(test)]
mod tests;
