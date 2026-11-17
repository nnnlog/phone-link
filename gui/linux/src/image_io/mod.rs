//! Image decoding and encoding shared by the image edges (the clipboard, message attachments and the
//! desktop picture): an image is decoded with its ICC profile and written with it, so its colors keep
//! their meaning, and converted to sRGB where it leaves as sRGB.

use std::borrow::Cow;
use std::io::{Cursor, Seek, Write};

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ColorType, DynamicImage, ImageDecoder, ImageEncoder, ImageError, ImageFormat, ImageReader, RgbaImage};
use moxcms::{CmsError, ColorProfile, DataColorSpace, Layout, TransformOptions};
use tiff::encoder::{TiffEncoder, TiffValue, colortype};
use tiff::tags::{Tag, Type};

#[cfg(test)]
pub mod testing;
#[cfg(test)]
mod tests;

/// `ExtraSamples`: the fourth sample is alpha, not premultiplied.
const UNASSOCIATED_ALPHA: u16 = 2;

/// Why an image was not written as TIFF.
#[derive(Debug, thiserror::Error)]
pub enum TiffEncodeError {
    #[error("{0}")]
    Encode(#[from] tiff::TiffError),
    #[error("no TIFF is written of {0:?} pixels")]
    UnsupportedPixels(ColorType),
}

/// Why an image's colors were not converted to sRGB.
#[derive(Debug, thiserror::Error)]
pub enum SrgbConversionError {
    #[error("the ICC profile does not read: {0}")]
    Profile(#[source] CmsError),
    #[error("no transform from the ICC profile to sRGB: {0}")]
    Transform(#[source] CmsError),
    #[error("the conversion to sRGB failed: {0}")]
    Conversion(#[source] CmsError),
    #[error("a {space:?} profile does not describe {color:?} pixels")]
    ProfileMismatch { space: DataColorSpace, color: ColorType },
}

/// The ICC profile describes pixels of `color`: by its data color space (the header's bytes 16–19), an
/// RGB profile RGB pixels and a gray profile gray ones.
fn profile_fits(icc: &[u8], color: ColorType) -> bool {
    let space: &[u8] = if color.has_color() { b"RGB " } else { b"GRAY" };
    icc.get(16..20) == Some(space)
}

/// The first image of `bytes`, and its ICC profile when it carries one.
pub fn decode(bytes: &[u8]) -> Result<(DynamicImage, Option<Vec<u8>>), ImageError> {
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let format = reader.format();
    let mut decoder = reader.into_decoder()?;
    let icc = icc_profile(&mut decoder, format, bytes)?;
    Ok((DynamicImage::from_decoder(decoder)?, icc))
}

/// The ICC profile of the image `decoder` reads from `bytes` in `format`.
pub fn icc_profile(
    decoder: &mut impl ImageDecoder,
    format: Option<ImageFormat>,
    bytes: &[u8],
) -> Result<Option<Vec<u8>>, ImageError> {
    match format {
        Some(ImageFormat::Tiff) => Ok(tiff_icc(bytes)),
        _ => decoder.icc_profile(),
    }
}

/// The ICC profile of a TIFF's first image, read with the tiff crate by its `IccProfile` tag. The image
/// crate's TIFF decoder (image 0.25.9) asks for `Tag::Unknown(34675)`, but the tiff crate files the tag
/// as `IccProfile`, so it never finds one. A profile that does not read is none.
fn tiff_icc(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut decoder = tiff::decoder::Decoder::new(Cursor::new(bytes)).ok()?;
    decoder.find_tag(Tag::IccProfile).ok()??.into_u8_vec().ok()
}

/// `image` as PNG in its own pixel format — floating-point samples, which PNG does not store, as 16
/// bits — with `icc` when it describes the pixels.
pub fn png(image: &DynamicImage, icc: Option<&[u8]>) -> Result<Vec<u8>, ImageError> {
    let image = match image {
        DynamicImage::ImageRgb32F(_) => Cow::Owned(DynamicImage::ImageRgb16(image.to_rgb16())),
        DynamicImage::ImageRgba32F(_) => Cow::Owned(DynamicImage::ImageRgba16(image.to_rgba16())),
        _ => Cow::Borrowed(image),
    };
    let mut bytes = Vec::new();
    let mut encoder = PngEncoder::new(&mut bytes);
    if let Some(icc) = icc.filter(|icc| profile_fits(icc, image.color())) {
        encoder.set_icc_profile(icc.to_vec()).map_err(ImageError::Unsupported)?;
    }
    encoder.write_image(image.as_bytes(), image.width(), image.height(), image.color().into())?;
    Ok(bytes)
}

/// `image`, gray or RGB without alpha, as JPEG at `quality`, with `icc` when it describes the pixels.
pub fn jpeg(image: &DynamicImage, quality: u8, icc: Option<&[u8]>) -> Result<Vec<u8>, ImageError> {
    let mut bytes = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut bytes, quality);
    if let Some(icc) = icc.filter(|icc| profile_fits(icc, image.color())) {
        encoder.set_icc_profile(icc.to_vec()).map_err(ImageError::Unsupported)?;
    }
    encoder.write_image(image.as_bytes(), image.width(), image.height(), image.color().into())?;
    Ok(bytes)
}

/// An ICC profile as TIFF stores it: bytes of type `UNDEFINED`.
struct IccProfile<'a>(&'a [u8]);

impl TiffValue for IccProfile<'_> {
    const BYTE_LEN: u8 = 1;
    const FIELD_TYPE: Type = Type::UNDEFINED;

