//! Wallpaper — Phone Link's contract over the App layer: connect provider #4 (its request keys, its
//! gates as they stood before the reply, the reply's sequence number and permissions), the
//! `wallpaper` content processor (metadata and content steps, missing and mistyped keys), and which
//! wallpaper is shown.

use crate::support::platform_world;
use crate::support::wallpaper_world as world;

use phonelink_engine::app::consts::permission;
use phonelink_engine::codec::valueset::{Value, ValueSet};
use phonelink_engine::features::wallpaper::{PhoneLinkState, StoredWallpaper, WallpaperCopy, WallpaperEvent};
use phonelink_engine::proto::device_info::v1::WallpaperType;
use platform_world::{phone_id, settle};
use world::*;

fn granted() -> ValueSet {
    ValueSet::new().with("result", 0i32).with("wallpaperDevicePermission", true)
}

// ------------------------------------------------------------------------------------------------
// The connect contribution
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn the_connect_turns_wallpaper_on_and_resumes_from_the_stored_sequence_number() {
    let ww = WallpaperWorld::android();
    let wp = ww.install();
    ww.sw.w.connect_phone().await;

    let request = ww.connect(granted()).await;
    let configuration = request.object("configuration").unwrap();
    assert_eq!(configuration.get("Wallpaper"), Some(&Value::Bool(true)), "contract version 3.99 ≥ 3.84");
    assert_eq!(request.object("sequenceNumbers").unwrap().get("wallpaper"), Some(&Value::Int64(-1)));

    assert_eq!(ww.phone_sends(item(5, 0, b"\xFF\xD8home", 12)).await, ValueSet::new().with("result", 0i32));
    let request = ww.connect(granted()).await;
    assert_eq!(request.object("sequenceNumbers").unwrap().i64("wallpaper"), Some(12));
    assert_eq!(state(&wp).current_sequence, 12);
}

#[tokio::test(start_paused = true)]
async fn the_reply_sets_the_expected_sequence_number_and_the_permissions() {
    let ww = WallpaperWorld::android();
    let wp = ww.install();
    ww.sw.w.connect_phone().await;
    let mut rx = wp.subscribe();

    let reply = ValueSet::new()
        .with("result", 0i32)
        .with("wallpaper", 7i64)
        .with("wallpaperDevicePermission", true)
        .with("permissions", ValueSet::new().with("read_media_images", true).with("files", false));
    ww.connect(reply).await;

    assert_eq!(state(&wp), PhoneLinkState { current_sequence: -1, expected_sequence: 7, device_permission: true });
    let record = ww.sw.app.phone_record().unwrap();
    assert!(record.permission_or(permission::READ_MEDIA_IMAGES, false));
    assert!(!record.permission_or(permission::FILES, true));
    assert!(events(&mut rx).contains(&WallpaperEvent::Changed { phone: phone_id() }));

    // A result other than 0 leaves the expected number; the permissions are applied whatever it is;
    // a mistyped key is skipped.
    let reply = ValueSet::new()
        .with("result", 1i32)
        .with("wallpaper", 9i64)
        .with("wallpaperDevicePermission", false)
        .with("permissions", ValueSet::new().with("files", true).with("read_media_images", 1i32));
    ww.connect(reply).await;

    assert_eq!(state(&wp), PhoneLinkState { current_sequence: -1, expected_sequence: 7, device_permission: false });
    let record = ww.sw.app.phone_record().unwrap();
    assert!(record.permission_or(permission::FILES, false));
    assert!(record.permission_or(permission::READ_MEDIA_IMAGES, false), "the Int32 value was skipped");

    // `wallpaper` absent or not an Int64 with result 0: -1.
    ww.connect(ValueSet::new().with("result", 0i32).with("wallpaper", 3i32)).await;
    assert_eq!(state(&wp).expected_sequence, -1);
    assert!(!state(&wp).device_permission, "absent: unchanged");
}

