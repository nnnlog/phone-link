use super::*;

fn view(id: i64, app: Option<&str>, title: Option<&str>, artist: Option<&str>) -> MediaSessionView {
    MediaSessionView {
        id,
        app_name: app.map(str::to_owned),
        title: title.map(str::to_owned),
        artist: artist.map(str::to_owned),
        album_art: None,
        app_icon: None,
        playing: true,
    }
}

fn controls_with(selected_id: Option<i64>, shown: bool) -> MediaControlsState {
    MediaControlsState {
        supported: true,
        setting: true,
        notifications_permission: true,
        shown,
        sessions: vec![view(1, Some("Music"), Some("Blue Hour"), None), view(2, Some("Podcasts"), None, None)],
        selected_id,
        commanding: false,
    }
}

#[test]
fn a_session_falls_back_to_its_app_and_media() {
    let session = Session::of(&view(1, Some("Podcasts"), Some(""), Some("")));
    assert_eq!((session.title.as_str(), session.artist), ("Podcasts", None));
    let named = Session::of(&view(1, Some("Music"), Some("Blue Hour"), Some("The Paper Lanterns")));
    assert_eq!(named.subtitle(), "The Paper Lanterns · Music");
    let anonymous = Session::of(&view(1, None, None, None));
    assert_eq!(anonymous.title, anonymous.app);
}

#[test]
fn the_controls_show_the_selected_session_only() {
    assert_eq!(selected(&controls_with(Some(2), true)).map(|s| s.id), Some(2));
    assert!(shown(&controls_with(Some(1), true)));
    // No selection, or a selection the phone no longer has, shows nothing: no session is guessed.
    assert!(!shown(&controls_with(None, true)));
    assert!(!shown(&controls_with(Some(9), true)));
    assert!(!shown(&controls_with(Some(1), false)));
}

#[test]
fn the_setting_says_why_it_does_not_apply() {
    let mut controls = controls_with(Some(1), true);
    assert!(setting_applies(&controls));
    assert_eq!(setting_detail(None), "Control what plays on your phone from this computer.");
    controls.notifications_permission = false;
    assert!(!setting_applies(&controls));
    assert!(setting_detail(Some(&controls)).starts_with("This needs notification access"));
    controls.supported = false;
    assert_eq!(setting_detail(Some(&controls)), "This phone doesn't support media controls.");
}

#[test]
fn an_app_icon_becomes_a_white_glyph_shaped_by_its_brightness() {
    let mut icon = image::RgbImage::new(3, 1);
    icon.put_pixel(0, 0, image::Rgb([255, 255, 255]));
    icon.put_pixel(1, 0, image::Rgb([0, 0, 0]));
    icon.put_pixel(2, 0, image::Rgb([0, 128, 64]));
    let mut png = std::io::Cursor::new(Vec::new());
    icon.write_to(&mut png, image::ImageFormat::Png).expect("a PNG encodes");

    let glyph = glyph_png(png.get_ref()).expect("a glyph");
    let glyph = image::load_from_memory(&glyph).expect("the glyph decodes").to_rgba8();
    let alphas: Vec<u8> = glyph.pixels().map(|p| p.0[3]).collect();
    assert_eq!(alphas, vec![255, 0, 128]);
    assert!(glyph.pixels().all(|p| p.0[..3] == [255, 255, 255]));
    assert_eq!(glyph_png(b"not an image"), None);
}
