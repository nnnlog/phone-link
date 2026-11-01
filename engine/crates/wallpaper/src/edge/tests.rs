use super::standins::*;
use super::testing::*;
use super::*;
use prost::Message;

#[test]
fn the_size_rule() {
    let s = |w, h| {
        let p = wallpaper_scaled_size(w, h);
        (p.width, p.height)
    };

    assert_eq!(s(1920, 512), (1920, 512), "512 high is kept");
    assert_eq!(s(300, 200), (300, 200));
    assert_eq!(s(5120, 2880), (910, 512), "910.22 rounds down");
    assert_eq!(s(1920, 1080), (910, 512), "910.22");
    assert_eq!(s(2560, 1600), (819, 512), "819.2");
    assert_eq!(s(3, 1024), (2, 512), "1.5 rounds to the even 2");
    assert_eq!(s(5, 1024), (2, 512), "2.5 rounds to the even 2");
    assert_eq!(s(7, 1024), (4, 512), "3.5 rounds to the even 4");
    assert_eq!(s(1, 4096), (0, 512), "0.125");
}

#[test]
fn the_three_answers() {
    let pc = FixedPcWallpaper::new(PcWallpaper::Picture { jpeg: TINY_JPEG.to_vec() });
    let answers = PcWallpaperAnswers::new(pc.clone());
    let r = answers.answer();

    assert_eq!(r.status, WallpaperResponseStatus::Ok as i32);
    assert_eq!(r.wallpapers.len(), 1);
    assert_eq!(r.wallpapers[0].wallpaper_type, WallpaperType::Homescreen as i32);
    assert_eq!(r.wallpapers[0].payload.as_deref(), Some(TINY_JPEG.as_slice()));
    assert_eq!(r.wallpapers[0].id, 0);

    pc.set(PcWallpaper::Unreadable);
    assert_eq!(answers.answer().encode_to_vec(), [0x0A, 0x02, 0x10, 0x01]);
    pc.set(PcWallpaper::NotSet);
    assert_eq!(answers.answer().encode_to_vec(), [0x0A, 0x00]);
    pc.set(PcWallpaper::Picture { jpeg: b"\x89PNG".to_vec() });
    assert_eq!(answers.answer().encode_to_vec(), [0x0A, 0x02, 0x10, 0x01], "not a JPEG");
}

#[test]
fn a_failed_read_is_answered_with_the_last_picture_else_as_unreadable() {
    let answers = PcWallpaperAnswers::new(FixedPcWallpaper::failing());
    assert_eq!(answers.answer().encode_to_vec(), [0x0A, 0x02, 0x10, 0x01], "nothing read yet");

    let pc = FixedPcWallpaper::new(PcWallpaper::Picture { jpeg: TINY_JPEG.to_vec() });
    let answers = PcWallpaperAnswers::new(pc.clone());
    let picture = answers.answer();
    pc.fail();
    assert_eq!(answers.answer(), picture, "the last picture");

    pc.set(PcWallpaper::NotSet);
    assert_eq!(answers.answer().encode_to_vec(), [0x0A, 0x00]);
    pc.fail();
    assert_eq!(answers.answer().encode_to_vec(), [0x0A, 0x02, 0x10, 0x01], "no picture forgets the last one");
}