#[tokio::test(start_paused = true)]
async fn the_reply_is_gated_on_the_record_as_it_stood_before_the_reply() {
    let ww = WallpaperWorld::android();
    let wp = ww.install();
    ww.sw.w.connect_phone().await;

    // This reply makes the phone unsupported, but the gate of this connect was taken before it.
    let reply = ValueSet::new().with("result", 0i32).with("wallpaper", 9i64).with("contractVersion", 3.5f64);
    ww.connect(reply).await;
    assert_eq!(state(&wp).expected_sequence, 9);

    // The next connect sees the stored 3.5: off, no sequence number, and its reply's number is not read.
    let request = ww.connect(ValueSet::new().with("result", 0i32).with("wallpaper", 11i64)).await;
    assert_eq!(request.object("configuration").unwrap().get("Wallpaper"), Some(&Value::Bool(false)));
    assert_eq!(request.object("sequenceNumbers").unwrap().get("wallpaper"), None);
    assert_eq!(state(&wp).expected_sequence, 9);
}

#[tokio::test(start_paused = true)]
async fn without_the_capability_bit_only_the_flag_is_sent() {
    let ww = WallpaperWorld::android();
    let wp = ww.install();
    ww.sw.w.connect_phone().await;

    // The reply records capability bits without Wallpaper (0x200).
    let reply = ValueSet::new()
        .with("result", 0i32)
        .with("capabilities", ValueSet::new().with("registeredPermissionTypes", 0x4D5Fi32));
    ww.connect(reply).await;

    let request = ww.connect(ValueSet::new().with("result", 0i32).with("wallpaper", 4i64)).await;
    assert_eq!(request.object("configuration").unwrap().get("Wallpaper"), Some(&Value::Bool(true)));
    assert_eq!(request.object("sequenceNumbers").unwrap().get("wallpaper"), None);
    assert_eq!(state(&wp).expected_sequence, -1);
}

// ------------------------------------------------------------------------------------------------
// The content processor
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn an_item_is_stored_by_type_and_answered_0() {
    let ww = WallpaperWorld::android();
    let wp = ww.install();
    ww.sw.w.connect_phone().await;
    let mut rx = wp.subscribe();

    assert_eq!(ww.phone_sends(item(5, 0, b"home", 12)).await, ValueSet::new().with("result", 0i32));
    assert_eq!(stored(&wp, WallpaperCopy::PhoneLink, 0), Some(StoredWallpaper { id: 5, bytes: b"home".to_vec() }));
    assert_eq!(events(&mut rx), vec![WallpaperEvent::Changed { phone: phone_id() }]);

    // The lock screen is stored, raises nothing, and is never shown.
    assert_eq!(ww.phone_sends(item(6, 1, b"lock", 13)).await, ValueSet::new().with("result", 0i32));
    assert_eq!(stored(&wp, WallpaperCopy::PhoneLink, 1), Some(StoredWallpaper { id: 6, bytes: b"lock".to_vec() }));
    assert!(events(&mut rx).is_empty());

    // Any other type is stored under its own type.
    ww.phone_sends(item(8, 4, b"x", 14)).await;
    assert_eq!(stored(&wp, WallpaperCopy::PhoneLink, 4).map(|w| w.id), Some(8));
    assert_eq!(state(&wp).current_sequence, 14);

    // A newer home screen replaces the row.
    ww.phone_sends(item(9, 0, b"home2", 15)).await;
    assert_eq!(stored(&wp, WallpaperCopy::PhoneLink, 0), Some(StoredWallpaper { id: 9, bytes: b"home2".to_vec() }));
}

#[tokio::test(start_paused = true)]
async fn the_metadata_step_answers_nothing_and_keeps_the_transaction() {
    let ww = WallpaperWorld::android();
    let wp = ww.install();
    ww.sw.w.connect_phone().await;

    let answer = ww.phone_sends(ValueSet::new().with("metadata", true).with("syncID", "s-1")).await;
    assert_eq!(answer.get("result"), None, "an empty answer: {answer:?}");

    // The item of the same transaction goes to the stored processor.
    let answer = ww.phone_sends(item(1, 0, b"h", 2).with("syncID", "s-1")).await;
    assert_eq!(answer, ValueSet::new().with("result", 0i32));
    assert!(stored(&wp, WallpaperCopy::PhoneLink, 0).is_some());
}

