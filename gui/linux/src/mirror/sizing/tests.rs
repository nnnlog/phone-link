use serde_json::{Value, json};

use super::answer;
use super::geometry::{Placement, Rect, Screen, Size};
use super::placement::{cascaded, restored, screen_index};
use super::request::DEFAULT_ASPECT;
use super::sizes::{
    content_aspect, default_size, height_changed, holds_navigation_bar, minimum_size, reshaped, same_aspect,
};

const VISIBLE: Size = Size::new(1600.0, 1000.0);
/// A toolbar of 52 pixels.
const NON_CLIENT: Size = Size::new(0.0, 52.0);

/// A 1600 × 1040 screen with a 40-pixel panel at its top.
fn screen() -> Screen {
    Screen { frame: Rect::new(0.0, 0.0, 1600.0, 1040.0), visible: Rect::new(0.0, 40.0, 1600.0, 1000.0) }
}

/// The whole window, toolbar included, fits 75 % of the visible room each way.
#[test]
fn a_new_window_fits_three_quarters_of_the_screen() {
    assert_eq!(default_size(0.45, VISIBLE, NON_CLIENT), Size::new(314.0, 750.0), "750 high in all, 698 of video");
    assert_eq!(default_size(16.0 / 9.0, VISIBLE, NON_CLIENT), Size::new(1200.0, 727.0), "1200 wide, 675 of video");
}

#[test]
fn a_window_steps_away_from_another_at_its_place() {
    let frame = Rect::new(643.0, 165.0, 314.0, 750.0);
    assert_eq!(cascaded(frame, &[screen()], &[]), frame);

    // A peer at the same top-left corner: one step down and right.
    assert_eq!(cascaded(frame, &[screen()], &[frame]), frame.offset(32.0));
    // Within 4 pixels counts; 5 pixels does not.
    let near = Rect::new(646.0, 162.0, 300.0, 744.0);
    assert_eq!(cascaded(frame, &[screen()], &[near]), frame.offset(32.0));
    let apart = Rect::new(648.0, 165.0, 314.0, 750.0);
    assert_eq!(cascaded(frame, &[screen()], &[apart]), frame);

    // Two peers in a row: two steps.
    assert_eq!(cascaded(frame, &[screen()], &[frame, frame.offset(32.0)]), frame.offset(64.0));

    // A step that leaves the visible room ends the cascade where it started.
    let low = Rect::new(643.0, 280.0, 314.0, 750.0);
    assert_eq!(cascaded(low, &[screen()], &[low]), low);

    // At most 8 places are tried: with all 8 taken the window stays where it started.
    let small = Rect::new(100.0, 240.0, 200.0, 300.0);
    let crowded: Vec<Rect> = (0..8_u32).map(|step| small.offset(f64::from(step) * 32.0)).collect();
    assert_eq!(cascaded(small, &[screen()], &crowded[..7]), small.offset(224.0));
    assert_eq!(cascaded(small, &[screen()], &crowded), small);
}

/// Only peers on the window's screen count.
#[test]
fn peers_on_another_screen_do_not_count() {
    let second =
        Screen { frame: Rect::new(1600.0, 0.0, 1000.0, 800.0), visible: Rect::new(1600.0, 25.0, 1000.0, 775.0) };
    let frame = Rect::new(1300.0, 240.0, 300.0, 600.0);
    // Mostly on the second screen, its top-left corner at the window's.
    let peer = Rect::new(1302.0, 240.0, 1000.0, 600.0);
    assert_eq!(cascaded(frame, &[screen(), second], &[peer]), frame);

    let frames = [screen().frame, second.frame];
    assert_eq!(screen_index(&peer, &frames), Some(1));
    assert_eq!(screen_index(&Rect::new(2700.0, 0.0, 10.0, 10.0), &frames), Some(1), "the nearest");
}

