//! A picture file for the phone: decoded — the image formats of the `image` crate, JPEG XL and SVG,
//! and GNOME's slideshow and timed backgrounds as the picture they show now —, converted to sRGB,
//! scaled to the engine's size rule, drawn over black and encoded as JPEG.

use std::io::Cursor;
use std::path::Path;
use std::time::SystemTime;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, RgbImage};
use jxl_oxide::integration::JxlDecoder;
use phonelink_wallpaper::PcWallpaper;
use phonelink_wallpaper::edge::wallpaper_scaled_size;
use resvg::usvg;
use tracing::{info, warn};

use super::slideshow::{Slideshow, SlideshowError};

#[cfg(test)]
mod tests;

/// The JPEG quality the picture is encoded at.
const JPEG_QUALITY: u8 = 90;

/// Why a picture file was not read, or not encoded for the phone.
#[derive(Debug, thiserror::Error)]
enum PictureError {
    #[error("{0}")]
    Xml(#[from] roxmltree::Error),
    #[error("an XML file of <{0}> is no picture")]
    OtherXml(String),
    #[error("{0}")]
    Image(#[from] image::ImageError),
    #[error("{0}")]
    Svg(#[from] usvg::Error),
    #[error("the picture scales to {width}×{height}")]
    ScaledToNothing { width: u32, height: u32 },
    #[error("the drawn SVG has no pixels")]
    NoSvgPixels,
}

/// The picture file at `path` for the phone: none when it does not exist, unreadable when it is a
/// folder or does not decode. A GNOME slideshow stands for the picture it shows now.
pub(super) fn picture_of(path: &Path) -> PcWallpaper {
    picture_file(path, SystemTime::now(), true)
}

/// [`picture_of`] at `now`; a slideshow is read only while `slideshows` is set, so one slideshow does
/// not stand for another.
fn picture_file(path: &Path, now: SystemTime, slideshows: bool) -> PcWallpaper {
    match std::fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return PcWallpaper::NotSet,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "the desktop picture was not read");
            return PcWallpaper::Unreadable;
        }
        Ok(metadata) if metadata.is_dir() => {
            warn!(path = %path.display(), "the desktop picture setting names a folder");
            return PcWallpaper::Unreadable;
        }
        Ok(_) => {}
    }

    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "the desktop picture was not read");
            return PcWallpaper::Unreadable;
        }
    };

    let folder = path.parent().unwrap_or(Path::new("/"));
    let content = match Content::of(&bytes) {
        Ok(content) => content,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "the desktop picture was not read");
            return PcWallpaper::Unreadable;
        }
    };

    let Content::Slideshow(text) = content else {
        return match encode(content, folder) {
            Ok(jpeg) => PcWallpaper::Picture { jpeg },
            Err(e) => {
                warn!(path = %path.display(), error = %e, "the desktop picture was not encoded for the phone");
                PcWallpaper::Unreadable
            }
        };
    };

    let shown = if slideshows {
        Slideshow::parse(text, folder).and_then(|slideshow| slideshow.picture_at(unix_time(now)).map(Path::to_owned))
    } else {
        Err(SlideshowError::Nested)
    };
    match shown {
        Ok(shown) => picture_file(&shown, now, false),
        Err(e) => {
            warn!(path = %path.display(), error = %e, "the desktop picture's slideshow was not read");
            PcWallpaper::Unreadable
        }
    }
}

/// `time` in seconds since the Unix epoch, negative before it.
fn unix_time(time: SystemTime) -> i64 {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_secs()).unwrap_or(i64::MAX),
    }
}

