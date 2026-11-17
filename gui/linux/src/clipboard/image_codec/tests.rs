use image::{ColorType, GrayImage, Luma};

use super::*;
use crate::image_io::testing::display_p3;

fn png_of(image: &DynamicImage, icc: Option<&[u8]>) -> Vec<u8> {
    image_io::png(image, icc).unwrap()
}

#[test]
fn png_round_trips_through_resize() {
    let png = png_of(&DynamicImage::new_rgba8(8, 4), None);
    let resized = ImageCrateCodec.resize_png(png, 4, 2).unwrap();
    assert_eq!(ImageCrateCodec.size(resized).unwrap(), ClipboardImageSize { width: 4, height: 2 });
    let png = png_of(&DynamicImage::new_rgba8(8, 4), None);
    assert!(ImageCrateCodec.resize_png(png, 0, 2).is_err());
}

#[test]
fn the_png_keeps_the_decoded_pixel_format_and_profile() {
    let gray = DynamicImage::ImageLuma8(GrayImage::from_pixel(16, 16, Luma([77])));
    let png = ImageCrateCodec.encode_png(png_of(&gray, None)).unwrap();
    assert_eq!(image::load_from_memory(&png).unwrap().color(), ColorType::L8);

    let rgb = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(16, 16, image::Rgb([1, 2, 3])));
    let tagged = png_of(&rgb, Some(&display_p3()));
    for output in [
        ImageCrateCodec.encode_png(tagged.clone()).unwrap(),
        ImageCrateCodec.resize_png(tagged.clone(), 8, 8).unwrap(),
        ImageCrateCodec.encode_tiff(tagged.clone()).unwrap(),
    ] {
        assert_eq!(image_io::decode(&output).unwrap().1, Some(display_p3()));
    }
    let tiff = image_io::decode(&ImageCrateCodec.encode_tiff(tagged).unwrap()).unwrap().0;
    assert_eq!(tiff.color(), ColorType::Rgba8);
}

#[test]
fn resizing_does_not_bleed_transparent_colors() {
    // Transparent red beside opaque blue: the red never shows at the edge.
    let image = RgbaImage::from_fn(16, 4, |x, _| if x < 8 { Rgba([255, 0, 0, 0]) } else { Rgba([0, 0, 255, 255]) });
    let resized = image::load_from_memory(
        &ImageCrateCodec.resize_png(png_of(&DynamicImage::ImageRgba8(image), None), 5, 2).unwrap(),
    )
    .unwrap()
    .to_rgba8();
    for pixel in resized.pixels().filter(|p| p[3] > 0) {
        assert!(pixel[0] <= 2, "no red in {pixel:?}");
    }
}
