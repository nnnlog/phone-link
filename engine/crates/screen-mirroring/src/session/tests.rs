use super::*;
use phonelink_nano::channel::testing::pair;

fn ch(kind: u32, props: &str) -> RawChannel {
    let class = "Microsoft::Basix::Dct::Channel::Class::X";
    pair(&format!("{class},9,{kind},{props}"), 1400).0.0
}

#[test]
fn sets_group_by_session_and_need_all_four() {
    let mut g = Grouping::default();
    let p = "workflow=remote&type=container&packageName=com.a&sessionId=s1&setId=s1";
    assert!(matches!(g.offer(ch(4, p)), Grouped::Held));
    assert!(matches!(g.offer(ch(1, p)), Grouped::Held), "VideoSource is the PC's VideoTarget");
    assert!(matches!(g.offer(ch(2, p)), Grouped::Held));
    assert!(matches!(g.offer(ch(1, p)), Grouped::Held), "a later video replaces the earlier");

    assert!(matches!(g.offer(ch(4, "workflow=remote&packageName=com.a&sessionId=s2")), Grouped::Held));
    assert!(matches!(g.offer(ch(5, "workflow=remote&packageName=com.b&sessionId=s1")), Grouped::Closed));

    match g.offer(ch(5, p)) {
        Grouped::Complete { session_id, set } => {
            assert_eq!((session_id.as_str(), set.package.as_deref()), ("s1", Some("com.a")));
        }
        _ => panic!("complete"),
    }

    assert!(matches!(g.offer(ch(6, p)), Grouped::Closed), "audio is not part of a window set");
    assert!(matches!(g.offer(ch(4, "workflow=remote")), Grouped::Closed));
}

#[test]
fn display_transform_and_deferred_video() {
    let mut d = Display::default();
    d.device_resolution_changed((1080, 2400));
    d.video_size((720, 1600));
    assert_eq!((d.device_resolution(), d.video, d.rotation()), (Some((1080, 2400)), Some((720, 1600)), 0));

    d.orientation = 1;
    assert_eq!((d.device_resolution(), d.rotation()), (Some((2400, 1080)), -90));

    d.video_size((1600, 720));
    assert_eq!(d.video, Some((720, 1600)), "held: disagrees with the device resolution");

    d.device_resolution_changed((2400, 1080));
    assert_eq!(d.video, Some((1600, 720)), "applied with the matching resolution");

    // A held size that still disagrees with the next resolution is dropped.
    d.video_size((720, 1600));
    d.device_resolution_changed((2400, 1080));
    d.device_resolution_changed((1080, 2400));
    assert_eq!(d.video, Some((1600, 720)), "the held size was dropped");

    // A square size is not portrait.
    let mut sq = Display::default();
    sq.device_resolution_changed((2400, 1080));
    sq.video_size((1000, 1000));
    sq.video_size((1000, 1000));
    assert_eq!(sq.video, Some((1000, 1000)), "square agrees with landscape");
}

/// The phone record's device resolution reads as "<width>,<height>", as the phone registers it.
#[test]
fn the_record_resolution_is_width_comma_height() {
    assert_eq!(record_resolution("1440,3120"), Some((1440, 3120)));
    assert_eq!(record_resolution(" 1080 , 2400 "), Some((1080, 2400)));
    assert_eq!(record_resolution("1080x2400"), None);
    assert_eq!(record_resolution("1,2,3"), None, "a third part");
    assert_eq!(record_resolution("0,2400"), None);
    assert_eq!(record_resolution(""), None);

    // A window starts with it; the phone's own size replaces the current one, not the start.
    let mut d = Display::default();
    d.start_with_record_resolution("1440,3120");
    assert_eq!((d.initial_device_resolution, d.device_resolution()), (Some((1440, 3120)), Some((1440, 3120))));

    d.device_resolution_changed((1080, 2400));
    assert_eq!((d.initial_device_resolution, d.device_resolution()), (Some((1440, 3120)), Some((1080, 2400))));

    let mut none = Display::default();
    none.start_with_record_resolution("1080x2400");
    assert_eq!((none.initial_device_resolution, none.device_resolution()), (None, None));
}

#[test]
fn navigation_buttons_only_for_gesture_navigation() {
    let mut d = Display::default();
    assert!(!d.navigation_buttons(), "unknown until the phone says");

    d.navigation_mode = 0;
    assert!(d.navigation_buttons(), "gestures: the window offers the buttons");

    for bar in 1..=3 {
        d.navigation_mode = bar;
        assert!(!d.navigation_buttons(), "the phone's own bar shows in the video");
    }
}
