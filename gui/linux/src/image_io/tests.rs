use image::{GrayAlphaImage, LumaA, Rgb, RgbImage, Rgba};

use super::testing::display_p3;
use super::*;

#[test]
fn profiles_fit_pixels_of_their_color_space() {
    let p3 = display_p3();
    assert!(profile_fits(&p3, ColorType::Rgba8));
    assert!(!profile_fits(&p3, ColorType::L8));
    let gray = ColorProfile::new_gray_with_gamma(2.2).encode().expect("the profile encodes");
    assert!(profile_fits(&gray, ColorType::La16));
    assert!(!profile_fits(&gray, ColorType::Rgb8));
    assert!(!profile_fits(b"short", ColorType::Rgb8));
}

#[test]
fn png_keeps_the_pixel_format_and_the_profile() {
    let gray = DynamicImage::ImageLuma8(image::GrayImage::from_pixel(3, 2, image::Luma([90])));
    let (decoded, icc) = decode(&png(&gray, Some(&display_p3())).unwrap()).unwrap();
    assert_eq!(decoded.color(), ColorType::L8);
    assert_eq!(icc, None, "an RGB profile is not written for gray pixels");

    let deep = DynamicImage::ImageRgb16(image::ImageBuffer::from_pixel(3, 2, Rgb([1000u16, 2000, 3000])));
    let (decoded, icc) = decode(&png(&deep, Some(&display_p3())).unwrap()).unwrap();
    assert_eq!(decoded, deep);
    assert_eq!(icc, Some(display_p3()));

    let float = DynamicImage::ImageRgba32F(image::ImageBuffer::from_pixel(2, 2, Rgba([0.5f32, 0.25, 1.0, 1.0])));
    let (decoded, _) = decode(&png(&float, None).unwrap()).unwrap();
    assert_eq!(decoded.color(), ColorType::Rgba16);
}

#[test]
fn tiff_keeps_the_profile_and_writes_gray_with_alpha_as_rgba() {
    let rgb = DynamicImage::ImageRgb8(RgbImage::from_pixel(4, 3, Rgb([10, 20, 30])));
    let (decoded, icc) = decode(&tiff(&rgb, Some(&display_p3())).unwrap()).unwrap();
    assert_eq!(decoded, rgb);
    assert_eq!(icc, Some(display_p3()));

    let gray_alpha = DynamicImage::ImageLumaA8(GrayAlphaImage::from_pixel(4, 3, LumaA([200, 128])));
    let (decoded, _) = decode(&tiff(&gray_alpha, None).unwrap()).unwrap();
    assert_eq!(decoded, DynamicImage::ImageRgba8(gray_alpha.to_rgba8()));
}

#[test]
fn jpeg_carries_the_profile() {
    let rgb = DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 8, Rgb([200, 100, 50])));
    let (_, icc) = decode(&jpeg(&rgb, 90, Some(&display_p3())).unwrap()).unwrap();
    assert_eq!(icc, Some(display_p3()));
}

#[test]
fn colors_are_converted_to_srgb() {
    let red = DynamicImage::ImageRgb8(RgbImage::from_pixel(2, 2, Rgb([255, 0, 0])));
    let converted = srgb_rgba8(&red, Some(&display_p3())).unwrap();
    let [r, g, b, a] = converted.get_pixel(0, 0).0;
    assert_eq!((r, a), (255, 255));
    assert!(g < 10 && b < 10, "Display P3 red is sRGB red, clipped: {:?}", converted.get_pixel(0, 0));

    let untagged = srgb_rgba8(&red, None).unwrap();
    assert_eq!(untagged.get_pixel(1, 1).0, [255, 0, 0, 255]);

    let gray = ColorProfile::new_gray_with_gamma(2.2).encode().unwrap();
    let mid = DynamicImage::ImageLumaA8(GrayAlphaImage::from_pixel(1, 1, LumaA([128, 90])));
    let [r, g, b, a] = srgb_rgba8(&mid, Some(&gray)).unwrap().get_pixel(0, 0).0;
    assert!(r == g && g == b && r.abs_diff(128) < 8, "gray stays gray: {r} {g} {b}");
    assert_eq!(a, 90, "alpha is kept");

    assert!(srgb_rgba8(&red, Some(&gray)).is_err(), "a gray profile does not describe RGB pixels");
    assert!(srgb_rgba8(&red, Some(b"not a profile")).is_err());
}