#[tokio::test(start_paused = true)]
async fn missing_keys_store_nothing_and_mistyped_ones_are_refused() {
    let ww = WallpaperWorld::android();
    let wp = ww.install();
    ww.sw.w.connect_phone().await;

    let mut partial = item(1, 0, b"h", 2);
    partial.remove("sequenceNumber");
    assert_eq!(ww.phone_sends(partial).await, ValueSet::new().with("result", 0i32));
    assert_eq!(stored(&wp, WallpaperCopy::PhoneLink, 0), None);

    // Absent keys are not read, so their companions' types do not matter.
    let partial = ValueSet::new().with("wallpaperId", "x").with("wallpaper", "y");
    assert_eq!(ww.phone_sends(partial).await, ValueSet::new().with("result", 0i32));

    for (k, v) in [
        ("wallpaperId", Value::Int32(1)),
        ("wallpaperType", Value::String("home".into())),
        ("wallpaper", Value::String("base64?".into())),
        ("sequenceNumber", Value::UInt32(2)),
    ] {
        let mut bad = item(1, 0, b"h", 2);
        bad.insert(k, v);
        assert_eq!(ww.phone_sends(bad).await, ValueSet::new().with("result", 1i32), "{k}");
    }
    assert_eq!(stored(&wp, WallpaperCopy::PhoneLink, 0), None);
    assert_eq!(state(&wp).current_sequence, -1);

    // Without a correlationVector (neither the payload's nor a header's).
    let raw = item(1, 0, b"h", 2).with("contentType", "wallpaper").with("contractVersion", 3.99f64);
    assert_eq!(ww.phone_sends_raw(raw).await, ValueSet::new().with("result", 1i32));
    assert_eq!(stored(&wp, WallpaperCopy::PhoneLink, 0), None);
}

// ------------------------------------------------------------------------------------------------
// What is shown
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn phone_links_home_screen_shows_with_the_permission_and_a_supported_phone() {
    let ww = WallpaperWorld::android();
    let wp = ww.install();
    ww.sw.w.connect_phone().await;

    ww.phone_sends(item(5, 0, b"home", 12)).await;
    assert_eq!(shown(&wp), None, "the permission defaults to false");

    ww.connect(granted()).await;
    let s = shown(&wp).expect("shown");
    assert_eq!((s.image.as_slice(), s.copy), (b"home".as_slice(), WallpaperCopy::PhoneLink));

    // An empty image is none.
    ww.phone_sends(item(6, 0, b"", 13)).await;
    assert_eq!(shown(&wp), None);
    ww.phone_sends(item(7, 0, b"home3", 14)).await;
    assert!(shown(&wp).is_some());

    // A phone that reports a contract version below 3.84 is not supported.
    ww.connect(granted().with("contractVersion", 3.5f64)).await;
    assert_eq!(shown(&wp), None);
}

#[tokio::test(start_paused = true)]
async fn the_cross_device_copy_shows_when_phone_link_gives_none() {
    let ww = WallpaperWorld::android();
    ww.phone_answers_pulls(&ok_reply(vec![entry(WallpaperType::Homescreen, 3, Some(b"pulled"))]));
    let wp = ww.install();
    ww.sw.w.connect_phone().await;
    settle().await;

    let s = shown(&wp).expect("the pulled image");
    assert_eq!((s.image.as_slice(), s.copy), (b"pulled".as_slice(), WallpaperCopy::CrossDevice));

    ww.phone_sends(item(5, 0, b"home", 12)).await;
    assert_eq!(shown(&wp).unwrap().copy, WallpaperCopy::CrossDevice, "no permission yet");
    ww.connect(granted()).await;
    assert_eq!(shown(&wp).unwrap().copy, WallpaperCopy::PhoneLink);
}
