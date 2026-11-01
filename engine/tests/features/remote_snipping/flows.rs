//! New-photo notifications: the capability request and its triggers, the tag-6 publication and the
//! notification (rate limit, window, silent pop-up, expiry, activation), tag 3 and the permission
//! request, and which phones the feature covers for the photos feature's own notification.

use crate::support::platform_world;
use crate::support::remote_snipping_world as world;
use phonelink_engine::config::features::RemoteSnippingConfig;

use std::time::Duration;

use prost::Message;

use phonelink_engine::auth::PeerDevice;
use phonelink_engine::features::remote_snipping::{PermissionOutcome, RemoteSnippingEvent};
use phonelink_engine::features::shared::notifier::NotificationResponse;
use phonelink_engine::proto::platform_sdk::v1 as pb;
use phonelink_engine::proto::remotesnipping::v1 as rs;

use platform_world::{drm_answer, phone_id, settle};
use world::{SnipWorld, new_photo, permission_publication, phone_item};

/// The capability request's `/DeviceResourceManager` body.
const CAPABILITY_BODY: [u8; 16] =
    [0x08, 0x06, 0x10, 0x01, 0x1a, 0x02, 0x08, 0x01, 0x22, 0x06, 0x2f, 0x6d, 0x65, 0x64, 0x69, 0x61];
/// The permission request's body.
const PERMISSION_BODY: [u8; 23] = [
    0x08, 0x03, 0x10, 0x04, 0x1a, 0x03, 0x0a, 0x01, 0x03, 0x22, 0x0c, 0x2f, 0x70, 0x65, 0x72, 0x6d, 0x69, 0x73, 0x73,
    0x69, 0x6f, 0x6e, 0x73,
];

fn trace(names: (&str, &str)) -> Vec<(String, String)> {
    vec![("ypScenarioId".into(), names.0.into()), ("ypTriggerId".into(), names.1.into())]
}

async fn connected_world() -> SnipWorld {
    let world = SnipWorld::new(phone_item());
    world.w.connect_phone().await;
    world
}

#[tokio::test(start_paused = true)]
async fn an_available_phone_is_asked_for_its_capability_at_start() {
    let world = connected_world().await;
    world.phone_serves(true, true, None, None);
    let feature = world.install();
    settle().await;

    let requests = world.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0.encode_to_vec(), CAPABILITY_BODY);
    assert_eq!(requests[0].1, trace(("CDEH.RemoteSnipping.RefreshDeviceCapabilities", "RemoteSnippingServiceStarted")));

    let settings = feature.settings(&phone_id()).unwrap();
    assert!(settings.applicable && settings.enabled && !settings.needs_permission && !settings.requesting_permission);
    assert!(settings.covers_new_photo_notifications);
}

#[tokio::test(start_paused = true)]
async fn the_capability_request_goes_to_enabled_android_phones_only() {
    // Not Android.
    let world = SnipWorld::new(phonelink_engine::types::DeviceInfoItem { os_name: Some("iOS".into()), ..phone_item() });
    world.w.connect_phone().await;
    let _feature = world.install();
    settle().await;
    assert!(world.requests().is_empty());

    // Disabled.
    let world = connected_world().await;
    world.switches.set_enabled(&phone_id(), false).unwrap();
    let _feature = world.install();
    settle().await;
    assert!(world.requests().is_empty());

    // Off.
    let world = connected_world().await;
    world.install_with(RemoteSnippingConfig { enabled: false, ..Default::default() });
    settle().await;
    assert!(world.requests().is_empty());
}

