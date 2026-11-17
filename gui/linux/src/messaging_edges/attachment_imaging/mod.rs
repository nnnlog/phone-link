//! Fitting outgoing image attachments to the carrier's limits with the `image` crate: scaled into the
//! box and encoded in the source's own format or as JPEG until the bytes fit, keeping the ICC profile.

use std::io::Cursor;

use image::codecs::gif::{GifDecoder, GifEncoder, Repeat};
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{AnimationDecoder, DynamicImage, Frame, ImageDecoder, ImageFormat, ImageReader, RgbImage};
use phonelink_engine::edge::EdgeError;
use phonelink_messaging::AttachmentImaging;
use phonelink_messaging::edge::FittedImage;
use tracing::info;

use crate::edges::unexpected;
use crate::image_io;

#[cfg(test)]
mod tests;

/// The JPEG qualities an image is written at, tried in order until it fits its budget.
const QUALITIES: [u8; 9] = [90, 80, 70, 60, 50, 40, 30, 20, 10];
/// When no encoding of a size fits the budget, the next size tried is this share of it…
const SHRINK_FACTOR: f64 = 0.75;
/// …down to a longest side of this many pixels.
const SMALLEST_SIDE: u32 = 16;
/// The GIF encoder's color quantization speed (1 the slowest, 30 the fastest): the gif crate's
/// suggested balance, which keeps the quality and takes a fraction of the time of the slowest.
const GIF_SPEED: i32 = 10;

/// Outgoing image attachments fitted to the carrier limits with the `image` crate.
#[derive(Debug, Default)]
pub struct ImageCrateImaging;

impl AttachmentImaging for ImageCrateImaging {
    fn sniff_content_type(&self, bytes: Vec<u8>) -> Option<String> {
        sniff(&bytes).map(str::to_owned)
    }

    fn fit_image(
        &self,
        bytes: Vec<u8>,
        max_width: u32,
        max_height: u32,
        max_bytes: u64,
    ) -> Result<Option<FittedImage>, EdgeError> {
        fit_image(&bytes, max_width, max_height, max_bytes)
    }
}

/// The ISO base media file brands of HEIF images (`ftyp` at byte 4, the major brand after it).
fn heif_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.get(4..8)? != b"ftyp" {
        return None;
    }

    match bytes.get(8..12)? {
        b"heic" | b"heix" | b"hevc" | b"hevx" | b"heim" | b"heis" | b"hevm" | b"hevs" => Some("image/heic"),
        b"mif1" | b"msf1" => Some("image/heif"),
        b"avif" | b"avis" => Some("image/avif"),
        _ => None,
    }
}

/// The MIME type of an image recognized by its first bytes: PNG, JPEG, GIF, WebP, TIFF, BMP, HEIC,
/// HEIF and AVIF. Formats whose signature is too short to tell them from other content (PNM, ICO,
/// TGA and the like) are not recognized.
fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if let Some(heif) = heif_type(bytes) {
        return Some(heif);
    }

    match image::guess_format(bytes).ok()? {
        ImageFormat::Png => Some("image/png"),
        ImageFormat::Jpeg => Some("image/jpeg"),
        ImageFormat::Gif => Some("image/gif"),
        ImageFormat::WebP => Some("image/webp"),
        ImageFormat::Tiff => Some("image/tiff"),
        ImageFormat::Bmp => Some("image/bmp"),
        _ => None,
    }
}

/// The formats this edge decodes, by the bytes' signature.
fn decodable_format(bytes: &[u8]) -> Option<ImageFormat> {
    if heif_type(bytes).is_some() {
        return None;
    }
    let format = image::guess_format(bytes).ok()?;
    matches!(
        format,
        ImageFormat::Png
            | ImageFormat::Jpeg
            | ImageFormat::Gif
            | ImageFormat::WebP
            | ImageFormat::Tiff
            | ImageFormat::Bmp
    )
    .then_some(format)
}

/// The formats tried at each size: the source's own when this edge writes it, then JPEG.
fn output_formats(source: ImageFormat) -> Vec<ImageFormat> {
    let own = matches!(
        source,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::Tiff | ImageFormat::Bmp
    );
    let mut formats = if own { vec![source] } else { Vec::new() };
    if source != ImageFormat::Jpeg {
        formats.push(ImageFormat::Jpeg);
    }
    formats
}

