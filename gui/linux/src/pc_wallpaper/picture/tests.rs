use super::*;
use crate::pc_wallpaper::testing::{TestDir, encoded_png, jpeg_pixel, near};

#[test]
fn a_picture_is_scaled_to_the_phones_height_and_sent_as_jpeg() {
    let dir = std::env::temp_dir().join(format!("phonelink-wallpaper-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let tall = dir.join("tall.png");
    image::RgbaImage::from_pixel(300, 1024, image::Rgba([10, 200, 30, 255])).save(&tall).unwrap();
    let PcWallpaper::Picture { jpeg } = picture_of(&tall) else { panic!("a picture") };
    let decoded = image::load_from_memory(&jpeg).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (150, 512));
    assert!(jpeg.starts_with(&[0xFF, 0xD8, 0xFF]));

    let broken = dir.join("broken.png");
    std::fs::write(&broken, b"not a picture").unwrap();
    assert_eq!(picture_of(&broken), PcWallpaper::Unreadable);
    assert_eq!(picture_of(&dir.join("missing.png")), PcWallpaper::NotSet);
    assert_eq!(picture_of(&dir), PcWallpaper::Unreadable);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// An 8 × 4 lossless JPEG XL: its left half orange (230, 120, 20), its right half blue (20, 60, 200).
const ORANGE_AND_BLUE_JXL: [u8; 191] = [
    0xFF, 0x0A, 0x18, 0x00, 0x0E, 0x80, 0x48, 0x08, 0x02, 0x01, 0x00, 0xC8, 0x02, 0x4B, 0x18, 0x9B, 0x9C, 0x71, 0x84,
    0x03, 0x38, 0x80, 0x03, 0x38, 0x20, 0x4A, 0xC0, 0x39, 0x05, 0x01, 0x00, 0x20, 0x44, 0x80, 0x08, 0x10, 0x01, 0x22,
    0x40, 0x84, 0xFF, 0xF7, 0xEF, 0xF9, 0xEF, 0xA1, 0x31, 0xE7, 0x9C, 0x6B, 0xED, 0x73, 0x6F, 0x92, 0x24, 0x09, 0x01,
    0x55, 0x55, 0x55, 0x55, 0x55, 0xD5, 0xFF, 0xFF, 0xFF, 0x73, 0xEF, 0xEB, 0xEE, 0xEE, 0xEE, 0x86, 0xFF, 0xF7, 0xEF,
    0xF9, 0xEF, 0xA1, 0x31, 0xE7, 0x9C, 0x6B, 0xED, 0x73, 0x6F, 0x92, 0x24, 0x09, 0x01, 0x55, 0x55, 0x55, 0x55, 0x55,
    0xD5, 0xFF, 0xFF, 0xFF, 0x73, 0xEF, 0xEB, 0xEE, 0xEE, 0xEE, 0x86, 0xFF, 0xF7, 0xEF, 0xF9, 0xEF, 0xA1, 0x31, 0xE7,
    0x9C, 0x6B, 0xED, 0x73, 0x6F, 0x92, 0x24, 0x09, 0x01, 0x55, 0x55, 0x55, 0x55, 0x55, 0xD5, 0xFF, 0xFF, 0xFF, 0x73,
    0xEF, 0xEB, 0xEE, 0xEE, 0xEE, 0x86, 0xFF, 0xF7, 0xEF, 0xF9, 0xEF, 0xA1, 0x31, 0xE7, 0x9C, 0x6B, 0xED, 0x73, 0x6F,
    0x92, 0x24, 0x09, 0x01, 0x55, 0x55, 0x55, 0x55, 0x55, 0xD5, 0xFF, 0xFF, 0xFF, 0x73, 0xEF, 0xEB, 0xEE, 0xEE, 0xEE,
    0x3E, 0x00, 0x17, 0x1D, 0x38, 0x09, 0xC0, 0x0D, 0x0F, 0x52, 0xE0, 0xB9, 0x10, 0xE0, 0x86, 0x06, 0x38, 0x19, 0xC0,
    0x0D,
];

#[test]
fn gnome_jpeg_xl_and_svg_pictures_are_read() {
    let dir = TestDir::new("wallpaper-formats");
    let jxl = dir.write("adwaita-l.jxl", ORANGE_AND_BLUE_JXL);
    let (width, height, left) = jpeg_pixel(picture_of(&jxl), 0, 1);
    assert_eq!((width, height), (8, 4));
    assert!(near(left, [230, 120, 20]), "{left:?}");
    assert!(near(jpeg_pixel(picture_of(&jxl), 7, 2).2, [20, 60, 200]));

    let svg = dir.write(
        "blobs-l.svg",
        r##"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" width="2000" height="1000">
                <rect width="1000" height="1000" fill="#ff0000"/>
                <rect x="1000" width="1000" height="1000" fill="#0000ff" fill-opacity="0.5"/></svg>"##,
    );
    let (width, height, left) = jpeg_pixel(picture_of(&svg), 100, 256);
    assert_eq!((width, height), (1024, 512), "drawn at the phone's height");
    assert!(near(left, [255, 0, 0]), "{left:?}");
    assert!(near(jpeg_pixel(picture_of(&svg), 900, 256).2, [0, 0, 128]), "half-transparent over black");

    let other = dir.write("other.xml", "<?xml version=\"1.0\"?><wallpapers/>");
    assert_eq!(picture_of(&other), PcWallpaper::Unreadable);
}

#[test]
fn a_picture_with_a_color_profile_is_sent_in_srgb() {
    let dir = TestDir::new("wallpaper-icc");
    let color = image::Rgb([60, 160, 90]);
    let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(16, 16, color));
    let p3 = crate::image_io::testing::display_p3();
    let tagged = dir.write("p3.png", crate::image_io::png(&image, Some(&p3)).unwrap());
    let untagged = dir.write("plain.png", crate::image_io::png(&image, None).unwrap());

    let expected = crate::image_io::srgb_rgba8(&image, Some(&p3)).unwrap().get_pixel(0, 0).0;
    let converted = jpeg_pixel(picture_of(&tagged), 8, 8).2;
    assert!(near(converted, [expected[0], expected[1], expected[2]]), "{converted:?} for {expected:?}");
    assert!(!near(converted, color.0), "the colors are converted");
    assert!(near(jpeg_pixel(picture_of(&untagged), 8, 8).2, color.0));
}

#[test]
fn a_slideshow_file_stands_for_the_picture_it_shows() {
    let dir = TestDir::new("wallpaper-slideshow");
    dir.write("day.png", encoded_png(&RgbImage::from_pixel(4, 2, image::Rgb([200, 30, 30]))));
    let xml =
        |file: &str| format!("<background><static><duration>60</duration><file>{file}</file></static></background>");
    let slideshow = dir.write("timed.xml", xml("day.png"));
    let (_, _, color) = jpeg_pixel(picture_of(&slideshow), 1, 1);
    assert!(near(color, [200, 30, 30]), "{color:?}");

    assert_eq!(picture_of(&dir.write("gone.xml", xml("gone.png"))), PcWallpaper::NotSet);
    assert_eq!(picture_of(&dir.write("nested.xml", xml("timed.xml"))), PcWallpaper::Unreadable);
    assert_eq!(picture_of(&dir.write("broken.xml", "<background><static>")), PcWallpaper::Unreadable);
}