#[tokio::test(start_paused = true)]
async fn availability_and_the_enabled_flag_ask_again_and_a_new_phone_is_asked_with_a_session_start() {
    // Not available at start: nothing.
    let world = SnipWorld::new(phone_item());
    let _feature = world.install();
    settle().await;
    assert!(world.requests().is_empty());

    // Available: asked.
    world.w.connect_phone().await;
    settle().await;
    assert_eq!(world.media_requests().len(), 1);

    // The enabled flag changes while available: asked again (not while off).
    world.switches.set_enabled(&phone_id(), false).unwrap();
    settle().await;
    assert_eq!(world.media_requests().len(), 1);
    world.switches.set_enabled(&phone_id(), true).unwrap();
    settle().await;
    assert_eq!(world.media_requests().len(), 2);

    // A phone added to the account is asked at once (with a forced session start: it is connected
    // here, so the request goes out directly).
    world.peers.remove(&phone_id());
    world.peers.upsert(PeerDevice { client: phone_id(), info: phone_item() });
    settle().await;
    assert_eq!(world.media_requests().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn the_capability_answer_decides_what_the_feature_covers() {
    // Never answered (not available, no request yet): covered.
    let world = SnipWorld::new(phone_item());
    let feature = world.install();
    settle().await;
    assert!(feature.covers(&phone_id()), "never answered: covered");

    // A reply that parses with no capabilities at all: not covered.
    let world = connected_world().await;
    let feature = world.install();
    settle().await;
    assert!(!feature.covers(&phone_id()));

    // A reply without the capability: not covered, until a new photo is published.
    let world = connected_world().await;
    world.phone_serves(false, true, Some(vec![1, 2, 3]), None);
    let feature = world.install();
    settle().await;
    assert!(!feature.covers(&phone_id()));
    assert!(!feature.settings(&phone_id()).unwrap().applicable);

    world.new_photo("m-1", 5, Duration::from_secs(1)).await;
    assert!(feature.covers(&phone_id()), "the phone publishes new photos after all");

    // No MEDIA handler: not covered.
    let world = connected_world().await;
    world.answer(|_| Some(drm_answer(&[], Some(2))));
    let feature = world.install();
    settle().await;
    assert!(!feature.covers(&phone_id()));

    // A failed request keeps the answer (never answered: covered).
    let world = connected_world().await;
    world.answer(|_| None);
    let feature = world.install();
    settle().await;
    assert!(feature.covers(&phone_id()));

    // Option off or phone disabled: not covered.
    let world = connected_world().await;
    let off = world.install_with(RemoteSnippingConfig { enabled: false, ..Default::default() });
    assert!(!off.covers(&phone_id()));

    let world = connected_world().await;
    world.phone_serves(true, true, None, None);
    let feature = world.install();
    settle().await;
    assert!(feature.covers(&phone_id()));
    world.switches.set_enabled(&phone_id(), false).unwrap();
    assert!(!feature.covers(&phone_id()));
}

#[tokio::test(start_paused = true)]
async fn a_new_photo_is_notified_with_its_thumbnail_and_opens_the_photo() {
    let world = connected_world().await;
    world.phone_serves(true, true, Some(vec![0xAA, 0xBB]), Some(vec![0xCC; 4]));
    let feature = world.install();
    let mut events = feature.subscribe();
    settle().await;

    world.new_photo("m-1", 42, Duration::from_secs(3)).await;

    let media = world.media_requests();
    assert_eq!(media.len(), 2);
    assert_eq!(
        media[1],
        rs::MediaRequestMessage { request_type: rs::MediaRequestType::Thumbnail as i32, media_id: Some(42) }
    );
    assert_eq!(world.requests()[1].1, trace(("CDEH.RemoteSnipping.GetThumbnailScenarioId", "NewPhotoEventReceived")));

    let posted = world.notifier.posted();
    assert_eq!(posted.len(), 1);
    let n = &posted[0];
    assert_eq!(n.title, "New photo");
    assert_eq!(n.body, "Select here to view, edit, or share the photo");
    assert_eq!(n.subtitle.as_deref(), Some("Galaxy"));
    assert_eq!(n.image.as_deref(), Some(&[0xAA, 0xBB][..]));
    assert!(n.alert && n.actions.is_empty());
    assert!(n.id.starts_with("remote_snipping/"), "{}", n.id);

    SnipWorld::events(&mut events);
    world.responses.route(&n.id, &n.user_info, NotificationResponse::Activated);
    settle().await;

    let media = world.media_requests();
    assert_eq!(
        media[2],
        rs::MediaRequestMessage { request_type: rs::MediaRequestType::Photo as i32, media_id: Some(42) }
    );
    assert_eq!(world.requests()[2].1, trace(("CDEH.RemoteSnipping.GetPhotoScenarioId", "NewPhotoToastClicked")));
    let opened: Vec<_> = SnipWorld::events(&mut events)
        .into_iter()
        .filter_map(|e| match e {
            RemoteSnippingEvent::OpenImage { media_id, bytes, .. } => Some((media_id, bytes)),
            _ => None,
        })
        .collect();
    assert_eq!(opened, [(42, vec![0xCC; 4])]);
}

#[tokio::test(start_paused = true)]
async fn the_activation_falls_back_to_the_thumbnail() {
    let world = connected_world().await;
    world.phone_serves(true, true, Some(vec![7, 7]), None);
    let feature = world.install();
    let mut events = feature.subscribe();
    settle().await;

    world.new_photo("m-1", 9, Duration::ZERO).await;
    let n = world.notifier.posted()[0].clone();
    SnipWorld::events(&mut events);
    world.responses.route(&n.id, &n.user_info, NotificationResponse::Activated);
    settle().await;

    assert!(SnipWorld::events(&mut events).contains(&RemoteSnippingEvent::OpenImage {
        phone: phone_id(),
        media_id: 9,
        bytes: vec![7, 7]
    }));

    // A dismissed notification keeps nothing: its activation (from an earlier run, say) has no
    // fallback and opens nothing when the photo does not come.
    world.new_photo("m-2", 10, Duration::ZERO).await;
    tokio::time::advance(Duration::from_secs(2)).await;
    world.clock.advance(Duration::from_secs(2));
    world.new_photo("m-3", 11, Duration::ZERO).await;

    let n = world.notifier.posted().last().unwrap().clone();
    world.responses.route(&n.id, &n.user_info, NotificationResponse::Dismissed);
    SnipWorld::events(&mut events);
    world.responses.route(&n.id, &n.user_info, NotificationResponse::Activated);
    settle().await;
    assert!(!SnipWorld::events(&mut events).iter().any(|e| matches!(e, RemoteSnippingEvent::OpenImage { .. })));
}

#[tokio::test(start_paused = true)]
async fn the_rate_limit() {
    let world = connected_world().await;
    world.phone_serves(true, true, Some(vec![1]), None);
    let _feature = world.install();
    settle().await;
    let thumbnails = |w: &SnipWorld| {
        w.media_requests().iter().filter(|m| m.request_type == rs::MediaRequestType::Thumbnail as i32).count()
    };

    world.new_photo("m-1", 1, Duration::from_secs(1)).await;
    assert_eq!(thumbnails(&world), 1);

    // Within 1 s of the last attempt: nothing, and the refused attempt does not take the slot.
    world.clock.advance(Duration::from_millis(500));
    world.new_photo("m-2", 2, Duration::ZERO).await;
    assert_eq!(thumbnails(&world), 1);

    world.clock.advance(Duration::from_millis(500));
    world.new_photo("m-3", 3, Duration::ZERO).await;
    assert_eq!(thumbnails(&world), 2, "1 s after the first attempt");
}

#[tokio::test(start_paused = true)]
async fn the_window_and_the_rate_limit_slot() {
    let world = connected_world().await;
    world.phone_serves(true, true, Some(vec![1]), None);
    let _feature = world.install();
    settle().await;
    let thumbnails = |w: &SnipWorld| {
        w.media_requests().iter().filter(|m| m.request_type == rs::MediaRequestType::Thumbnail as i32).count()
    };

    // 120 s old: not notified, but the slot is taken.
    world.new_photo("m-1", 1, Duration::from_secs(120)).await;
    assert_eq!(thumbnails(&world), 0);
    world.clock.advance(Duration::from_millis(999));
    world.new_photo("m-2", 2, Duration::ZERO).await;
    assert_eq!(thumbnails(&world), 0, "within 1 s of the refused attempt");

    world.clock.advance(Duration::from_millis(1));
    world.new_photo("m-3", 3, Duration::from_millis(119_999)).await;
    assert_eq!(thumbnails(&world), 1);

    // A last-modified time in the future passes; an absent one does not.
    world.clock.advance(Duration::from_secs(1));
    let future = world.clock_now() + Duration::from_secs(600);
    world.publish(6, "m-4", new_photo(4, Some(future))).await;
    assert_eq!(thumbnails(&world), 2);
    world.clock.advance(Duration::from_secs(1));
    world.publish(6, "m-5", new_photo(5, None)).await;
    assert_eq!(thumbnails(&world), 2);
}

#[tokio::test(start_paused = true)]
async fn a_pop_up_within_7_s_is_silent_and_a_notification_expires_after_5_minutes() {
    let world = connected_world().await;
    world.phone_serves(true, true, Some(vec![1]), None);
    let _feature = world.install();
    settle().await;

    world.new_photo("m-1", 1, Duration::ZERO).await;
    world.clock.advance(Duration::from_secs(3));
    world.new_photo("m-2", 2, Duration::ZERO).await;
    world.clock.advance(Duration::from_secs(5));
    world.new_photo("m-3", 3, Duration::ZERO).await;

    let alerts: Vec<bool> = world.notifier.posted().iter().map(|n| n.alert).collect();
    // 3 s after the pop-up: silent; 8 s after it (the silent one does not count): pops up.
    assert_eq!(alerts, [true, false, true]);

    let first = world.notifier.posted()[0].id.clone();
    assert!(world.notifier.withdrawn().is_empty());

    tokio::time::advance(Duration::from_secs(300)).await;
    settle().await;
    assert!(world.notifier.withdrawn().contains(&first));
    assert_eq!(world.notifier.withdrawn().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn publications_that_are_not_notified() {
    let world = connected_world().await;
    world.phone_serves(true, true, Some(vec![1]), None);
    let feature = world.install();
    settle().await;
    let before = world.media_requests().len();

    // Malformed, no item, another change type, unauthorized, the setting off, the phone disabled.
    world.publish(6, "m-1", vec![0x12, 0x05]).await;
    world.publish(6, "m-2", rs::MediaChangeMessage { change_type: 1, changed_media: vec![] }.encode_to_vec()).await;
    world.publish(6, "m-3", rs::MediaChangeMessage { change_type: 0, ..new_change() }.encode_to_vec()).await;
    world.notifier.set_authorized(false);
    world.new_photo("m-4", 4, Duration::ZERO).await;
    world.notifier.set_authorized(true);
    world.clock.advance(Duration::from_secs(2));
    feature.set_enabled(&phone_id(), false).unwrap();
    world.new_photo("m-5", 5, Duration::ZERO).await;
    feature.set_enabled(&phone_id(), true).unwrap();
    world.switches.set_enabled(&phone_id(), false).unwrap();
    world.new_photo("m-6", 6, Duration::ZERO).await;

    assert_eq!(world.media_requests().len(), before);
    assert!(world.notifier.posted().is_empty());
}

fn new_change() -> rs::MediaChangeMessage {
    rs::MediaChangeMessage {
        change_type: 1,
        changed_media: vec![rs::MediaMetadata { media_id: 1, ..Default::default() }],
    }
}

#[tokio::test(start_paused = true)]
async fn an_empty_thumbnail_posts_nothing() {
    let world = connected_world().await;
    world.phone_serves(true, true, Some(Vec::new()), None);
    let _feature = world.install();
    settle().await;

    world.new_photo("m-1", 1, Duration::ZERO).await;
    assert!(world.notifier.posted().is_empty());
}

#[tokio::test(start_paused = true)]
async fn tag_3_and_every_reply_set_the_permission() {
    let world = connected_world().await;
    world.phone_serves(true, false, Some(vec![1]), None);
    let feature = world.install();
    settle().await;
    assert!(feature.settings(&phone_id()).unwrap().needs_permission);

    world.publish(3, "p-1", permission_publication(false)).await;
    assert!(feature.settings(&phone_id()).unwrap().needs_permission);
    world.publish(3, "p-2", permission_publication(true)).await;
    assert!(!feature.settings(&phone_id()).unwrap().needs_permission);

    // A thumbnail reply with has_permissions false clears it again.
    world.new_photo("m-1", 1, Duration::ZERO).await;
    assert!(feature.settings(&phone_id()).unwrap().needs_permission);
}

#[tokio::test(start_paused = true)]
async fn the_permission_request() {
    use phonelink_engine::proto::permissions::v1 as perm;
    let world = connected_world().await;
    let feature = world.install();
    settle().await;

    let reply = |status: perm::PermissionsRequestStatus, photos: bool| {
        perm::PermissionsResponseMessage {
            request_status: status as i32,
            feature_permissions: if photos { vec![perm::FeaturePermissions::Photos as i32] } else { vec![] },
            ..Default::default()
        }
        .encode_to_vec()
    };

    let r = reply(perm::PermissionsRequestStatus::Accepted, false);
    world.answer(move |m| {
        (m.resource_type == pb::ProtoDeviceResourceType::Permissions as i32).then(|| drm_answer(&r, Some(1)))
    });
    assert_eq!(feature.request_permission(&phone_id()).await.unwrap(), PermissionOutcome::RequestSent);
    let (body, trace_state) = world.requests().last().unwrap().clone();
    assert_eq!(body.encode_to_vec(), PERMISSION_BODY);
    assert_eq!(trace_state, trace(("CDEH.RemoteSnipping.RequestMediaPermissions", "SettingsItemHyperlinkClicked")));
    assert!(feature.settings(&phone_id()).unwrap().needs_permission);

    let r = reply(perm::PermissionsRequestStatus::Accepted, true);
    world.answer(move |_| Some(drm_answer(&r, Some(1))));
    assert_eq!(feature.request_permission(&phone_id()).await.unwrap(), PermissionOutcome::Granted);
    assert!(!feature.settings(&phone_id()).unwrap().needs_permission);

    let r = reply(perm::PermissionsRequestStatus::TypeUnsupported, true);
    world.answer(move |_| Some(drm_answer(&r, Some(1))));
    assert!(matches!(feature.request_permission(&phone_id()).await.unwrap(), PermissionOutcome::Failed(_)));
    assert!(!feature.settings(&phone_id()).unwrap().requesting_permission);
}

#[tokio::test(start_paused = true)]
async fn a_phone_added_while_unavailable_is_asked_with_a_session_start() {
    let world = SnipWorld::new(phone_item());
    let _feature = world.install();
    settle().await;
    assert!(world.w.waker.calls().is_empty(), "an unavailable phone is not asked at start");

    world.peers.remove(&phone_id());
    world.peers.upsert(PeerDevice { client: phone_id(), info: phone_item() });
    settle().await;
    assert!(!world.w.waker.calls().is_empty(), "the forced session start wakes the phone");
}

#[tokio::test(start_paused = true)]
async fn a_removed_phone_has_its_notifications_withdrawn() {
    let world = connected_world().await;
    world.phone_serves(true, true, Some(vec![1]), None);
    let _feature = world.install();
    settle().await;

    world.new_photo("m-1", 1, Duration::ZERO).await;
    let id = world.notifier.posted()[0].id.clone();

    world.peers.remove(&phone_id());
    settle().await;
    assert_eq!(world.notifier.withdrawn(), [id]);
}

#[tokio::test(start_paused = true)]
async fn a_second_notification_of_one_photo_replaces_the_first() {
    let world = connected_world().await;
    world.phone_serves(true, true, Some(vec![1]), None);
    let _feature = world.install();
    settle().await;

    world.new_photo("m-1", 1, Duration::ZERO).await;
    world.clock.advance(Duration::from_secs(2));
    world.new_photo("m-2", 1, Duration::ZERO).await;

    let ids: Vec<String> = world.notifier.posted().iter().map(|n| n.id.clone()).collect();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], ids[1]);
}