/// A kept frame is used as it was when it overlaps a screen, out of the visible room or not.
#[test]
fn a_kept_frame_returns_as_it_was() {
    let screens = [screen().frame];
    let inside = Rect::new(10.0, 10.0, 300.0, 652.0);
    assert_eq!(restored(inside, &screens), Some(inside));
    let across = Rect::new(1500.0, -20.0, 300.0, 1200.0);
    assert_eq!(restored(across, &screens), Some(across), "not moved in");
    assert_eq!(restored(Rect::new(1700.0, 0.0, 300.0, 652.0), &screens), None, "on no screen");
    assert_eq!(restored(Rect::new(1600.0, 0.0, 300.0, 652.0), &screens), None, "touching is not overlapping");
    assert_eq!(restored(Rect::new(0.0, 0.0, 0.0, 0.0), &screens), None);
}

#[test]
fn a_video_of_the_same_aspect_changes_nothing() {
    let portrait = Size::new(360.0, 852.0);
    let size = reshaped(Size::new(362.0, 800.0), portrait, 0.45, None, VISIBLE, NON_CLIENT);
    assert_eq!(size, None, "0.4525 is within 0.01 of 0.45");
}

/// A video that fits between the smallest window and 75 % of the largest one takes its own size.
#[test]
fn a_new_aspect_takes_the_videos_own_size() {
    let portrait = Size::new(360.0, 852.0);
    let size = reshaped(Size::new(400.0, 300.0), portrait, 0.45, None, VISIBLE, NON_CLIENT);
    assert_eq!(size, Some(Size::new(400.0, 352.0)));
}

/// A video larger than 75 % of the largest window of its aspect shrinks to it; one smaller than the
/// smallest window grows to it.
#[test]
fn a_new_aspect_stays_between_the_limits() {
    let portrait = Size::new(360.0, 852.0);
    // The largest 2.22 window is 1600 × 772: 75 % is 1200 × 579, 527 of video high.
    let large = reshaped(Size::new(2400.0, 1080.0), portrait, 0.45, None, VISIBLE, NON_CLIENT);
    assert_eq!(large, Some(Size::new(1171.0, 579.0)));

    let tiny = reshaped(Size::new(100.0, 200.0), Size::new(400.0, 352.0), 400.0 / 300.0, None, VISIBLE, NON_CLIENT);
    assert_eq!(tiny, Some(Size::new(240.0, 532.0)), "240 wide for a portrait video");
}

/// The size before the last reshape returns for its aspect when it fits the visible room.
#[test]
fn the_previous_size_returns_for_its_aspect() {
    let portrait = Size::new(360.0, 852.0);
    let landscape = Size::new(1171.0, 579.0);
    let previous = Placement { aspect: 0.45, size: portrait };
    let back = reshaped(Size::new(720.0, 1600.0), landscape, 2.2, Some(previous), VISIBLE, NON_CLIENT);
    assert_eq!(back, Some(portrait));

    let too_tall = Placement { aspect: 0.45, size: Size::new(480.0, 1100.0) };
    let other = reshaped(Size::new(720.0, 1600.0), landscape, 2.2, Some(too_tall), VISIBLE, NON_CLIENT);
    assert_ne!(other, Some(too_tall.size), "it no longer fits the visible room");
}

/// Without a height of its own the window's aspect is the one it was given.
#[test]
fn a_window_without_a_video_area_takes_the_fallback_aspect() {
    let flat = Size::new(400.0, 52.0);
    assert_eq!(reshaped(Size::new(450.0, 1000.0), flat, 0.45, None, VISIBLE, NON_CLIENT), None);
    assert_eq!(reshaped(Size::new(0.0, 1000.0), flat, 0.45, None, VISIBLE, NON_CLIENT), None);
}

#[test]
fn the_smallest_window_follows_the_aspect() {
    assert_eq!(minimum_size(0.45, VISIBLE, NON_CLIENT), Size::new(240.0, 585.0));
    assert_eq!(minimum_size(1.0, VISIBLE, NON_CLIENT), Size::new(240.0, 292.0), "1 is portrait");
    assert_eq!(minimum_size(2.0, VISIBLE, NON_CLIENT), Size::new(296.0, 200.0));
    assert_eq!(minimum_size(1.2, VISIBLE, NON_CLIENT), Size::new(240.0, 200.0), "at least 240 wide");
    let short = Size::new(1600.0, 400.0);
    assert_eq!(minimum_size(0.3, short, NON_CLIENT).height, 400.0, "at most the visible height");
}