    fn count(&self) -> usize {
        self.0.len()
    }

    fn data(&self) -> Cow<'_, [u8]> {
        Cow::Borrowed(self.0)
    }
}

/// `image` as an uncompressed TIFF in its own pixel format, with `icc` when it describes the pixels
/// written. Gray with alpha, which TIFF writers do not take here, is written as RGBA.
pub fn tiff(image: &DynamicImage, icc: Option<&[u8]>) -> Result<Vec<u8>, TiffEncodeError> {
    let (width, height) = (image.width(), image.height());
    let fits = |color: ColorType| icc.filter(|icc| profile_fits(icc, color));
    let mut bytes = Cursor::new(Vec::new());
    {
        let mut encoder = TiffEncoder::new(&mut bytes)?;
        let encoder = &mut encoder;
        let written = match image {
            DynamicImage::ImageLuma8(i) => {
                write::<colortype::Gray8, _>(encoder, width, height, i.as_raw(), fits(ColorType::L8), false)
            }
            DynamicImage::ImageLuma16(i) => {
                write::<colortype::Gray16, _>(encoder, width, height, i.as_raw(), fits(ColorType::L16), false)
            }
            DynamicImage::ImageLumaA8(_) => {
                let rgba = image.to_rgba8();
                write::<colortype::RGBA8, _>(encoder, width, height, rgba.as_raw(), fits(ColorType::Rgba8), true)
            }
            DynamicImage::ImageLumaA16(_) => {
                let rgba = image.to_rgba16();
                write::<colortype::RGBA16, _>(encoder, width, height, rgba.as_raw(), fits(ColorType::Rgba16), true)
            }
            DynamicImage::ImageRgb8(i) => {
                write::<colortype::RGB8, _>(encoder, width, height, i.as_raw(), fits(ColorType::Rgb8), false)
            }
            DynamicImage::ImageRgb16(i) => {
                write::<colortype::RGB16, _>(encoder, width, height, i.as_raw(), fits(ColorType::Rgb16), false)
            }
            DynamicImage::ImageRgba8(i) => {
                write::<colortype::RGBA8, _>(encoder, width, height, i.as_raw(), fits(ColorType::Rgba8), true)
            }
            DynamicImage::ImageRgba16(i) => {
                write::<colortype::RGBA16, _>(encoder, width, height, i.as_raw(), fits(ColorType::Rgba16), true)
            }
            DynamicImage::ImageRgb32F(i) => {
                write::<colortype::RGB32Float, _>(encoder, width, height, i.as_raw(), fits(ColorType::Rgb32F), false)
            }
            DynamicImage::ImageRgba32F(i) => {
                write::<colortype::RGBA32Float, _>(encoder, width, height, i.as_raw(), fits(ColorType::Rgba32F), true)
            }
            other => return Err(TiffEncodeError::UnsupportedPixels(other.color())),
        };
        written?;
    }
    Ok(bytes.into_inner())
}

fn write<C: colortype::ColorType, W: Write + Seek>(
    encoder: &mut TiffEncoder<W>,
    width: u32,
    height: u32,
    samples: &[C::Inner],
    icc: Option<&[u8]>,
    alpha: bool,
) -> tiff::TiffResult<()>
where
    [C::Inner]: TiffValue,
{
    let mut image = encoder.new_image::<C>(width, height)?;
    if alpha {
        image.encoder().write_tag(Tag::ExtraSamples, UNASSOCIATED_ALPHA)?;
    }
    if let Some(icc) = icc {
        image.encoder().write_tag(Tag::IccProfile, IccProfile(icc))?;
    }
    image.write_data(samples)
}

/// `image` as RGBA in sRGB, its colors converted from `icc`, the RGB or gray profile it carries; without
/// a profile its samples are taken as sRGB. Fails when the profile does not read or does not describe
/// the pixels (a CMYK profile, which the decoder has already turned into RGB).
pub fn srgb_rgba8(image: &DynamicImage, icc: Option<&[u8]>) -> Result<RgbaImage, SrgbConversionError> {
    let mut rgba = image.to_rgba8();
    let Some(icc) = icc else { return Ok(rgba) };
    let profile = ColorProfile::new_from_slice(icc).map_err(SrgbConversionError::Profile)?;
    let srgb = ColorProfile::new_srgb();
    let transform = |from: Layout, to: Layout| {
        profile
            .create_transform_8bit(from, &srgb, to, TransformOptions::default())
            .map_err(SrgbConversionError::Transform)
    };
    let failed = SrgbConversionError::Conversion;

    match profile.color_space {
        DataColorSpace::Rgb if image.color().has_color() => {
            let source = rgba.as_raw().clone();
            transform(Layout::Rgba, Layout::Rgba)?.transform(&source, &mut rgba).map_err(failed)?;
        }
        // Gray goes to RGB without its alpha, because moxcms 0.7.11 reads a gray-with-alpha source as gray
        // alone; the alpha stays as it was.
        DataColorSpace::Gray if !image.color().has_color() => {
            let mut rgb = vec![0; rgba.len() / 4 * 3];
            transform(Layout::Gray, Layout::Rgb)?.transform(image.to_luma8().as_raw(), &mut rgb).map_err(failed)?;
            for (pixel, color) in rgba.pixels_mut().zip(rgb.as_chunks::<3>().0) {
                pixel.0[..3].copy_from_slice(color);
            }
        }
        space => return Err(SrgbConversionError::ProfileMismatch { space, color: image.color() }),
    }

    Ok(rgba)
}
