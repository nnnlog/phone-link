//! The photos connect provider and its contribution to the connect message.

use crate::support::app_world;
use crate::support::photos_world as support;
use phonelink_engine::config::features::PhotosConfig;

use app_world::*;
use phonelink_engine::app::ConnectKind;
use phonelink_engine::app::consts::permission;
use phonelink_engine::codec::valueset::{self, Value, ValueSet};
use phonelink_engine::features::photos::PhotosEvent;
use support::*;

/// `{"yppCapabilities":32}` plus `extra` keys.
fn custom(extra: &str) -> String {
    format!(r#"{{"yppCapabilities":32{extra}}}"#)
}

/// Sends a connect answered with `response` and returns the request's `ValueSet`.
async fn connect(world: &World, response: ValueSet) -> ValueSet {
    world.h.messenger.answer_all(response_with(response));
    world.h.app.send_connect(ConnectKind::Background, "Test").await;
    let reqs = world.h.messenger.requests_on("/connect/v1");
    valueset::decode(&reqs.last().unwrap().message.payload).unwrap()
}

#[tokio::test(start_paused = true)]
async fn contribution_keys_with_the_photos_capability() {
    let world = world();
    let vs = connect(&world, ValueSet::new().with("result", 0i32)).await;

    let config = vs.object("configuration").unwrap();
    assert_eq!(config.get("MediaInfoSync"), Some(&Value::Bool(true)));
    assert_eq!(config.get("PhotosLegacySync"), Some(&Value::Bool(false)));
    assert_eq!(config.get("MediaInfoSnapshotSize"), Some(&Value::Int32(2000)));
    assert_eq!(config.get("AutoHydratedThumbnailsCount"), Some(&Value::Int32(25)));

    // In key order: after `UnlinkableVersion`, before `RecentApps`.
    let keys: Vec<&str> = config.keys().collect();
    let at = |k: &str| keys.iter().position(|x| *x == k).unwrap();
    assert!(at("UnlinkableVersion") < at("MediaInfoSync"));
    assert!(at("AutoHydratedThumbnailsCount") < at("RecentApps"));
    assert_eq!(vs.object("sequenceNumbers").unwrap().get("media_info"), Some(&Value::Int64(-1)));
}

#[tokio::test(start_paused = true)]
async fn without_the_capability_no_sequence_and_no_expected() {
    // registeredPermissionTypes = Messages only.
    let world = world_with(PhotosConfig::default(), Some(&custom(r#","registeredPermissionTypes":2"#)));
    let vs = connect(&world, ValueSet::new().with("result", 0i32).with("media_info", 9i64)).await;

    assert!(vs.object("configuration").unwrap().contains_key("MediaInfoSync"));
    assert!(!vs.object("sequenceNumbers").unwrap().contains_key("media_info"));

    let status = world.photos.status().unwrap();
    assert_eq!(status.expected_sequence, -1, "media_info is not read without the capability");
    assert!(!status.enabled);
}

#[tokio::test(start_paused = true)]
async fn the_configured_count_is_sent() {
    let world = world_with(PhotosConfig { auto_hydrated_thumbnails_count: 7, ..Default::default() }, None);
    let vs = connect(&world, ValueSet::new().with("result", 0i32)).await;
    assert_eq!(vs.object("configuration").unwrap().get("AutoHydratedThumbnailsCount"), Some(&Value::Int32(7)));
}

#[tokio::test(start_paused = true)]
async fn expected_is_read_only_with_result_0() {
    let world = world();
    connect(&world, ValueSet::new().with("result", 0i32).with("media_info", 42i64)).await;
    let status = world.photos.status().unwrap();
    assert_eq!(status.expected_sequence, 42);
    assert!(!status.up_to_date);
    assert!(status.syncing, "not up to date: the syncing indicator shows");

    connect(&world, ValueSet::new().with("result", 1i32).with("media_info", 50i64)).await;
    assert_eq!(world.photos.status().unwrap().expected_sequence, 42, "result 1: not read");
    connect(&world, ValueSet::new().with("result", 0i64).with("media_info", 50i64)).await;
    assert_eq!(world.photos.status().unwrap().expected_sequence, 42, "result of another type: not read");
    connect(&world, ValueSet::new().with("result", 0i32).with("media_info", 50i32)).await;
    assert_eq!(world.photos.status().unwrap().expected_sequence, -1, "media_info of another type reads -1");
    connect(&world, ValueSet::new().with("result", 0i32).with("media_info", 3i64)).await;
    connect(&world, ValueSet::new().with("result", 0i32)).await;
    assert_eq!(world.photos.status().unwrap().expected_sequence, -1, "absent reads -1");
}

#[tokio::test(start_paused = true)]
async fn the_syncing_indicator_ends_after_two_minutes_or_when_up_to_date() {
    let world = world();
    let mut events = world.events();
    connect(&world, ValueSet::new().with("result", 0i32).with("media_info", 5i64)).await;

    assert!(world.photos.status().unwrap().syncing);
    tokio::time::sleep(std::time::Duration::from_secs(119)).await;
    assert!(world.photos.status().unwrap().syncing);

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let status = world.photos.status().unwrap();
    assert!(!status.syncing && !status.up_to_date, "at most 2 minutes");
    assert!(drain(&mut events).iter().any(|e| matches!(e, PhotosEvent::StatusChanged { .. })));

    // A new expected value re-arms it; completing the sync clears it and records the sync time.
    connect(&world, ValueSet::new().with("result", 0i32).with("media_info", 6i64)).await;
    assert!(!world.photos.status().unwrap().syncing, "the up-to-date state did not change");

    connect(&world, ValueSet::new().with("result", 0i32).with("media_info", -1i64)).await;
    connect(&world, ValueSet::new().with("result", 0i32).with("media_info", 6i64)).await;
    assert!(world.photos.status().unwrap().syncing);

    world.inbound(media_info(Some("s"), metadata(&[], &[], None, Some(6)))).await;
    let status = world.photos.status().unwrap();
    assert!(!status.syncing && status.up_to_date);
    assert_eq!(status.last_sync_ms, Some(world.now_ms()));
}

#[tokio::test(start_paused = true)]
async fn permissions_and_capabilities_are_applied_whatever_the_result() {
    let world = world();
    let perms = ValueSet::new().with("photos", false).with("photos_full", true).with("photos_delete", true);
    let caps = ValueSet::new().with("photos", 3i32);
    connect(&world, ValueSet::new().with("result", 1i32).with("permissions", perms).with("capabilities", caps)).await;

    let record = world.h.app.phone_record().unwrap();
    assert!(!record.permission_or(permission::PHOTOS, true));
    assert!(record.permission_or(permission::PHOTOS_FULL, false));
    assert!(record.permission_or(permission::PHOTOS_DELETE, false));
    assert_eq!(world.photos.status().unwrap().capabilities, 3);

    // Absent or mistyped keys are skipped; the capabilities are kept.
    let perms = ValueSet::new().with("photos", 1i32);
    let caps = ValueSet::new().with("photos", 1i64);
    connect(&world, ValueSet::new().with("permissions", perms).with("capabilities", caps)).await;
    assert!(!world.h.app.phone_record().unwrap().permission_or(permission::PHOTOS, true));
    assert_eq!(world.photos.status().unwrap().capabilities, 3);

    connect(&world, ValueSet::new().with("capabilities", ValueSet::new()).with("permissions", 5i32)).await;
    assert_eq!(world.photos.status().unwrap().capabilities, 3);

    // `mediaSettings.dcim_directory` is ignored.
    let ms = ValueSet::new().with("dcim_directory", "/storage/emulated/0/DCIM");
    connect(&world, ValueSet::new().with("result", 0i32).with("mediaSettings", ms)).await;
}

#[tokio::test(start_paused = true)]
async fn every_connect_reply_rearms_the_full_access_reminder() {
    // Android SDK 34, every permission at its default (Photos held, PhotosFull and PhotosDelete not,
    // post-notification held).
    let world = world_with(PhotosConfig::default(), Some(&custom(r#","androidSdkVersion":34"#)));
    assert!(world.photos.status().unwrap().full_access_reminder);
    world.photos.dismiss_full_access_reminder().unwrap();
    assert!(!world.photos.status().unwrap().full_access_reminder);

    // A failed connect (no response) does not re-arm it.
    world.h.messenger.answer_all(status_response(phonelink_engine::seams::PlatformRequestStatus::NoResponse));
    world.h.app.send_connect(ConnectKind::Background, "Test").await;
    assert!(!world.photos.status().unwrap().full_access_reminder);

    connect(&world, ValueSet::new().with("result", 1i32)).await;
    assert!(world.photos.status().unwrap().full_access_reminder);
}

#[tokio::test(start_paused = true)]
async fn the_connect_carries_the_stored_current_sequence() {
    let world = world();
    world.inbound(media_info(Some("s"), metadata(&[], &[], None, Some(77)))).await;

    let vs = connect(&world, ValueSet::new().with("result", 0i32)).await;
    assert_eq!(vs.object("sequenceNumbers").unwrap().get("media_info"), Some(&Value::Int64(77)));
}

#[tokio::test(start_paused = true)]
async fn an_unreadable_store_leaves_media_info_out_rather_than_inventing_one() {
    let dir = tempfile::tempdir().unwrap();
    let location = phonelink_engine::features::StoreLocation::Directory(dir.path().to_path_buf());
    let world = world_at(PhotosConfig::default(), location, None);
    world.inbound(media_info(Some("s"), metadata(&[], &[], None, Some(77)))).await;

    // The store breaks under the open connection: its state row goes away.
    let file = dir.path().join("phones").join(phone().to_string()).join("photos.sqlite3");
    rusqlite::Connection::open(&file).unwrap().execute_batch("DROP TABLE photos_state;").unwrap();
    assert!(world.photos.status().is_err(), "the store is unreadable");

    let vs = connect(&world, ValueSet::new().with("result", 0i32)).await;
    let config = vs.object("configuration").unwrap();
    assert_eq!(config.get("MediaInfoSync"), Some(&Value::Bool(true)), "the provider's other keys are sent");
    assert!(
        !vs.object("sequenceNumbers").unwrap().contains_key("media_info"),
        "no media_info can be stated when the stored current sequence cannot be read"
    );
}

#[tokio::test(start_paused = true)]
async fn the_syncing_indicator_shows_at_start_when_the_stored_pair_is_behind() {
    let dir = tempfile::tempdir().unwrap();
    let location = phonelink_engine::features::StoreLocation::Directory(dir.path().to_path_buf());

    {
        let world = world_at(PhotosConfig::default(), location.clone(), None);
        connect(&world, ValueSet::new().with("result", 0i32).with("media_info", 8i64)).await;
    }

    // The first read at start: not up to date, so the indicator shows for at most 2 minutes.
    let world = world_at(PhotosConfig::default(), location, None);
    let status = world.photos.status().unwrap();
    assert!(status.syncing && !status.up_to_date);
    tokio::time::sleep(std::time::Duration::from_secs(121)).await;
    assert!(!world.photos.status().unwrap().syncing);
}

#[tokio::test(start_paused = true)]
async fn nothing_requests_a_sync_when_the_pc_is_behind() {
    let world = world();
    connect(&world, ValueSet::new().with("result", 0i32).with("media_info", 50i64)).await;
    assert!(!world.photos.status().unwrap().up_to_date);

    tokio::time::sleep(std::time::Duration::from_secs(300)).await;
    assert!(world.media_requests().is_empty());
}