#[test]
fn the_navigation_bar_changes_the_height() {
    let size = Size::new(360.0, 852.0);
    let barred = Size::new(0.0, 84.0);
    let grown = height_changed(size, 32.0, 0.45, VISIBLE, barred);
    assert_eq!(grown, Size::new(360.0, 884.0));
    assert_eq!(height_changed(size, 500.0, 0.45, VISIBLE, barred).height, 1000.0);

    // A window whose video area without the bar has the aspect already holds it.
    assert!(holds_navigation_bar(grown, NON_CLIENT, 0.45));
    assert!(!holds_navigation_bar(size, NON_CLIENT, 0.45));
}

#[test]
fn a_window_asks_with_json() {
    let visible = json!({ "width": 1600, "height": 1000 });
    let non_client = json!({ "width": 0, "height": 52 });
    let opening = json!({ "op": "opening", "aspect": 0.45, "visible": visible, "nonClient": non_client });
    assert_eq!(answer(&opening), Ok(json!({ "width": 314.0, "height": 750.0, "aspect": 0.45 })));

    let unknown = json!({ "op": "opening", "aspect": null, "visible": visible, "nonClient": non_client });
    assert_eq!(answer(&unknown).map(|v| v["aspect"].clone()), Ok(json!(DEFAULT_ASPECT)), "a phone's usual aspect");

    let reshape = json!({
        "op": "reshape", "content": { "width": 720, "height": 1600 }, "shapedFor": 2.2,
        "current": { "width": 1171, "height": 579 },
        "fallbackAspect": 2.2, "previous": { "aspect": 0.45, "width": 360, "height": 852 },
        "visible": visible, "nonClient": non_client,
    });
    assert_eq!(answer(&reshape), Ok(json!({ "width": 360.0, "height": 852.0 })));
    let same = json!({
        "op": "reshape", "content": { "width": 362, "height": 800 }, "shapedFor": 0.45,
        "current": { "width": 300, "height": 852 }, "fallbackAspect": 0.45, "previous": null,
        "visible": visible, "nonClient": non_client,
    });
    assert_eq!(answer(&same), Ok(Value::Null), "the user's own shape stays while the video keeps its aspect");
    let turned = json!({
        "op": "reshape", "content": { "width": 800, "height": 362 }, "shapedFor": 0.45,
        "current": { "width": 300, "height": 852 }, "fallbackAspect": 0.45, "previous": null,
        "visible": visible, "nonClient": non_client,
    });
    assert_eq!(answer(&turned), Ok(json!({ "width": 800.0, "height": 414.0 })));

    let restore = json!({
        "op": "restore", "saved": { "x": 1700, "y": 0, "width": 300, "height": 652 },
        "screens": [{ "x": 0, "y": 0, "width": 1600, "height": 1040 }],
    });
    assert_eq!(answer(&restore), Ok(Value::Null));

    let frame = json!({ "x": 643, "y": 165, "width": 314, "height": 750 });
    let cascade = json!({
        "op": "cascade", "frame": frame, "peers": [frame],
        "screens": [{ "frame": { "x": 0, "y": 0, "width": 1600, "height": 1040 },
                      "visible": { "x": 0, "y": 40, "width": 1600, "height": 1000 } }],
    });
    assert_eq!(answer(&cascade), Ok(json!({ "x": 675.0, "y": 197.0, "width": 314.0, "height": 750.0 })));
}

/// A request missing what its op needs is refused, not answered with a guess.
#[test]
fn an_incomplete_request_is_refused() {
    assert!(answer(&json!({ "op": "opening", "aspect": 0.45 })).is_err());
    assert!(answer(&json!({ "op": "spin" })).is_err());
    assert!(answer(&json!({ "aspect": 0.45 })).is_err());
    assert!(answer(&json!({ "op": "restore", "saved": { "x": 0, "y": 0, "width": 1, "height": 1 } })).is_err());
}

#[test]
fn aspects_within_a_hundredth_are_the_same() {
    assert!(same_aspect(0.45, 0.4525));
    assert!(!same_aspect(0.45, 0.47));
    assert_eq!(content_aspect(Size::new(360.0, 852.0), NON_CLIENT), Some(0.45));
    assert_eq!(content_aspect(Size::new(360.0, 52.0), NON_CLIENT), None);
}
