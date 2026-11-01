use super::standins::*;
use super::testing::*;
use super::*;

#[test]
fn fake_imaging_truncates_recognised_images_only() {
    let img = FakeImaging::default();
    let png = [b"\x89PNG".as_slice(), &[0u8; 20]].concat();

    let out = img.fit_image(png.clone(), 10, 10, 8).unwrap().unwrap();
    assert_eq!(out.bytes.len(), 8);
    assert_eq!(out.content_type, "image/png");

    assert_eq!(img.fit_image(b"plain".to_vec(), 10, 10, 2).unwrap(), None);
    assert_eq!(img.sniff_content_type(png), Some("image/png".into()));

    assert_eq!(FakeFileTypes.family("text/x-vcard".into()), FileFamily::VCard);
    assert_eq!(FakeFileTypes.file_extension("image/jpeg".into()).as_deref(), Some(".jpg"));
}