/// An orientation that turns the image a quarter: its shown width and height are swapped.
fn turns_quarter(orientation: Orientation) -> bool {
    matches!(
        orientation,
        Orientation::Rotate90 | Orientation::Rotate270 | Orientation::Rotate90FlipH | Orientation::Rotate270FlipH
    )
}

/// The decoded image: one picture with its orientation applied, or a GIF's frames with their delays
/// and its loop count (`None`: the file plays once).
enum Source {
    Still(DynamicImage),
    Animated { frames: Vec<Frame>, repeat: Option<Repeat> },
}

/// The loop count of a GIF's NETSCAPE2.0 application extension; `None` when it has none.
fn gif_repeat(bytes: &[u8]) -> Option<Repeat> {
    const MARKER: &[u8] = b"NETSCAPE2.0\x03\x01";
    let at = bytes.windows(MARKER.len()).position(|window| window == MARKER)? + MARKER.len();
    let count = u16::from_le_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]);
    Some(if count == 0 { Repeat::Infinite } else { Repeat::Finite(count) })
}

/// Unchanged when the image already fits both limits. Otherwise the largest size within the limits
/// (aspect ratio kept, never enlarged, its EXIF orientation applied) that an encoding fits into
/// `max_bytes`: at each size the source's own format when this edge writes it — every frame, for a
/// GIF — then JPEG at decreasing quality; a size that none fits is shrunk by [`SHRINK_FACTOR`]. Fails
/// when nothing fits at [`SMALLEST_SIDE`] (the engine then sends the original); `None` when the bytes
/// are no image this edge decodes.
fn fit_image(bytes: &[u8], max_width: u32, max_height: u32, max_bytes: u64) -> Result<Option<FittedImage>, EdgeError> {
    let Some(format) = decodable_format(bytes) else { return Ok(None) };
    let content_type = format.to_mime_type().to_owned();
    let Ok(mut decoder) = ImageReader::with_format(Cursor::new(bytes), format).into_decoder() else { return Ok(None) };
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 {
        return Ok(None);
    }
    if max_width == 0 || max_height == 0 {
        return Err(unexpected(format!("no image fits within {max_width}×{max_height}")));
    }

    let orientation = decoder.orientation().map_err(|e| unexpected(format!("image orientation: {e}")))?;
    let icc = image_io::icc_profile(&mut decoder, Some(format), bytes)
        .map_err(|e| unexpected(format!("image color profile: {e}")))?;
    let (shown_width, shown_height) = if turns_quarter(orientation) { (height, width) } else { (width, height) };
    let scale =
        (f64::from(max_width) / f64::from(shown_width)).min(f64::from(max_height) / f64::from(shown_height)).min(1.0);
    if scale >= 1.0 && bytes.len() as u64 <= max_bytes {
        return Ok(Some(FittedImage { bytes: bytes.to_vec(), content_type }));
    }

    let source = if format == ImageFormat::Gif {
        let frames = GifDecoder::new(Cursor::new(bytes))
            .and_then(|gif| gif.into_frames().collect_frames())
            .map_err(|e| unexpected(format!("GIF decode: {e}")))?;
        Source::Animated { frames, repeat: gif_repeat(bytes) }
    } else {
        let mut image = DynamicImage::from_decoder(decoder).map_err(|e| unexpected(format!("image decode: {e}")))?;
        image.apply_orientation(orientation);
        Source::Still(image)
    };

    let formats = output_formats(format);
    let longest = f64::from(shown_width.max(shown_height));
    let mut side = ((longest * scale).floor() as u32).max(1);
    loop {
        for output in &formats {
            if let Some(fitted) = encode(&source, icc.as_deref(), *output, side, max_bytes)? {
                return Ok(Some(fitted));
            }
        }
        let next = (f64::from(side) * SHRINK_FACTOR).floor() as u32;
        if next < SMALLEST_SIDE {
            return Err(unexpected(format!("no encoding of the image fits {max_bytes} bytes, down to {side} pixels")));
        }
        side = next;
    }
}