/// What a picture file holds.
enum Content<'a> {
    /// JPEG XL, by its codestream or container signature.
    JpegXl(&'a [u8]),
    /// SVG, or gzip-compressed SVG.
    Svg(&'a [u8]),
    /// A GNOME background slideshow (`<background>`), as text.
    Slideshow(&'a str),
    /// Any format the `image` crate reads.
    Raster(&'a [u8]),
}

impl<'a> Content<'a> {
    /// What `bytes` hold, by their signature or their XML root element.
    fn of(bytes: &'a [u8]) -> Result<Self, PictureError> {
        const JXL_CODESTREAM: &[u8] = &[0xFF, 0x0A];
        const JXL_CONTAINER: &[u8] = &[0, 0, 0, 0x0C, b'J', b'X', b'L', b' ', 0x0D, 0x0A, 0x87, 0x0A];
        const GZIP: &[u8] = &[0x1F, 0x8B];

        if bytes.starts_with(JXL_CODESTREAM) || bytes.starts_with(JXL_CONTAINER) {
            return Ok(Self::JpegXl(bytes));
        }
        if bytes.starts_with(GZIP) {
            return Ok(Self::Svg(bytes));
        }

        let starts_as_xml = |text: &&str| text.trim_start_matches('\u{feff}').trim_start().starts_with('<');
        let Some(text) = std::str::from_utf8(bytes).ok().filter(starts_as_xml) else {
            return Ok(Self::Raster(bytes));
        };
        let options = roxmltree::ParsingOptions { allow_dtd: true, ..roxmltree::ParsingOptions::default() };
        let document = roxmltree::Document::parse_with_options(text, options)?;
        match document.root_element().tag_name().name() {
            "svg" => Ok(Self::Svg(bytes)),
            "background" => Ok(Self::Slideshow(text)),
            other => Err(PictureError::OtherXml(other.to_owned())),
        }
    }
}

/// The picture, drawn opaque over black in sRGB at the engine's size and written as JPEG. Its relative
/// references are taken from `folder`.
fn encode(content: Content<'_>, folder: &Path) -> Result<Vec<u8>, PictureError> {
    let opaque = match content {
        Content::JpegXl(bytes) => {
            let mut decoder = JxlDecoder::new(Cursor::new(bytes))?;
            let icc = decoder.icc_profile()?;
            raster(DynamicImage::from_decoder(decoder)?, icc.as_deref())?
        }
        Content::Svg(bytes) => svg(bytes, folder)?,
        Content::Raster(bytes) => {
            let (image, icc) = crate::image_io::decode(bytes)?;
            raster(image, icc.as_deref())?
        }
        Content::Slideshow(_) => unreachable!("a slideshow is resolved to its picture before it is encoded"),
    };
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY).encode_image(&opaque)?;
    Ok(jpeg)
}

/// A decoded picture scaled to the engine's size rule, its colors converted from its ICC profile `icc`
/// to sRGB, and drawn over black.
fn raster(image: DynamicImage, icc: Option<&[u8]>) -> Result<RgbImage, PictureError> {
    let size = wallpaper_scaled_size(image.width(), image.height());
    if size.width == 0 || size.height == 0 {
        return Err(PictureError::ScaledToNothing { width: size.width, height: size.height });
    }

    let image = if (size.width, size.height) == (image.width(), image.height()) {
        image
    } else {
        image.resize_exact(size.width, size.height, FilterType::Lanczos3)
    };
    let rgba = crate::image_io::srgb_rgba8(&image, icc).unwrap_or_else(|e| {
        info!(error = %e, "the desktop picture's colors are taken as sRGB");
        image.to_rgba8()
    });

    let over_black =
        |channel: u8, alpha: u8| u8::try_from(u16::from(channel) * u16::from(alpha) / 255).unwrap_or(u8::MAX);
    Ok(RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        image::Rgb([over_black(r, a), over_black(g, a), over_black(b, a)])
    }))
}

/// An SVG drawn at the size the engine's rule gives for its own size, over black.
fn svg(bytes: &[u8], folder: &Path) -> Result<RgbImage, PictureError> {
    use resvg::tiny_skia;

    let options = usvg::Options { resources_dir: Some(folder.to_owned()), ..usvg::Options::default() };
    let tree = usvg::Tree::from_data(bytes, &options)?;
    let natural = tree.size();
    let size = wallpaper_scaled_size(natural.width().round() as u32, natural.height().round() as u32);
    let mut pixmap = tiny_skia::Pixmap::new(size.width, size.height)
        .ok_or(PictureError::ScaledToNothing { width: size.width, height: size.height })?;
    let scale =
        tiny_skia::Transform::from_scale(size.width as f32 / natural.width(), size.height as f32 / natural.height());
    resvg::render(&tree, scale, &mut pixmap.as_mut());
    // Premultiplied colors are the colors over black.
    let pixels = pixmap.data().as_chunks::<4>().0.iter().flat_map(|[r, g, b, _]| [*r, *g, *b]).collect();
    RgbImage::from_raw(size.width, size.height, pixels).ok_or(PictureError::NoSvgPixels)
}
