//! The user-facing photos flows: permission requests, the media sync request, delete, the feature
//! gates, cache maintenance, and the new-photo notification.

use crate::assert_matches;
use crate::support::app_world;
use crate::support::photos_world as support;
use phonelink_engine::config::features::PhotosConfig;

use std::collections::HashMap;
use std::time::Duration;

use app_world::*;
use phonelink_engine::app::consts::permission;
use phonelink_engine::app::{AppOutcome, ConnectKind};
use phonelink_engine::codec::valueset::{self, ValueSet};
use phonelink_engine::edge::Clock;
use phonelink_engine::features::StoreLocation;
use phonelink_engine::features::photos::{
    DeletePrompt, DeleteState, PhotosError, PhotosEvent, USER_INFO_MEDIA_ID, USER_INFO_PHONE,
};
use phonelink_engine::features::shared::notifier::NotificationResponse;
use support::*;

fn custom(extra: &str) -> String {
    format!(r#"{{"yppCapabilities":32{extra}}}"#)
}

fn perm(world: &World, bit: u32, default: bool) -> bool {
    world.h.app.phone_record().unwrap().permission_or(bit, default)
}

/// A connect answered with `response`.
async fn connect(world: &World, response: ValueSet) {
    world.h.messenger.answer_all(response_with(response));
    world.h.app.send_connect(ConnectKind::Background, "Test").await;
}

fn permission_payload(world: &World, i: usize) -> ValueSet {
    world.permission_requests()[i].clone()
}

// ------------------------------------------------------------------------------------------------
// Permission requests and the media sync request
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn photo_access_request_and_its_sync_follow_up() {
    let world = world();
    let phone = Phone::install(&world);
    phone.queue_permission(4); // Accepted
    assert_eq!(world.photos.request_photo_access().await.unwrap(), AppOutcome::Accepted);

    let req = &world.h.messenger.requests_on(PERMISSION)[0];
    assert_eq!(req.priority, 0, "user-initiated");
    let p = permission_payload(&world, 0);
    assert_eq!(p.str("contentType"), Some("permission"));
    assert_eq!(p.i32("permissionType"), Some(1));
    assert_eq!(p.bool("silent"), Some(false));
    assert!(p.str("permissionSessionId").is_some());
    assert!(perm(&world, permission::PHOTOS, false) && perm(&world, permission::PHOTOS_FULL, false));

    // The media sync request follows (Photos is held), with default options.
    wait_held(&phone, 1).await;
    let sync = phone.take();
    assert_eq!(sync.content_type(), Some("mediaInfo"));
    let cv = sync.payload.str("correlationVector").unwrap().to_string();
    assert_ne!(Some(cv.as_str()), p.str("correlationVector"), "a fresh correlationVector, not the permission's");
    assert_eq!(sync.payload.string_array("relatedCorrelationIds"), Some(&[cv][..]));
    assert_eq!(sync.payload.string_array("subcontentTypes"), Some(&["mediaInfo".to_string()][..]));
    assert_eq!(sync.payload.bool("isuserinitiated"), Some(false));
    assert!(world.photos.status().unwrap().syncing, "the indicator shows while the sync request is in flight");

    // While it is in flight, another follow-up is not sent.
    phone.queue_permission(0);
    world.photos.request_photo_access().await.unwrap();
    settle().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(phone.held_count(), 0);

    // Its reply only sets the Photos permission.
    sync.answer(result_response(6));
    settle().await;
    assert!(!perm(&world, permission::PHOTOS, true));
    assert!(!world.photos.status().unwrap().syncing);
}

#[tokio::test(start_paused = true)]
async fn access_reply_mappings() {
    // (reply, Photos, PhotosFull, sync follows)
    for (code, photos, full, sync) in [
        (0, true, true, true),
        (4, true, true, true),
        (7, true, false, true),
        (5, false, false, false),
        (6, false, false, false),
    ] {
        let world = world();
        let phone = Phone::install(&world);
        phone.queue_permission(code);
        world.photos.request_photo_access().await.unwrap();

        assert_eq!(perm(&world, permission::PHOTOS, !photos), photos, "code {code}");
        assert_eq!(perm(&world, permission::PHOTOS_FULL, !full), full, "code {code}");
        settle().await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(phone.held_count(), usize::from(sync), "code {code}");
    }
}

#[tokio::test(start_paused = true)]
async fn full_access_request_and_an_unchanged_reply() {
    let world = world();
    let phone = Phone::install(&world);
    phone.queue_permission(1); // Failure leaves the bits unchanged; Photos was held: the sync follows.

    assert_eq!(world.photos.request_full_access().await.unwrap(), AppOutcome::Failure);
    let p = permission_payload(&world, 0);
    assert_eq!(p.i32("permissionType"), Some(0x2000_0000));
    assert_eq!(p.bool("silent"), Some(false));

    wait_held(&phone, 1).await;
    phone.take().answer(result_response(0));
    settle().await;
    assert!(perm(&world, permission::PHOTOS, false));
}

#[tokio::test(start_paused = true)]
async fn the_sync_request_is_not_sent_while_syncing() {
    let world = world();
    // The connect reply makes the PC not up to date: the indicator shows for 2 minutes.
    connect(&world, ValueSet::new().with("result", 0i32).with("media_info", 5i64)).await;
    let phone = Phone::install(&world);

    world.photos.request_photo_access().await.unwrap();
    settle().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(phone.held_count(), 0);

    tokio::time::sleep(Duration::from_secs(120)).await;
    world.photos.request_photo_access().await.unwrap();
    wait_held(&phone, 1).await;
}

#[tokio::test(start_paused = true)]
async fn the_full_access_reminder_rules() {
    let world = world_with(PhotosConfig::default(), Some(&custom(r#","androidSdkVersion":34"#)));
    assert!(world.photos.status().unwrap().full_access_reminder);

    // Any of: SDK < 34, PhotosDelete held, Photos not held, PhotosFull held, post-notification not held.
    for (bit, value) in [
        (permission::PHOTOS_DELETE, true),
        (permission::PHOTOS, false),
        (permission::PHOTOS_FULL, true),
        (permission::POST_NOTIFICATIONS, false),
    ] {
        world.h.app.update_phone_record(|r| r.set_permission(bit, value));
        assert!(!world.photos.status().unwrap().full_access_reminder, "bit {bit:#x}");
        world.h.app.update_phone_record(|r| r.set_permission(bit, !value));
        assert!(world.photos.status().unwrap().full_access_reminder);
    }

    let old = world_with(PhotosConfig::default(), Some(&custom(r#","androidSdkVersion":33"#)));
    assert!(!old.photos.status().unwrap().full_access_reminder);

    // Dismissed: until the next connect reply.
    world.photos.dismiss_full_access_reminder().unwrap();
    assert!(!world.photos.status().unwrap().full_access_reminder);
}

#[tokio::test(start_paused = true)]
async fn record_changes_are_announced() {
    let world = world();
    let mut events = world.events();
    world.h.app.update_phone_record(|r| r.set_permission(permission::PHOTOS, false));
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(drain(&mut events).iter().any(|e| matches!(e, PhotosEvent::StatusChanged { .. })));
    assert!(!world.photos.status().unwrap().photos_permission);
}

// ------------------------------------------------------------------------------------------------
// Delete
// ------------------------------------------------------------------------------------------------

/// A world whose phone advertises `capabilities` through a metadata message, with items 1..=3.
async fn delete_world(capabilities: i32, custom_data: Option<&str>) -> World {
    delete_world_at(capabilities, custom_data, StoreLocation::Memory).await
}

async fn delete_world_at(capabilities: i32, custom_data: Option<&str>, stores: StoreLocation) -> World {
    let world = world_at(PhotosConfig::default(), stores, custom_data);
    seed(&world, &[it(1, 1).checksum(11), it(2, 2).checksum(0), it(3, 3)]).await;
    let settings = ValueSet::new().with("mediaCapabilities", capabilities);
    world
        .inbound(media_info(Some("m"), metadata(&[1, 2, 3], &[1, 2, 3], None, None).with("mediaSettings", settings)))
        .await;
    world
}

fn delete_reply(ok: &[i64], failed: &[i64]) -> phonelink_engine::seams::PlatformResponse {
    response_with(
        ValueSet::new()
            .with("result", 0i32)
            .with("mediaSuccessfullyDeletedIds", ok.to_vec())
            .with("mediaUnsuccessfullyDeletedIds", failed.to_vec()),
    )
}

#[tokio::test(start_paused = true)]
async fn delete_is_offered_with_media_delete_and_the_pc_setting() {
    let world = delete_world(0, None).await;
    let status = world.photos.status().unwrap();
    assert!(!status.delete_offered);
    assert_matches!(world.photos.prepare_delete(1).await, Err(PhotosError::DeleteNotOffered));
    assert_matches!(world.photos.delete(1).await.unwrap_err(), PhotosError::DeleteNotOffered);

    let world = delete_world(1, None).await;
    assert!(world.photos.status().unwrap().delete_offered);

    world.photos.set_deletion_allowed(false).unwrap();
    assert!(!world.photos.status().unwrap().delete_offered);

    world.photos.set_deletion_allowed(true).unwrap();
    assert_eq!(world.photos.prepare_delete(1).await.expect("Ok"), DeletePrompt::Confirmation);
    assert!(world.permission_requests().is_empty(), "no PermissionRequiredMediaDelete: no pre-check");
}

#[tokio::test(start_paused = true)]
async fn the_delete_action_needs_a_permission_on_android_13() {
    // Android 14 (the harness's osVersion): enabled while any of photo-delete, post-notification or
    // auto-launch is held; post-notification defaults to held.
    let world = delete_world(1, None).await;
    assert!(world.photos.status().unwrap().delete_enabled);
    world.h.app.update_phone_record(|r| r.set_permission(permission::POST_NOTIFICATIONS, false));
    assert!(!world.photos.status().unwrap().delete_enabled);
    assert_matches!(world.photos.delete(1).await.unwrap_err(), PhotosError::DeleteNotEnabled);
    world.h.app.update_phone_record(|r| r.set_permission(permission::AUTO_LAUNCH, true));
    assert!(world.photos.status().unwrap().delete_enabled);

    // The photo-delete permission alone also enables it.
    world.h.app.update_phone_record(|r| {
        r.set_permission(permission::AUTO_LAUNCH, false);
        r.set_permission(permission::PHOTOS_DELETE, true);
    });
    assert!(world.photos.status().unwrap().delete_enabled);
    world.h.app.update_phone_record(|r| r.set_permission(permission::PHOTOS_DELETE, false));
    assert!(!world.photos.status().unwrap().delete_enabled);

    // On Android 12 always.
    connect(&world, ValueSet::new().with("attributes", ValueSet::new().with("osVersion", "12.1"))).await;
    assert!(world.photos.status().unwrap().delete_enabled);

    // An osVersion whose text before the first `.` is not an integer: always.
    connect(&world, ValueSet::new().with("attributes", ValueSet::new().with("osVersion", "Baklava"))).await;
    assert!(world.photos.status().unwrap().delete_enabled);
    connect(&world, ValueSet::new().with("attributes", ValueSet::new().with("osVersion", "13"))).await;
    assert!(!world.photos.status().unwrap().delete_enabled);
}

#[tokio::test(start_paused = true)]
async fn the_silent_delete_permission_pre_check() {
    let world = delete_world(3, None).await;
    let phone = Phone::install(&world);
    phone.queue_permission(5); // Denied

    assert_eq!(world.photos.prepare_delete(1).await.expect("Ok"), DeletePrompt::MissingPermission);
    let p = permission_payload(&world, 0);
    assert_eq!(p.i32("permissionType"), Some(0x20_0000));
    assert_eq!(p.bool("silent"), Some(true));
    assert_eq!(world.h.messenger.requests_on(PERMISSION)[0].priority, 0, "user-initiated");
    assert!(!perm(&world, permission::PHOTOS_DELETE, false));

    // The prompt's action: a non-silent request; Success grants it, and the delete is not resumed.
    phone.queue_permission(0);
    assert_eq!(world.photos.request_delete_permission().await.unwrap(), AppOutcome::Success);
    assert_eq!(permission_payload(&world, 1).bool("silent"), Some(false));
    assert!(perm(&world, permission::PHOTOS_DELETE, false));
    assert!(world.media_requests().is_empty());

    // Held now: no pre-check.
    assert_eq!(world.photos.prepare_delete(1).await.expect("Ok"), DeletePrompt::Confirmation);
    assert_eq!(world.permission_requests().len(), 2);

    // An Accepted pre-check grants it.
    world.h.app.update_phone_record(|r| r.set_permission(permission::PHOTOS_DELETE, false));
    phone.queue_permission(4);
    assert_eq!(world.photos.prepare_delete(1).await.expect("Ok"), DeletePrompt::Confirmation);
    assert!(perm(&world, permission::PHOTOS_DELETE, false));
}

#[tokio::test(start_paused = true)]
async fn a_successful_delete_hides_the_item_after_250_ms_and_keeps_the_cache() {
    let world = delete_world(1, None).await;
    let phone = Phone::install(&world);
    let mut events = world.events();
    let photos = world.photos.clone();
    let deleting = tokio::spawn(async move { photos.delete(1).await });

    wait_held(&phone, 1).await;
    assert_eq!(world.photos.item(1).unwrap().unwrap().delete_state, DeleteState::Deleting);

    let req = phone.take();
    assert_eq!(
        req.payload.keys().take(4).collect::<Vec<_>>(),
        ["correlationVector", "requestType", "mediaIds", "checksums"]
    );
    assert_eq!(req.payload.i32("requestType"), Some(0));
    assert_eq!(req.ids(), [1]);
    assert_eq!(req.payload.i64_array("checksums"), Some(&[11i64][..]));
    assert!(req.content_type().is_none(), "no contentType");

    let sent = &world.h.messenger.requests_on(MEDIA)[0];
    assert_eq!(sent.timeout, Duration::from_secs(60), "MediaDeleteTimeoutMs");
    assert_eq!(sent.priority, 100, "not user-initiated");

    req.answer(delete_reply(&[1], &[]));
    let result = deleting.await.unwrap().unwrap();

    assert_eq!((result.deleted.clone(), result.reverted.clone(), result.failed_notice), (vec![1], vec![], false));
    assert_eq!(world.photos.item(1).unwrap().unwrap().delete_state, DeleteState::Deleted);
    assert_eq!(world.gallery_ids(), [3, 2, 1], "still shown for 250 ms");

    tokio::time::sleep(Duration::from_millis(250)).await;
    settle().await;
    assert_eq!(world.gallery_ids(), [3, 2]);
    assert!(world.photos.item(1).unwrap().is_some(), "the cache entry stays");
    assert!(drain(&mut events).iter().any(|e| matches!(e, PhotosEvent::DeleteFinished { .. })));

    // The phone's next data removes it from the cache.
    world.inbound(media_info(Some("n"), metadata(&[2, 3], &[2, 3], None, None))).await;
    assert!(world.photos.item(1).unwrap().is_none());
}

#[tokio::test(start_paused = true)]
async fn a_delete_confirmed_after_the_item_left_the_cache_reverts_nothing() {
    let world = delete_world(1, None).await;
    let phone = Phone::install(&world);
    let photos = world.photos.clone();
    let deleting = tokio::spawn(async move { photos.delete(1).await });

    wait_held(&phone, 1).await;
    let req = phone.take();

    // The phone's next data omits 1 and arrives before the delete reply.
    world.inbound(media_info(Some("n"), metadata(&[2, 3], &[2, 3], None, None))).await;
    assert!(world.photos.item(1).unwrap().is_none());

    req.answer(delete_reply(&[1], &[]));
    let result = deleting.await.unwrap().unwrap();
    assert_eq!(result.deleted, [1], "every id the phone reports deleted");
    assert!(result.reverted.is_empty(), "an id the phone reports deleted is not one of the other ids");
    assert!(!result.failed_notice);

    tokio::time::sleep(Duration::from_millis(250)).await;
    settle().await;
    assert_eq!(world.gallery_ids(), [3, 2]);
}

#[tokio::test(start_paused = true)]
async fn an_item_a_delete_hid_is_no_grid_item() {
    let world = delete_world(1, None).await;
    let phone = Phone::install(&world);
    let photos = world.photos.clone();
    let deleting = tokio::spawn(async move { photos.delete(1).await });

    wait_held(&phone, 1).await;
    phone.take().answer(delete_reply(&[1], &[]));
    deleting.await.unwrap().unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await;
    settle().await;
    assert_eq!(world.gallery_ids(), [3, 2]);
    assert!(world.photos.item(1).unwrap().is_some(), "the cache entry stays until the phone's data drops it");

    // A grid click or a context menu on the hidden item (a stale reference in the UI) fetches nothing.
    assert!(!world.photos.open_item(1).unwrap(), "a hidden item opens no viewer");
    world.photos.on_context_menu_opened(1).unwrap();
    settle().await;
    assert!(sent_full_ids(&world).is_empty());

    // A displayed item still does.
    assert!(world.photos.open_item(2).unwrap());
    wait_held(&phone, 1).await;
    assert_eq!(sent_full_ids(&world), [2]);
}

/// Deletes `id` on the phone, which reports it deleted, and lets the 250 ms pass.
async fn delete_on_phone(world: &World, phone: &Phone, id: i64) {
    let photos = world.photos.clone();
    let deleting = tokio::spawn(async move { photos.delete(id).await });
    wait_held(phone, 1).await;
    let req = phone.take();
    assert_eq!((req.content_type(), req.ids()), (None, vec![id]), "the delete request");
    req.answer(delete_reply(&[id], &[]));
    assert_eq!(deleting.await.unwrap().unwrap().deleted, [id]);

    tokio::time::sleep(Duration::from_millis(250)).await;
    settle().await;
}

#[tokio::test(start_paused = true)]
async fn a_fetch_reply_after_the_delete_does_not_bring_the_item_back() {
    let world = delete_world(1, None).await;
    seed(&world, &[it(4, 4).no_thumb()]).await;
    let phone = Phone::install(&world);

    // Quick Look on 1 fetches its full image, a context menu on 2 prefetches it, and 4 (no image yet)
    // asks for its thumbnail; the requests are on their way when the deletes happen.
    assert!(world.photos.open_item(1).unwrap());
    world.photos.on_context_menu_opened(2).unwrap();
    world.photos.on_item_visible(4).unwrap();
    wait_held(&phone, 3).await;

    let mut late: HashMap<i64, Held> = HashMap::new();
    for _ in 0..3 {
        let held = phone.take();
        late.insert(held.ids()[0], held);
    }

    assert_eq!(late[&4].content_type(), Some("thumbnails"));

    // 4 gets an image (its full bytes), so it can be deleted too.
    store_full_image(&world, &phone, 4, b"four").await;
    for id in [1, 2, 4] {
        delete_on_phone(&world, &phone, id).await;
    }

    assert_eq!(world.gallery_ids(), [3]);
    world.photos.on_viewer_closed().unwrap();

    // The replies to the requests sent before the deletes are still stored.
    let mut events = world.events();
    late.remove(&1).unwrap().answer(full_reply(&[(1, b"one")]));
    late.remove(&2).unwrap().answer(full_reply(&[(2, b"two")]));
    late.remove(&4).unwrap().answer(thumbnail_reply(&[it(4, 4)]));

    settle().await;
    assert!(world.photos.item(1).unwrap().unwrap().has_full && world.photos.item(2).unwrap().unwrap().has_full);
    assert!(world.photos.item(4).unwrap().unwrap().has_thumbnail);
    assert!(drain(&mut events).iter().any(|e| matches!(e, PhotosEvent::FullImageStored { id: 1, .. })));

    // They are no phone data: the deleted items stay out of the gallery, with no delete action.
    assert_eq!(world.gallery_ids(), [3]);
    for id in [1, 2, 4] {
        assert_matches!(world.photos.prepare_delete(id).await, Err(PhotosError::ItemDeleteNotEnabled(got0)) if got0 == id);
        assert_matches!(world.photos.delete(id).await.unwrap_err(), PhotosError::ItemDeleteNotEnabled(got0) if got0 == id);
    }

    settle().await;
    assert_eq!(phone.held_count(), 0, "no second delete reaches the phone");
    assert_eq!(world.media_of(None).len(), 3);
}

#[tokio::test(start_paused = true)]
async fn the_phones_item_data_that_writes_a_deleted_item_brings_it_back() {
    let world = delete_world(1, None).await;
    let phone = Phone::install(&world);
    delete_on_phone(&world, &phone, 1).await;
    assert_eq!(world.gallery_ids(), [3, 2]);

    // The same item again, unchanged (same times, URI and checksum, a thumbnail cached): the
    // message writes nothing, and the item stays out.
    seed(&world, &[it(1, 1).checksum(11)]).await;
    assert_eq!(world.gallery_ids(), [3, 2]);

    // The phone reports it changed: it is written, and back in the gallery with its delete action.
    let mut events = world.events();
    seed(&world, &[it(1, 1).times(1, 5).checksum(11)]).await;
    assert_eq!(world.gallery_ids(), [3, 2, 1]);
    assert!(drain(&mut events).iter().any(|e| matches!(e, PhotosEvent::GalleryChanged { .. })));
    assert_eq!(world.photos.item(1).unwrap().unwrap().delete_state, DeleteState::None);
    assert_eq!(world.photos.prepare_delete(1).await.expect("Ok"), DeletePrompt::Confirmation);
}

#[tokio::test(start_paused = true)]
async fn a_delete_reply_met_by_an_unreadable_store_still_ends_the_delete() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("phones").join(phone().to_string()).join("photos.sqlite3");
    let world = delete_world_at(1, None, StoreLocation::Directory(dir.path().to_path_buf())).await;

    let scripted = Phone::install(&world);
    let mut events = world.events();

    let photos = world.photos.clone();
    let deleting = tokio::spawn(async move { photos.delete(1).await });
    wait_held(&scripted, 1).await;

    // The cache table goes away under the open store while the delete is in flight.
    let conn = rusqlite::Connection::open(&file).unwrap();
    conn.execute_batch("ALTER TABLE media RENAME TO media_away;").unwrap();

    scripted.take().answer(delete_reply(&[1], &[]));
    let result = deleting.await.unwrap().unwrap();
    assert_eq!((result.deleted.clone(), result.failed_notice), (vec![1], false));
    assert!(drain(&mut events).iter().any(|e| matches!(e, PhotosEvent::DeleteFinished { .. })));

    // Readable again: the item is no longer "deleting" (nothing was taken out of a gallery that
    // could not be read), so its delete action is back.
    conn.execute_batch("ALTER TABLE media_away RENAME TO media;").unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await;
    settle().await;
    assert_eq!(world.photos.item(1).unwrap().unwrap().delete_state, DeleteState::None);
    assert_eq!(world.gallery_ids(), [3, 2, 1]);
    assert_eq!(world.photos.prepare_delete(1).await.expect("Ok"), DeletePrompt::Confirmation);
}

#[tokio::test(start_paused = true)]
async fn delete_reply_table() {
    // (reply, deleted, failed notice, PhotosDelete cleared)
    let cases: Vec<(phonelink_engine::seams::PlatformResponse, Vec<i64>, bool, bool)> = vec![
        (delete_reply(&[], &[2]), vec![], true, false),
        (delete_reply(&[], &[]), vec![], true, false),
        (delete_reply(&[2], &[3]), vec![2], true, false),
        (result_response(7), vec![], false, true),
        (result_response(1), vec![], false, false),
        (
            response_with(ValueSet::new().with("result", 0i32).with("mediaSuccessfullyDeletedIds", vec![2i64])),
            vec![],
            false,
            false,
        ),
        (
            response_with(
                ValueSet::new()
                    .with("result", 0i32)
                    .with("mediaSuccessfullyDeletedIds", vec![2i32])
                    .with("mediaUnsuccessfullyDeletedIds", Vec::<i64>::new()),
            ),
            vec![],
            false,
            false,
        ),
    ];

    for (i, (reply, deleted, notice, cleared)) in cases.into_iter().enumerate() {
        let world = delete_world(1, None).await;
        world.h.app.update_phone_record(|r| r.set_permission(permission::PHOTOS_DELETE, true));
        world.h.messenger.answer_all(reply);

        let result = world.photos.delete(2).await.unwrap();

        assert_eq!(result.deleted, deleted, "case {i}");
        assert_eq!(result.failed_notice, notice, "case {i}");
        assert_eq!(result.reverted, if deleted.is_empty() { vec![2] } else { vec![] }, "case {i}");
        assert_eq!(!perm(&world, permission::PHOTOS_DELETE, false), cleared, "case {i}");
        let state = world.photos.item(2).unwrap().unwrap().delete_state;
        assert_eq!(state, if deleted.is_empty() { DeleteState::None } else { DeleteState::Deleted }, "case {i}");
    }
}

// ------------------------------------------------------------------------------------------------
// Settings, maintenance
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn settings_and_the_cache_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let location = StoreLocation::Directory(dir.path().to_path_buf());
    {
        let world = world_at(PhotosConfig::default(), location.clone(), None);
        seed(&world, &[it(1, 1)]).await;

        world.photos.set_consent(false).unwrap();
        world.photos.set_deletion_allowed(false).unwrap();
        world.photos.set_new_photo_notifications(false).unwrap();
        world.photos.dismiss_full_access_reminder().unwrap();
        connect(
            &world,
            ValueSet::new()
                .with("result", 0i32)
                .with("media_info", 9i64)
                .with("capabilities", ValueSet::new().with("photos", 1i32)),
        )
        .await;
    }

    let world = world_at(PhotosConfig::default(), location, None);
    let status = world.photos.status().unwrap();
    assert!(!status.consent && !status.deletion_allowed && !status.new_photo_notifications);
    assert_eq!((status.expected_sequence, status.capabilities), (9, 1));
    assert_eq!(world.gallery_ids(), [1]);
    assert_eq!(world.photos.thumbnail(1).unwrap(), Some(vec![1, 1, 2]));
}

#[tokio::test(start_paused = true)]
async fn cache_maintenance_drops_old_full_bytes_outside_the_25_most_recent() {
    let world = world();
    let day = Duration::from_secs(86_400);
    let list: Vec<It> = (1..=30).map(|i| it(i, i)).collect();
    seed(&world, &list).await;
    let phone = Phone::install(&world);
    for id in 1..=30 {
        store_full_image(&world, &phone, id, b"x").await;
    }

    world.w.clock.advance(day * 31);
    world.photos.on_viewer_item_shown(3).unwrap();
    let mut events = world.events();
    world.photos.run_cache_maintenance();

    let with_full: Vec<i64> = world.photos.gallery().unwrap().iter().filter(|i| i.has_full).map(|i| i.id).collect();
    // The 25 most recent (30..=6) keep theirs; of 5..=1 only 3, seen just now.
    assert_eq!(with_full, (6..=30).rev().chain([3]).collect::<Vec<_>>());
    assert!(world.photos.gallery().unwrap().iter().all(|i| i.has_thumbnail), "thumbnails are never dropped");
    assert!(drain(&mut events).iter().any(|e| matches!(e, PhotosEvent::GalleryChanged { .. })));
}

#[tokio::test(start_paused = true)]
async fn cache_maintenance_runs_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let location = StoreLocation::Directory(dir.path().to_path_buf());
    {
        // A previous run, 40 days ago: 26 items, the oldest with full bytes, none seen since.
        let world = world_at(PhotosConfig::default(), location.clone(), None);
        world.w.clock.set(world.w.clock.now() - Duration::from_secs(40 * 86_400));
        let list: Vec<It> = (1..=26).map(|i| it(i, i)).collect();
        seed(&world, &list).await;
        let phone = Phone::install(&world);
        store_full_image(&world, &phone, 1, b"x").await;
        assert!(world.photos.item(1).unwrap().unwrap().has_full);
    }

    // This run's start drops it at once: no timer has to elapse.
    let start = tokio::time::Instant::now();
    let world = world_at(PhotosConfig::default(), location, None);
    settle().await;
    assert_eq!(start.elapsed(), Duration::ZERO);

    let item = world.photos.item(1).unwrap().unwrap();
    assert!(!item.has_full, "dropped by the run at start");
    assert!(item.has_thumbnail, "thumbnails are kept");
    assert!(world.photos.item(26).unwrap().is_some());
}

#[tokio::test(start_paused = true)]
async fn maintenance_runs_every_24_hours() {
    let world = world();
    seed(&world, &[it(1, 1)]).await;

    // The periodic run is observed through its dehydration: item 1 gets full bytes, falls outside
    // the 25 most recent and is not seen for 40 days.
    let phone = Phone::install(&world);
    store_full_image(&world, &phone, 1, b"x").await;
    let list: Vec<It> = (2..=26).map(|i| it(i, 1000 + i)).collect();
    seed(&world, &list).await;

    world.w.clock.advance(Duration::from_secs(40 * 86_400));
    assert!(world.photos.item(1).unwrap().unwrap().has_full);

    tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
    settle().await;
    assert!(!world.photos.item(1).unwrap().unwrap().has_full, "the daily run dropped it");
}

// ------------------------------------------------------------------------------------------------
// New-photo notification
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_just_taken_photo_is_notified_with_its_thumbnail() {
    let world = world();
    let now = world.now_ms();
    let fresh = it(7, now - 5_000).times(now - 5_000, now - 4_000).thumb(b"thumb");
    seed(&world, &[fresh, it(8, now - 600_000)]).await;

    let posted = world.w.notifier.posted();
    assert_eq!(posted.len(), 1);
    let n = &posted[0];
    assert!(n.id.starts_with("photos/"));
    assert_eq!(n.image.as_deref(), Some(&b"thumb"[..]));
    assert!(n.alert);
    assert_eq!(n.body, "IMG_7.jpg");
    assert_eq!(n.user_info.get(USER_INFO_MEDIA_ID).map(String::as_str), Some("7"));
    assert_eq!(n.user_info.get(USER_INFO_PHONE), Some(&phone().to_string()));

    // Within 7 s of the pop-up: silent; after: a pop-up again.
    world.w.clock.advance(Duration::from_secs(3));
    let now = world.now_ms();
    seed(&world, &[it(9, now - 1_000)]).await;
    assert!(!world.w.notifier.posted()[1].alert);

    world.w.clock.advance(Duration::from_secs(5));
    let now = world.now_ms();
    seed(&world, &[it(10, now - 1_000)]).await;
    assert!(world.w.notifier.posted()[2].alert, "7 s after the last pop-up, not after the silent one");
}

#[tokio::test(start_paused = true)]
async fn notification_rules() {
    let world = world();
    let now = world.now_ms();
    // Only the 3 most recent by taken time are considered; one without a thumbnail is not notified.
    let batch = [
        it(1, now - 1_000),
        it(2, now - 2_000).no_thumb(),
        it(3, now - 3_000),
        it(4, now - 4_000),
        it(5, now + 1_000).times(now + 1_000, now - 200_000),
    ];
    seed(&world, &batch).await;

    let ids: Vec<String> = world.w.notifier.posted().iter().map(|n| n.user_info[USER_INFO_MEDIA_ID].clone()).collect();
    // Candidates: 5 (taken/modified 201 s apart: no), 1, 2 (no thumbnail).
    assert_eq!(ids, ["1"]);

    // Off with the setting or without consent; not when the app may not post.
    world.photos.set_new_photo_notifications(false).unwrap();
    seed(&world, &[it(11, now)]).await;
    world.photos.set_new_photo_notifications(true).unwrap();
    world.photos.set_consent(false).unwrap();
    seed(&world, &[it(12, now)]).await;
    world.photos.set_consent(true).unwrap();
    world.w.notifier.set_authorized(false);
    seed(&world, &[it(13, now)]).await;
    assert_eq!(world.w.notifier.posted().len(), 1);

    // A thumbnail reply does not notify.
    world.w.notifier.set_authorized(true);
    seed(&world, &[it(14, now).no_thumb()]).await;

    let phone = Phone::install(&world);
    world.photos.on_item_visible(14).unwrap();
    wait_held(&phone, 1).await;
    phone.take().answer(thumbnail_reply(&[it(14, now)]));
    settle().await;
    assert_eq!(world.w.notifier.posted().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn activating_the_notification_fetches_and_opens_the_full_image() {
    let world = world();
    let now = world.now_ms();
    seed(&world, &[it(7, now - 1_000)]).await;
    let n = world.w.notifier.posted().pop().unwrap();
    let phone = Phone::install(&world);
    let mut events = world.events();

    world.w.responses.route(&n.id, &n.user_info, NotificationResponse::Activated);
    wait_held(&phone, 1).await;

    let req = phone.take();
    assert_eq!((req.content_type(), req.ids()), (Some("photos"), vec![7]));
    req.answer(full_reply(&[(7, b"full")]));
    settle().await;
    assert!(drain(&mut events).iter().any(|e| *e == PhotosEvent::OpenImage { phone: phone_id(), id: 7 }));

    // With the bytes cached: opened at once.
    world.w.responses.route(&n.id, &n.user_info, NotificationResponse::Activated);
    settle().await;
    assert!(drain(&mut events).iter().any(|e| matches!(e, PhotosEvent::OpenImage { id: 7, .. })));
    assert_eq!(phone.held_count(), 0);

    // Dismissed: nothing; consent off: nothing.
    world.w.responses.route(&n.id, &n.user_info, NotificationResponse::Dismissed);
    world.photos.set_consent(false).unwrap();
    world.w.responses.route(&n.id, &n.user_info, NotificationResponse::Activated);
    settle().await;
    assert!(!drain(&mut events).iter().any(|e| matches!(e, PhotosEvent::OpenImage { .. })));

    // Unknown user info: ignored.
    world.w.responses.route("photos/x", &HashMap::new(), NotificationResponse::Activated);
}

#[tokio::test(start_paused = true)]
async fn an_activation_whose_image_does_not_arrive_in_30_s_opens_nothing() {
    let world = world();
    let now = world.now_ms();
    seed(&world, &[it(7, now - 1_000)]).await;
    let n = world.w.notifier.posted().pop().unwrap();
    let phone = Phone::install(&world);
    let mut events = world.events();

    world.w.responses.route(&n.id, &n.user_info, NotificationResponse::Activated);
    wait_held(&phone, 1).await;

    tokio::time::sleep(Duration::from_secs(31)).await;
    let late = phone.take();
    late.answer(full_reply(&[(7, b"late")]));
    settle().await;
    assert!(!drain(&mut events).iter().any(|e| matches!(e, PhotosEvent::OpenImage { .. })));
}

#[tokio::test(start_paused = true)]
async fn an_activation_after_the_item_left_the_cache_does_nothing() {
    let world = world();
    let now = world.now_ms();
    seed(&world, &[it(7, now - 1_000)]).await;
    let n = world.w.notifier.posted().pop().unwrap();
    world.inbound(media_info(Some("m"), metadata(&[], &[], None, None))).await;
    let mut events = world.events();

    world.w.responses.route(&n.id, &n.user_info, NotificationResponse::Activated);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(world.media_requests().is_empty());
    assert!(!drain(&mut events).iter().any(|e| matches!(e, PhotosEvent::OpenImage { .. })));
}

fn phone_id() -> phonelink_engine::types::DcgClient {
    phone()
}

// ------------------------------------------------------------------------------------------------
// The gallery view
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn status_and_gallery_basics() {
    let world = world();
    let status = world.photos.status().unwrap();
    assert!(status.enabled && status.consent && status.photos_permission && status.up_to_date);
    assert!(!status.photos_full_permission && !status.photos_delete_permission && !status.syncing);
    assert_eq!((status.snapshot_size, status.capabilities, status.last_sync_ms), (2000, 0, None));

    seed(&world, &[it(1, 50).times(100, 50), it(2, 60).times(100, 60), it(3, 10).times(200, 10)]).await;

    // Taken DESC, then last-modified DESC.
    assert_eq!(world.gallery_ids(), [3, 2, 1]);
    let item = world.photos.item(2).unwrap().unwrap();
    assert!(item.is_actionable() && !item.has_full);
    assert_eq!((item.height, item.width, item.size, item.mime_type.as_str()), (4032, 3024, 2_000_000, "image/jpeg"));
    let name = world.photos.save_as_name(2).unwrap();
    assert_eq!((name.stem.as_str(), name.extension.as_str()), ("IMG_2", ".jpg"));

    // Viewing stamps the last-seen time.
    world.w.clock.advance(Duration::from_secs(10));
    world.photos.on_viewer_item_shown(2).unwrap();
    assert_eq!(world.photos.item(2).unwrap().unwrap().last_seen_ms, Some(world.now_ms()));

    // The payload of the connect is not affected by the PC-side settings.
    world.photos.set_consent(false).unwrap();
    world.h.app.send_connect(ConnectKind::Background, "Test").await;
    let vs = valueset::decode(&world.h.messenger.requests_on("/connect/v1")[0].message.payload).unwrap();
    assert_eq!(vs.object("configuration").unwrap().bool("MediaInfoSync"), Some(true));
}

// ------------------------------------------------------------------------------------------------
// Delete action availability, trace context names
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_disabled_delete_action_sends_no_pre_check() {
    // MediaDelete | PermissionRequiredMediaDelete on Android 14 with none of the permissions that
    // enable the action: `delete` is refused, and so is its pre-check.
    let world = delete_world(3, None).await;
    world.h.app.update_phone_record(|r| r.set_permission(permission::POST_NOTIFICATIONS, false));
    assert!(!world.photos.status().unwrap().delete_enabled);

    let phone = Phone::install(&world);
    phone.queue_permission(0);

    assert_matches!(world.photos.prepare_delete(1).await, Err(PhotosError::DeleteNotEnabled));
    assert!(world.permission_requests().is_empty(), "nothing is sent");
    assert!(!perm(&world, permission::PHOTOS_DELETE, false));
}

#[tokio::test(start_paused = true)]
async fn a_second_delete_of_an_item_already_deleting_sends_nothing() {
    let world = delete_world(1, None).await;
    let phone = Phone::install(&world);
    let photos = world.photos.clone();
    let first = tokio::spawn(async move { photos.delete(1).await });

    wait_held(&phone, 1).await;

    assert_matches!(world.photos.delete(1).await.unwrap_err(), PhotosError::ItemDeleteNotEnabled(1));
    assert_matches!(world.photos.prepare_delete(1).await, Err(PhotosError::ItemDeleteNotEnabled(1)));
    settle().await;
    assert_eq!(world.media_of(None).len(), 1, "one delete request");

    // Another item's delete action is unaffected.
    assert_eq!(world.photos.prepare_delete(2).await.expect("Ok"), DeletePrompt::Confirmation);
    phone.take().answer(delete_reply(&[1], &[]));
    first.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn the_delete_action_needs_an_item_that_shows_an_image() {
    let world = delete_world(1, None).await;
    seed(&world, &[it(4, 4).no_thumb()]).await;
    let _phone = Phone::install(&world);

    assert_matches!(world.photos.delete(4).await.unwrap_err(), PhotosError::ItemDeleteNotEnabled(4));
    assert_matches!(world.photos.prepare_delete(4).await, Err(PhotosError::ItemDeleteNotEnabled(4)));
    assert!(world.media_of(None).is_empty());
    assert!(world.permission_requests().is_empty());
    assert_eq!(world.photos.item(4).unwrap().unwrap().delete_state, DeleteState::None);

    // Not cached at all.
    assert_matches!(world.photos.prepare_delete(99).await, Err(PhotosError::NotCached(99)));
}

/// `(ypTriggerId, ypScenarioId)` of a sent request's hub trace state.
fn names(r: &phonelink_engine::seams::PlatformRequest) -> (String, String) {
    let get =
        |k: &str| r.trace.trace_state.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone()).unwrap_or_default();
    (get("ypTriggerId"), get("ypScenarioId"))
}

fn pair(trigger: &str, scenario: &str) -> (String, String) {
    (trigger.to_string(), scenario.to_string())
}

#[tokio::test(start_paused = true)]
async fn the_sync_delete_and_permission_requests_carry_phone_link_trace_names() {
    // Photo access, then its media sync: the sync keeps the permission request's trigger.
    let world = world();
    let phone = Phone::install(&world);
    world.photos.request_photo_access().await.unwrap();
    wait_held(&phone, 1).await;

    assert_eq!(
        names(&world.h.messenger.requests_on(PERMISSION)[0]),
        pair("ErrorActionInvoked", "PhotosDevicePermission")
    );
    assert_eq!(names(&world.h.messenger.requests_on(MEDIA)[0]), pair("ErrorActionInvoked", "SyncAllMedia"));
    phone.take().answer(result_response(0));
    settle().await;

    // Full access, then its media sync.
    world.photos.request_full_access().await.unwrap();
    wait_held(&phone, 1).await;

    assert_eq!(
        names(&world.h.messenger.requests_on(PERMISSION)[1]),
        pair("PhotosFullPermissionRequestInvoked", "PhotosDeviceFullPermission")
    );
    assert_eq!(
        names(&world.h.messenger.requests_on(MEDIA)[1]),
        pair("PhotosFullPermissionRequestInvoked", "SyncAllMedia")
    );
    phone.take().answer(result_response(0));

    // The delete pre-check, the missing-permission prompt and the delete.
    let world = delete_world(3, None).await;
    let phone = Phone::install(&world);
    phone.queue_permission(5);
    world.photos.prepare_delete(1).await.unwrap();
    phone.queue_permission(0);
    world.photos.request_delete_permission().await.unwrap();

    for r in world.h.messenger.requests_on(PERMISSION) {
        assert_eq!(names(&r), pair("PermissionRequired", "PhotosDeletePermission"));
    }

    let photos = world.photos.clone();
    let deleting = tokio::spawn(async move { photos.delete(1).await });
    wait_held(&phone, 1).await;

    assert_eq!(names(&world.h.messenger.requests_on(MEDIA)[0]), pair("DeleteInvoked", "DeleteMedia"));
    phone.take().answer(delete_reply(&[1], &[]));
    deleting.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn the_notification_activation_fetch_carries_the_toast_click_trace_names() {
    let world = world();
    let now = world.now_ms();
    seed(&world, &[it(7, now - 1_000)]).await;
    let n = world.w.notifier.posted().pop().unwrap();

    let phone = Phone::install(&world);
    world.w.responses.route(&n.id, &n.user_info, NotificationResponse::Activated);
    wait_held(&phone, 1).await;

    assert_eq!(names(&world.h.messenger.requests_on(MEDIA)[0]), pair("OnRemoteSnippingToastClick", "HydratePhoto"));
    phone.take().answer(full_reply(&[(7, b"full")]));
}

#[tokio::test(start_paused = true)]
async fn the_media_sync_reply_sets_the_photos_permission() {
    // (sync reply, Photos when it arrives, Photos after): only the reply status is used.
    for (code, before, after) in
        [(0, false, true), (4, false, true), (7, true, false), (5, true, false), (6, true, false), (1, false, false)]
    {
        let world = world();
        let phone = Phone::install(&world);
        world.photos.request_photo_access().await.unwrap();
        wait_held(&phone, 1).await;
        world.h.app.update_phone_record(|r| r.set_permission(permission::PHOTOS, before));

        phone.take().answer(result_response(code));
        settle().await;

        assert_eq!(perm(&world, permission::PHOTOS, !after), after, "code {code}");
        assert!(!world.photos.status().unwrap().syncing, "code {code}: the indicator hides after the reply");
    }
}

#[tokio::test(start_paused = true)]
async fn an_activation_with_new_photo_notifications_off_does_nothing() {
    let world = world();
    let now = world.now_ms();
    seed(&world, &[it(7, now - 1_000)]).await;
    let n = world.w.notifier.posted().pop().unwrap();
    world.photos.set_new_photo_notifications(false).unwrap();
    let mut events = world.events();

    world.w.responses.route(&n.id, &n.user_info, NotificationResponse::Activated);
    tokio::time::sleep(Duration::from_secs(1)).await;

    assert!(world.media_requests().is_empty());
    assert!(!drain(&mut events).iter().any(|e| matches!(e, PhotosEvent::OpenImage { .. })));
}

/// Items 1..=6 taken an hour ago (no notification), then item 7 just taken (notified); returns the
/// notification of 7.
async fn gallery_with_a_new_photo(world: &World) -> phonelink_engine::features::shared::notifier::SystemNotification {
    let now = world.now_ms();
    let old: Vec<It> = (1..=6).map(|i| it(i, now - 3_600_000 - i)).collect();
    seed(world, &old).await;
    seed(world, &[it(7, now - 1_000)]).await;

    let posted = world.w.notifier.posted();
    assert_eq!(posted.len(), 1);
    posted[0].clone()
}

fn sent_full_ids(world: &World) -> Vec<i64> {
    world.media_of(Some("photos")).iter().map(|p| p.i64_array("mediaIds").unwrap()[0]).collect()
}

#[tokio::test(start_paused = true)]
async fn closing_the_viewer_does_not_cancel_the_notification_activation_fetch() {
    let world = world();
    let n = gallery_with_a_new_photo(&world).await;
    let phone = Phone::install(&world);

    // Gallery fetches take the five slots; the activation's fetch of 7 waits behind them, unmarked.
    for id in 1..=5 {
        world.photos.open_item(id).unwrap();
    }

    wait_held(&phone, 5).await;
    world.w.responses.route(&n.id, &n.user_info, NotificationResponse::Activated);
    settle().await;

    // Closing the viewer cancels the gallery's fetches only.
    world.photos.on_viewer_closed().unwrap();
    let held = phone.take();
    let id = held.ids()[0];
    held.answer(full_reply(&[(id, b"x")]));

    wait_held(&phone, 5).await;
    assert!(sent_full_ids(&world).contains(&7), "the activation's fetch is sent once a slot frees");
}

#[tokio::test(start_paused = true)]
async fn leaving_the_gallery_cancels_the_queued_full_resolution_fetches() {
    let world = world();
    let n = gallery_with_a_new_photo(&world).await;
    let phone = Phone::install(&world);

    for id in 1..=6 {
        world.photos.open_item(id).unwrap();
    }

    wait_held(&phone, 5).await;

    // Queued behind the five in flight: the gallery's 1 and the activation's 7.
    world.w.responses.route(&n.id, &n.user_info, NotificationResponse::Activated);
    settle().await;
    world.photos.on_gallery_left().unwrap();

    // The five in flight complete and are applied; nothing queued is sent.
    while phone.held_count() > 0 {
        let held = phone.take();
        let id = held.ids()[0];
        held.answer(full_reply(&[(id, b"x")]));
    }

    settle().await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(phone.held_count(), 0);

    let mut sent = sent_full_ids(&world);
    sent.sort_unstable();
    assert_eq!(sent, [2, 3, 4, 5, 6], "1 and the activation's 7 were cancelled");
    assert!(world.photos.item(6).unwrap().unwrap().has_full, "a reply to a sent request is still applied");
}
