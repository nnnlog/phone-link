use std::io::Cursor;

use image::{Rgb, Rgba, RgbaImage};

use super::*;

fn encoded(image: &DynamicImage, format: ImageFormat) -> Vec<u8> {
    let mut output = Cursor::new(Vec::new());
    image.write_to(&mut output, format).expect("the test image encodes");
    output.into_inner()
}

fn gradient(width: u32, height: u32) -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_fn(width, height, |x, y| {
        Rgb([(x * 7 % 256) as u8, (y * 13 % 256) as u8, ((x + y) * 3 % 256) as u8])
    }))
}

#[test]
fn images_are_recognized_by_their_signature() {
    assert_eq!(sniff(&encoded(&gradient(4, 4), ImageFormat::Png)), Some("image/png"));
    assert_eq!(sniff(&encoded(&gradient(4, 4), ImageFormat::Jpeg)), Some("image/jpeg"));
    assert_eq!(sniff(b"\0\0\0\x18ftypheic\0\0\0\0"), Some("image/heic"));
    assert_eq!(sniff(b"\0\0\0\x18ftypmif1\0\0\0\0"), Some("image/heif"));
    assert_eq!(sniff(b"P3 not a picture"), None);
    assert_eq!(sniff(b"hello"), None);
}

#[test]
fn an_image_within_the_limits_comes_back_unchanged() {
    let png = encoded(&gradient(40, 20), ImageFormat::Png);
    let fitted = fit_image(&png, 100, 100, png.len() as u64).expect("fits").expect("an image");
    assert_eq!(fitted.bytes, png);
    assert_eq!(fitted.content_type, "image/png");
}

#[test]
fn a_large_image_is_scaled_into_the_box_keeping_its_format() {
    let png = encoded(&gradient(400, 200), ImageFormat::Png);
    let fitted = fit_image(&png, 100, 100, 1_000_000).expect("fits").expect("an image");
    assert_eq!(fitted.content_type, "image/png");
    let shown = image::load_from_memory(&fitted.bytes).expect("the result decodes");
    assert_eq!((shown.width(), shown.height()), (100, 50));
}

#[test]
fn a_tight_budget_falls_back_to_jpeg_over_white() {
    // Noise, which PNG cannot shrink: JPEG fits the budget at a larger size first.
    let noise = |x: u32, y: u32, salt: u32| ((x.wrapping_mul(7919) ^ y.wrapping_mul(104_729) ^ salt) % 251) as u8;
    let transparent = DynamicImage::ImageRgba8(RgbaImage::from_fn(300, 300, |x, y| {
        Rgba([noise(x, y, 1), noise(x, y, 2), noise(x, y, 3), if x < 150 { 0 } else { 255 }])
    }));
    let png = encoded(&transparent, ImageFormat::Png);
    let fitted = fit_image(&png, 300, 300, 6_000).expect("fits").expect("an image");
    assert_eq!(fitted.content_type, "image/jpeg");
    assert!(fitted.bytes.len() <= 6_000);
    let shown = image::load_from_memory(&fitted.bytes).expect("the result decodes").to_rgb8();
    let left = shown.get_pixel(2, shown.height() / 2);
    assert!(left.0.iter().all(|channel| *channel > 200), "transparent pixels turn white: {left:?}");
}

#[test]
fn nothing_fitting_is_an_error_and_unknown_bytes_are_left_alone() {
    let png = encoded(&gradient(64, 64), ImageFormat::Png);
    assert!(fit_image(&png, 32, 32, 10).is_err());
    assert!(fit_image(&png, 0, 32, 10_000).is_err());
    assert_eq!(fit_image(b"not an image", 10, 10, 10).expect("no failure"), None);
    assert_eq!(fit_image(b"\0\0\0\x18ftypheic\0\0\0\0", 10, 10, 10).expect("no failure"), None);
}

#[test]
fn a_gif_keeps_its_frames_and_loop_count() {
    let frames: Vec<Frame> = (0..3)
        .map(|i| {
            let buffer = RgbaImage::from_fn(120, 60, |x, _| Rgba([(x * 2) as u8, i * 80, 0, 255]));
            Frame::from_parts(buffer, 0, 0, image::Delay::from_numer_denom_ms(100, 1))
        })
        .collect();
    let mut gif = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut gif);
        encoder.set_repeat(Repeat::Infinite).expect("the loop count is set");
        encoder.encode_frames(frames).expect("the test GIF encodes");
    }
    assert!(matches!(gif_repeat(&gif), Some(Repeat::Infinite)));

    let fitted = fit_image(&gif, 60, 60, 1_000_000).expect("fits").expect("an image");
    assert_eq!(fitted.content_type, "image/gif");
    assert!(matches!(gif_repeat(&fitted.bytes), Some(Repeat::Infinite)));
    let decoded = GifDecoder::new(Cursor::new(fitted.bytes.clone()))
        .and_then(|gif| gif.into_frames().collect_frames())
        .expect("the result decodes");
    assert_eq!(decoded.len(), 3);
    assert_eq!(decoded[0].buffer().dimensions(), (60, 30));
}

#[test]
fn a_gray_image_with_alpha_is_written_as_an_rgba_tiff() {
    let gray_alpha = DynamicImage::ImageLumaA8(image::GrayAlphaImage::from_pixel(80, 40, image::LumaA([90, 200])));
    let fitted =
        encode(&Source::Still(gray_alpha), None, ImageFormat::Tiff, 20, 1_000_000).expect("encodes").expect("fits");
    assert_eq!(fitted.content_type, "image/tiff");
    let shown = image::load_from_memory(&fitted.bytes).expect("the result decodes");
    assert_eq!((shown.width(), shown.height(), shown.color()), (20, 10, image::ColorType::Rgba8));
}

#[test]
fn a_fitted_image_keeps_its_color_profile() {
    let p3 = image_io::testing::display_p3();
    let png = image_io::png(&gradient(400, 200), Some(&p3)).expect("the test image encodes");
    let fitted = fit_image(&png, 100, 100, 1_000_000).expect("fits").expect("an image");
    assert_eq!(fitted.content_type, "image/png");
    assert_eq!(image_io::decode(&fitted.bytes).expect("decodes").1.as_deref(), Some(&p3[..]));

    // JPEG keeps the profile of opaque pixels…
    let opaque = Source::Still(gradient(400, 200));
    let jpeg = encode(&opaque, Some(&p3), ImageFormat::Jpeg, 100, 1_000_000).expect("encodes").expect("fits");
    assert_eq!(image_io::decode(&jpeg.bytes).expect("decodes").1.as_deref(), Some(&p3[..]));
    // …and draws transparent ones over white in sRGB, without it.
    let transparent = Source::Still(DynamicImage::ImageRgba8(gradient(40, 20).to_rgba8()));
    let jpeg = encode(&transparent, Some(&p3), ImageFormat::Jpeg, 40, 1_000_000).expect("encodes").expect("fits");
    assert_eq!(image_io::decode(&jpeg.bytes).expect("decodes").1, None);
    // A gray image stays gray.
    let gray = Source::Still(DynamicImage::ImageLuma8(image::GrayImage::from_pixel(40, 20, image::Luma([100]))));
    let jpeg = encode(&gray, None, ImageFormat::Jpeg, 40, 1_000_000).expect("encodes").expect("fits");
    assert_eq!(image::load_from_memory(&jpeg.bytes).expect("decodes").color(), image::ColorType::L8);
}
