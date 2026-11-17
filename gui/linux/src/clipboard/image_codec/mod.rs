//! The image codec the clipboard feature converts pictures with: their size, PNG in their own pixel
//! format, a resized PNG and TIFF, each with the picture's ICC profile.

use image::imageops::{self, FilterType};
use image::{DynamicImage, Rgba, RgbaImage};
use phonelink_clipboard::image::{ClipboardImageCodec, ClipboardImageSize};
use phonelink_engine::edge::EdgeError;

use crate::image_io;

#[cfg(test)]
mod tests;

/// The clipboard's image codec on the `image` crate.
pub struct ImageCrateCodec;

/// The picture of `bytes` and its ICC profile.
fn decode(bytes: &[u8]) -> Result<(DynamicImage, Option<Vec<u8>>), EdgeError> {
    image_io::decode(bytes).map_err(|e| EdgeError::Unexpected { message: format!("image decode: {e}") })
}

/// A failure to encode the picture, as the engine's error.
fn encode_error(e: impl std::fmt::Display) -> EdgeError {
    EdgeError::Unexpected { message: format!("image encode: {e}") }
}

/// `image` resampled to `width` × `height` as 8-bit RGBA, with premultiplied alpha, so that the colors of
/// transparent pixels do not bleed into their neighbors.
fn resample(image: &DynamicImage, width: u32, height: u32) -> RgbaImage {
    let mut premultiplied = image.to_rgba8();
    premultiplied.pixels_mut().for_each(premultiply);
    let mut resized = imageops::resize(&premultiplied, width, height, FilterType::CatmullRom);
    resized.pixels_mut().for_each(unpremultiply);
    resized
}

fn premultiply(pixel: &mut Rgba<u8>) {
    let alpha = u16::from(pixel[3]);
    for channel in &mut pixel.0[..3] {
        *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
    }
}

/// Resampling can leave a color above its alpha; it is clamped.
fn unpremultiply(pixel: &mut Rgba<u8>) {
    let alpha = u16::from(pixel[3]);
    for channel in &mut pixel.0[..3] {
        *channel = (u16::from(*channel) * 255 + alpha / 2).checked_div(alpha).map_or(0, |value| value.min(255) as u8);
    }
}

impl ClipboardImageCodec for ImageCrateCodec {
    fn size(&self, image: Vec<u8>) -> Result<ClipboardImageSize, EdgeError> {
        let (decoded, _) = decode(&image)?;
        Ok(ClipboardImageSize { width: decoded.width(), height: decoded.height() })
    }

    /// In the decoded image's own pixel format, with its ICC profile.
    fn encode_png(&self, image: Vec<u8>) -> Result<Vec<u8>, EdgeError> {
        let (decoded, icc) = decode(&image)?;
        image_io::png(&decoded, icc.as_deref()).map_err(encode_error)
    }

    /// RGBA with 8 bits a channel, keeping an RGB profile; a gray image is drawn into RGB.
    fn resize_png(&self, image: Vec<u8>, width: u32, height: u32) -> Result<Vec<u8>, EdgeError> {
        let (decoded, icc) = decode(&image)?;
        if width == 0 || height == 0 {
            return Err(EdgeError::Unexpected { message: format!("cannot resize to {width}×{height}") });
        }
        let resized = DynamicImage::ImageRgba8(resample(&decoded, width, height));
        image_io::png(&resized, icc.as_deref()).map_err(encode_error)
    }

    /// RGBA with 8 bits a channel, keeping an RGB profile.
    fn encode_tiff(&self, image: Vec<u8>) -> Result<Vec<u8>, EdgeError> {
        let (decoded, icc) = decode(&image)?;
        image_io::tiff(&DynamicImage::ImageRgba8(decoded.to_rgba8()), icc.as_deref()).map_err(encode_error)
    }
}
