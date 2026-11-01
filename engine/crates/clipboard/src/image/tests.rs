use super::standins::{CodecCall, FakeCodec, FakeFormat};
use super::*;
use phonelink_core::config::features::{MAX_PASSES, PNG_LIMIT};

const L: usize = PNG_LIMIT;

#[test]
fn rounding_is_half_to_even() {
    assert_eq!(round_half_to_even(2.5), 2.0);
    assert_eq!(round_half_to_even(3.5), 4.0);
    assert_eq!(round_half_to_even(1.346), 1.0);
    assert_eq!(round_half_to_even(1.5), 2.0);
    assert_eq!(round_half_to_even(0.5), 0.0);
}

#[test]
fn pass_dimensions_follow_the_ratio() {
    // s ≤ 8.5: k = 1, each dimension divided by s.
    assert_eq!(pass_dimensions(4000, 3000, 2.0), (2000, 1500));
    assert_eq!(pass_dimensions(1000, 999, 3.0), (333, 333));
    // s = 21.25: s / 8.5 = 2.5 → k = 2 (a tie goes to even): W · 2 / 21.25.
    assert_eq!(pass_dimensions(4250, 2125, 21.25), (400, 200));
    // s = 29.75: s / 8.5 = 3.5 → k = 4: W · 4 / 29.75.
    assert_eq!(pass_dimensions(2975, 5950, 29.75), (400, 800));
    // s = 25.5: k = 3.
    assert_eq!(pass_dimensions(2550, 510, 25.5), (300, 60));
    // Truncation, not rounding.
    assert_eq!(pass_dimensions(3, 5, 2.0), (1, 2));
}

#[test]
fn small_images_are_sent_as_their_full_size_png() {
    let codec = FakeCodec::new();
    let image = FakeCodec::image(FakeFormat::Tiff, 100, 50);

    let png = reencode_png(&*codec, &image, L, MAX_PASSES).unwrap();

    assert!(is_png(&png));
    assert_eq!(png.len(), 16 + 5000);
    assert_eq!(codec.calls(), vec![CodecCall::Size, CodecCall::EncodePng { width: 100, height: 50 }]);

    // Exactly the limit is within it.
    codec.set_png_size(|_, _| L);
    assert_eq!(reencode_png(&*codec, &image, L, MAX_PASSES).unwrap().len(), L);
}

#[test]
fn a_large_image_is_downscaled_by_the_size_ratio() {
    let codec = FakeCodec::new();
    // 4000 x 3000 at one byte per pixel: s = 12000016 / 1048576 ≈ 11.44 → k = 1.
    let image = FakeCodec::image(FakeFormat::Bmp, 4000, 3000);

    let png = reencode_png(&*codec, &image, L, MAX_PASSES).unwrap();

    let s = 12_000_016f64 / L as f64;
    let expected = ((4000.0 / s) as u32, (3000.0 / s) as u32);
    assert_eq!(expected, (349, 262));
    assert_eq!(FakeCodec::decode(&png).map(|(_, w, h)| (w, h)), Some(expected));
    assert_eq!(
        codec.calls(),
        vec![
            CodecCall::Size,
            CodecCall::EncodePng { width: 4000, height: 3000 },
            CodecCall::ResizePng { width: 349, height: 262 },
        ]
    );
}

#[test]
fn each_pass_resizes_the_previous_result() {
    let codec = FakeCodec::new();
    // Twice the limit until the image is 500 px wide or less.
    codec.set_png_size(|w, _| if w > 500 { 2 * L } else { 100 });
    let image = FakeCodec::image(FakeFormat::Png, 4000, 3000);

    let png = reencode_png(&*codec, &image, L, MAX_PASSES).unwrap();
    assert_eq!(png.len(), 100);

    let resizes: Vec<_> = codec.calls().into_iter().filter(|c| matches!(c, CodecCall::ResizePng { .. })).collect();
    assert_eq!(
        resizes,
        vec![
            CodecCall::ResizePng { width: 2000, height: 1500 },
            CodecCall::ResizePng { width: 1000, height: 750 },
            CodecCall::ResizePng { width: 500, height: 375 },
        ]
    );
}

#[test]
fn five_passes_then_too_large() {
    let codec = FakeCodec::new();
    codec.set_png_size(|_, _| 2 * L);
    let image = FakeCodec::image(FakeFormat::Png, 4000, 3000);

    assert_eq!(reencode_png(&*codec, &image, L, MAX_PASSES), Err(ReencodeError::TooLarge { passes: 5, size: 2 * L }));
    let last = codec.calls().pop().unwrap();
    assert_eq!(last, CodecCall::ResizePng { width: 125, height: 93 });
}

#[test]
fn a_zero_dimension_fails_and_a_zero_sized_image_is_empty() {
    let codec = FakeCodec::new();
    codec.set_png_size(|_, _| 2 * L);
    let image = FakeCodec::image(FakeFormat::Png, 3, 3);

    // Pass 1: 3 / 2 → 1 x 1; pass 2: 1 / 2 → 0.
    assert_eq!(
        reencode_png(&*codec, &image, L, MAX_PASSES),
        Err(ReencodeError::ZeroDimension { pass: 2, width: 0, height: 0 })
    );

    let empty = FakeCodec::image(FakeFormat::Png, 0, 7);
    assert_eq!(reencode_png(&*codec, &empty, L, MAX_PASSES), Ok(Vec::new()));
    assert_eq!(announcement_png(&*codec, &empty), Ok(Vec::new()));
}

#[test]
fn codec_failures() {
    let codec = FakeCodec::new();
    assert!(matches!(reencode_png(&*codec, b"garbage", L, MAX_PASSES), Err(ReencodeError::Decode(_))));
    assert!(announcement_png(&*codec, b"garbage").is_err(), "a decode failure is a read failure");

    let image = FakeCodec::image(FakeFormat::Tiff, 10, 10);
    codec.fail_png(true);
    assert!(matches!(reencode_png(&*codec, &image, L, MAX_PASSES), Err(ReencodeError::Encode(_))));
    assert_eq!(announcement_png(&*codec, &image), Ok(Vec::new()), "the hash of no bytes is announced");

    codec.fail_png(false);
    codec.set_png_size(|w, _| if w > 5 { 2 * L } else { 10 });
    codec.fail_resize(true);
    assert!(matches!(reencode_png(&*codec, &image, L, MAX_PASSES), Err(ReencodeError::Resize { pass: 1, .. })));
}

#[test]
fn received_images_keep_png_bytes_and_add_tiff() {
    let codec = FakeCodec::new();
    let png = FakeCodec::image_with_len(FakeFormat::Png, 3, 2, 40);
    let (out_png, tiff) = pasteboard_image(&*codec, &png).unwrap();
    assert_eq!(out_png, png, "PNG bytes are written as received");
    assert_eq!(FakeCodec::decode(&tiff), Some((FakeFormat::Tiff, 3, 2)));

    let bmp = FakeCodec::image(FakeFormat::Bmp, 3, 2);
    let (out_png, _) = pasteboard_image(&*codec, &bmp).unwrap();
    assert!(is_png(&out_png), "other formats are re-encoded");
    assert!(pasteboard_image(&*codec, b"not an image").is_err());

    codec.fail_tiff(true);
    assert!(pasteboard_image(&*codec, &png).is_err());
}