fn first_frame(source: &Source) -> Result<DynamicImage, EdgeError> {
    match source {
        Source::Still(image) => Ok(image.clone()),
        Source::Animated { frames, .. } => frames
            .first()
            .map(|frame| DynamicImage::ImageRgba8(frame.buffer().clone()))
            .ok_or_else(|| unexpected("a GIF without frames".to_owned())),
    }
}

/// The image drawn over white in sRGB, for JPEG, which has no alpha: transparent pixels would come out
/// black. Colors with a profile that does not convert are taken as sRGB.
fn over_white(image: &DynamicImage, icc: Option<&[u8]>) -> RgbImage {
    let rgba = image_io::srgb_rgba8(image, icc).unwrap_or_else(|e| {
        info!(error = %e, "an attachment's colors are taken as sRGB over white");
        image.to_rgba8()
    });
    let mut rgb = RgbImage::new(rgba.width(), rgba.height());
    for (x, y, pixel) in rgba.enumerate_pixels() {
        let alpha = u32::from(pixel[3]);
        let blend = |channel: u8| ((u32::from(channel) * alpha + 255 * (255 - alpha)) / 255) as u8;
        rgb.put_pixel(x, y, image::Rgb([blend(pixel[0]), blend(pixel[1]), blend(pixel[2])]));
    }

    rgb
}

/// The image at `side` pixels on its longest side as `format` when that fits `max_bytes`: JPEG at the
/// first quality that fits, a GIF with all its frames, any other format once. JPEG, PNG and TIFF carry
/// the source's ICC profile `icc`.
fn encode(
    source: &Source,
    icc: Option<&[u8]>,
    format: ImageFormat,
    side: u32,
    max_bytes: u64,
) -> Result<Option<FittedImage>, EdgeError> {
    let fits = |bytes: Vec<u8>| {
        (bytes.len() as u64 <= max_bytes).then(|| FittedImage { bytes, content_type: format.to_mime_type().to_owned() })
    };
    let encoding = |e: &dyn std::fmt::Display| unexpected(format!("encoding a {side}-pixel image as {format:?}: {e}"));

    if format == ImageFormat::Gif {
        let Source::Animated { frames, repeat } = source else {
            return Err(unexpected("only a GIF is written as a GIF".to_owned()));
        };
        let mut output = Vec::new();
        {
            let mut encoder = GifEncoder::new_with_speed(&mut output, GIF_SPEED);
            if let Some(repeat) = repeat {
                encoder.set_repeat(*repeat).map_err(|e| encoding(&e))?;
            }
            for frame in frames {
                let resized =
                    DynamicImage::ImageRgba8(frame.buffer().clone()).resize(side, side, FilterType::CatmullRom);
                encoder
                    .encode_frame(Frame::from_parts(resized.to_rgba8(), 0, 0, frame.delay()))
                    .map_err(|e| encoding(&e))?;
            }
        }
        return Ok(fits(output));
    }

    let image = first_frame(source)?.resize(side, side, FilterType::CatmullRom);
    let output = match format {
        ImageFormat::Jpeg => {
            // Gray stays gray, with its profile; alpha is drawn over white in sRGB.
            let (pixels, icc) = match image.color() {
                color if color.has_alpha() => (DynamicImage::ImageRgb8(over_white(&image, icc)), None),
                color if !color.has_color() => (DynamicImage::ImageLuma8(image.to_luma8()), icc),
                _ => (DynamicImage::ImageRgb8(image.to_rgb8()), icc),
            };
            for quality in QUALITIES {
                let output = image_io::jpeg(&pixels, quality, icc).map_err(|e| encoding(&e))?;
                if let Some(fitted) = fits(output) {
                    return Ok(Some(fitted));
                }
            }
            return Ok(None);
        }
        ImageFormat::Png => image_io::png(&image, icc).map_err(|e| encoding(&e))?,
        ImageFormat::Tiff => image_io::tiff(&image, icc).map_err(|e| encoding(&e))?,
        _ => {
            let mut output = Cursor::new(Vec::new());
            image.write_to(&mut output, format).map_err(|e| encoding(&e))?;
            output.into_inner()
        }
    };
    Ok(fits(output))
}
