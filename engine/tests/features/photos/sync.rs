//! The inbound `mediaInfo` sync: the media-list metadata and items, malformed inbound data and the
//! replies to it, and a worked example.

use crate::support::app_world;
use crate::support::photos_world as support;
use phonelink_engine::config::features::PhotosConfig;

use app_world::*;
use phonelink_engine::app::ConnectKind;
use phonelink_engine::app::consts::permission;
use phonelink_engine::codec::valueset::{self, ValueSet};
use phonelink_engine::features::photos::{PhotosEvent, capability};
use support::*;

fn ok() -> ValueSet {
    ValueSet::new().with("result", 0i32)
}

fn failure() -> ValueSet {
    ValueSet::new().with("result", 1i32)
}

fn item_failure(reason: &str) -> ValueSet {
    ValueSet::new().with("result", 1i32).with("message", format!("{reason} 80004005"))
}

/// The `sequenceNumbers.media_info` of the next connect.
async fn connect_media_info(world: &World) -> Option<i64> {
    let before = world.h.messenger.requests_on("/connect/v1").len();
    world.h.app.send_connect(ConnectKind::Background, "Test").await;

    let reqs = world.h.messenger.requests_on("/connect/v1");
    assert_eq!(reqs.len(), before + 1);

    let vs = valueset::decode(&reqs.last().unwrap().message.payload).unwrap();
    vs.object("sequenceNumbers").unwrap().i64("media_info")
}

#[tokio::test(start_paused = true)]
async fn the_example_sync() {
    let world = world();
    let t = world.now_ms() - 3_600_000;
    // The PC already holds 3001 with checksum c1.
    seed(&world, &[it(3001, t).checksum(31)]).await;

    let answer =
        world.inbound(media_info(None, metadata(&[3003, 3002, 3001], &[t, t, t], Some(&[33, 32, 31]), Some(42)))).await;
    assert_eq!(
        answer.keys().collect::<Vec<_>>(),
        ["correlationVector", "contentType", "contractVersion", "itemCount", "mediaIds", "mediaRequestedIds", "result"]
    );
    assert_eq!(answer.str("correlationVector"), Some("cv-1"));
    assert_eq!(answer.str("contentType"), Some("mediaInfo"));
    assert_eq!(answer.f64("contractVersion"), Some(3.99));
    assert_eq!(answer.i32("itemCount"), Some(2));
    assert_eq!(answer.i64_array("mediaIds"), Some(&[3003i64, 3002][..]));
    assert_eq!(answer.i64_array("mediaRequestedIds"), Some(&[3003i64, 3002][..]));
    assert_eq!(answer.i32("result"), Some(0));

    // The answer's `result` is also the response header.
    let sent = world.h.messenger.responses().pop().unwrap();
    assert_eq!(sent.header("result").as_deref(), Some("0"));
    assert_eq!(world.photos.status().unwrap().current_sequence, -1, "not complete yet");

    // The item message of the same sync (same correlationVector, no syncID).
    let answer = world.inbound(media_info(None, items(&[it(3003, t).checksum(33), it(3002, t).checksum(32)]))).await;
    assert_eq!(answer, ok());
    let status = world.photos.status().unwrap();
    assert_eq!(status.current_sequence, 42);
    assert_eq!(status.last_sync_ms, Some(world.now_ms()));
    assert_eq!(world.gallery_ids(), [3003, 3002, 3001]);
    assert_eq!(world.photos.thumbnail(3003).unwrap(), Some(vec![0xBB, 1, 2]));

    // The next connect resumes from 42.
    assert_eq!(connect_media_info(&world).await, Some(42));
}

#[tokio::test(start_paused = true)]
async fn nothing_pending_completes_at_once() {
    let world = world();
    seed(&world, &[it(1, 100).checksum(5)]).await;

    let answer = world.inbound(media_info(Some("s"), metadata(&[1], &[100], Some(&[5]), Some(7)))).await;
    assert_eq!(answer.i32("itemCount"), Some(0));
    assert_eq!(answer.i64_array("mediaIds"), Some(&[][..]));
    assert_eq!(world.photos.status().unwrap().current_sequence, 7);

    // No state is kept: an item message under the same key is processed on its own.
    let answer = world.inbound(media_info(Some("s"), items(&[it(2, 100)]).with("sequenceNumber", 9i64))).await;
    assert_eq!(answer, ok());
    assert_eq!(world.photos.status().unwrap().current_sequence, 9, "the item message's own sequence");
}

#[tokio::test(start_paused = true)]
async fn pending_without_checksums_needs_thumbnail_and_same_time() {
    let world = world();
    seed(&world, &[it(1, 100), it(2, 200).no_thumb(), it(3, 300)]).await;

    let answer = world.inbound(media_info(Some("s"), metadata(&[4, 3, 2, 1], &[400, 301, 200, 100], None, None))).await;
    // 4 is new, 3's time changed, 2 has no thumbnail; 1 is unchanged with a thumbnail.
    assert_eq!(answer.i64_array("mediaIds"), Some(&[4i64, 3, 2][..]));
}

#[tokio::test(start_paused = true)]
async fn requested_ids_follow_auto_hydrated_thumbnails_count() {
    let world = world_with(PhotosConfig { auto_hydrated_thumbnails_count: 2, ..Default::default() }, None);
    let answer = world.inbound(media_info(Some("s"), metadata(&[9, 8, 7], &[1, 2, 3], None, None))).await;
    assert_eq!(answer.i64_array("mediaIds"), Some(&[9i64, 8, 7][..]));
    assert_eq!(answer.i64_array("mediaRequestedIds"), Some(&[9i64, 8][..]));
    assert_eq!(answer.i32("itemCount"), Some(3));

    let world = world_with(PhotosConfig { media_optimized_thumbnails: true, ..Default::default() }, None);
    let answer = world.inbound(media_info(Some("s"), metadata(&[9, 8, 7], &[1, 2, 3], None, None))).await;
    assert_eq!(answer.i64_array("mediaRequestedIds"), Some(&[][..]), "media_optimized_thumbnails off: none inline");
}

#[tokio::test(start_paused = true)]
async fn repeated_ids_are_ignored() {
    let world = world();
    let answer = world.inbound(media_info(Some("s"), metadata(&[5, 6, 5, 6, 7], &[1, 2, 3, 4, 5], None, None))).await;
    assert_eq!(answer.i64_array("mediaIds"), Some(&[5i64, 6, 7][..]));
    assert_eq!(answer.i32("itemCount"), Some(3));
}

#[tokio::test(start_paused = true)]
async fn ids_the_phone_no_longer_lists_are_deleted_and_an_empty_list_clears() {
    for checksums in [None, Some(&[][..])] {
        let world = world();
        seed(&world, &[it(1, 1), it(2, 2), it(3, 3)]).await;
        let sums = checksums.map(|_| &[30i64][..]);
        world.inbound(media_info(Some("a"), metadata(&[3], &[3], sums, None))).await;
        assert_eq!(world.gallery_ids(), [3]);

        let mut events = world.events();
        let answer = world.inbound(media_info(Some("b"), metadata(&[], &[], checksums, Some(3)))).await;
        assert_eq!(answer.i32("itemCount"), Some(0));
        assert!(world.gallery_ids().is_empty(), "an empty list clears the cache");
        assert_eq!(world.photos.status().unwrap().current_sequence, 3);
        assert!(drain(&mut events).iter().any(|e| matches!(e, PhotosEvent::GalleryChanged { .. })));
    }
}

#[tokio::test(start_paused = true)]
async fn media_settings_set_capabilities_and_the_delete_permission() {
    let world = world();
    let settings = ValueSet::new().with("mediaCapabilities", 3i32).with("mediaHasDeletePermission", true);
    world.inbound(media_info(Some("s"), metadata(&[], &[], None, None).with("mediaSettings", settings))).await;
    let status = world.photos.status().unwrap();
    assert_eq!(status.capabilities, capability::MEDIA_DELETE | capability::PERMISSION_REQUIRED_MEDIA_DELETE);
    assert!(status.photos_delete_permission);

    // Without `mediaCapabilities` the capabilities read None; a mediaSettings of another type is ignored.
    let settings = ValueSet::new().with("mediaHasDeletePermission", false);
    world.inbound(media_info(Some("s"), metadata(&[], &[], None, None).with("mediaSettings", settings))).await;
    let status = world.photos.status().unwrap();
    assert_eq!(status.capabilities, capability::NONE);
    assert!(!status.photos_delete_permission);

    let settings = ValueSet::new().with("mediaCapabilities", 1i32);
    world.inbound(media_info(Some("s"), metadata(&[], &[], None, None).with("mediaSettings", settings))).await;
    let answer = world.inbound(media_info(Some("s"), metadata(&[], &[], None, None).with("mediaSettings", 5i32))).await;
    assert_eq!(answer.i32("result"), Some(0));
    assert_eq!(world.photos.status().unwrap().capabilities, capability::MEDIA_DELETE);
}

#[tokio::test(start_paused = true)]
async fn malformed_metadata_is_answered_1_and_changes_nothing() {
    let world = world();
    seed(&world, &[it(1, 1), it(2, 2)]).await;
    let settings = ValueSet::new().with("mediaCapabilities", 1i32).with("mediaHasDeletePermission", true);
    let good = metadata(&[2], &[2], None, Some(5)).with("mediaSettings", settings.clone());

    let cases = [
        good.clone().with("mediaIds", vec![2i32]),
        good.clone().with("mediaTimes", Vec::<i64>::new()),
        good.clone().with("checksums", Vec::<i64>::new()),
        good.clone().with("checksums", vec!["x"]),
        good.clone().with("mediaSettings", settings.clone().with("mediaCapabilities", true)),
        good.clone().with("mediaSettings", settings.clone().with("mediaHasDeletePermission", 1i32)),
        {
            let mut v = good.clone();
            v.remove("mediaTimes");
            v
        },
        {
            let mut v = good.clone();
            v.remove("mediaIds");
            v
        },
    ];

    for (i, case) in cases.into_iter().enumerate() {
        let answer = world.inbound(media_info(Some("s"), case)).await;
        assert_eq!(answer, failure(), "case {i}");
    }

    // No correlationVector at all.
    let answer = world.inbound(media_info_without_cv(good)).await;
    assert_eq!(answer, failure());
    assert_eq!(world.gallery_ids(), [2, 1], "nothing deleted");
    let status = world.photos.status().unwrap();
    assert_eq!((status.capabilities, status.photos_delete_permission, status.current_sequence), (0, false, -1));
}

#[tokio::test(start_paused = true)]
async fn removed_items_and_the_never_completing_sync() {
    let world = world();
    seed(&world, &[it(1, 1), it(2, 2)]).await;

    let answer = world.inbound(media_info(Some("s"), metadata(&[1, 2, 3, 4], &[1, 2, 3, 4], None, Some(8)))).await;
    assert_eq!(answer.i64_array("mediaIds"), Some(&[3i64, 4][..]));

    // 3 arrives; 4 is reported removed: it stays pending, so the sync never completes.
    let answer = world.inbound(media_info(Some("s"), items(&[it(3, 3), it(4, 4).removed(), it(1, 1).removed()]))).await;
    assert_eq!(answer, ok());
    assert_eq!(world.gallery_ids(), [3, 2], "1 removed, 3 added");
    assert_eq!(world.photos.status().unwrap().current_sequence, -1);

    let answer = world.inbound(media_info(Some("s"), items(&[it(2, 2)]))).await;
    assert_eq!(answer, ok());
    assert_eq!(world.photos.status().unwrap().current_sequence, -1, "4 is still pending");

    // 4 arrives as an item: complete.
    world.inbound(media_info(Some("s"), items(&[it(4, 4)]))).await;
    assert_eq!(world.photos.status().unwrap().current_sequence, 8);
}

#[tokio::test(start_paused = true)]
async fn a_malformed_item_closes_the_sync_without_completing() {
    let world = world();
    let answer = world.inbound(media_info(Some("s"), metadata(&[1, 2], &[1, 2], None, Some(42)))).await;
    assert_eq!(answer.i32("itemCount"), Some(2));

    let mut bad = items(&[it(1, 1)]);
    bad.insert("mediaNames", Vec::<String>::new());

    let answer = world.inbound(media_info(Some("s"), bad)).await;
    assert_eq!(answer, item_failure("mediaNames shorter than mediaIds"));
    let sent = world.h.messenger.responses().pop().unwrap();
    assert_eq!(sent.header("result").as_deref(), Some("1"));
    assert!(world.gallery_ids().is_empty(), "nothing applied");

    // The sync is closed: the next item message of that key is processed on its own and completes
    // without the metadata's sequence number.
    let answer = world.inbound(media_info(Some("s"), items(&[it(1, 1), it(2, 2)]))).await;
    assert_eq!(answer, ok());
    let status = world.photos.status().unwrap();
    assert_eq!(status.current_sequence, -1);
    assert_eq!(world.gallery_ids(), [2, 1]);
}

#[tokio::test(start_paused = true)]
async fn malformed_item_messages() {
    let world = world();
    let good = items(&[it(1, 1), it(2, 2)]);
    let reasons = [
        (good.clone().with("mediaIds", vec![1i32, 2]), "mediaIds missing or not an Int64Array"),
        (good.clone().with("mediaTakenTimes", vec![1i64]), "mediaTakenTimes shorter than mediaIds"),
        (good.clone().with("mediaActions", vec![0i64, 0]), "mediaActions missing or not an Int32Array"),
        (good.clone().with("mediaHeights", vec!["x", "y"]), "mediaHeights missing or not an Int64Array"),
        (good.clone().with("checksums", vec![1i64]), "checksums shorter than mediaIds"),
        (good.clone().with("mediaThumbnailSizes", vec![3i64, 4]), "thumbnail slice beyond mediaThumbnails"),
    ];

    for (case, reason) in reasons {
        let answer = world.inbound(media_info(Some("x"), case)).await;
        assert_eq!(answer, item_failure(reason));
    }

    // No open sync and no correlationVector.
    let answer = world.inbound(media_info_without_cv(good.clone())).await;
    assert_eq!(answer, item_failure("item message without a correlationVector"));
    assert!(world.gallery_ids().is_empty());

    // Optional arrays of another type count as absent.
    let lenient = good.with("mediaURIs", vec![1i64, 2]).with("checksums", vec![true, false]);
    assert_eq!(world.inbound(media_info(Some("x"), lenient)).await, ok());
    let item = world.photos.item(1).unwrap().unwrap();
    assert_eq!((item.checksum, item.uri.as_str()), (0, ""));
}

#[tokio::test(start_paused = true)]
async fn item_overwrite_rule_last_seen_and_full_bytes_drop() {
    let world = world();
    let t0 = world.now_ms();
    seed(&world, &[it(1, 100).checksum(5)]).await;
    let first = world.photos.item(1).unwrap().unwrap();
    assert_eq!(first.last_seen_ms, Some(t0), "inserted with last-seen now");

    // Full bytes arrive through a fetch.
    let phone = Phone::install(&world);
    store_full_image(&world, &phone, 1, b"FULL").await;

    world.w.clock.advance(std::time::Duration::from_secs(60));
    // Same times and URI, cached thumbnail: no overwrite, but a carried thumbnail with another checksum
    // drops the full bytes.
    let mut same = it(1, 100).checksum(6).thumb(b"zz");
    same.name = "renamed.jpg".into();
    world.inbound(media_info(Some("u"), items(&[same]))).await;
    let item = world.photos.item(1).unwrap().unwrap();
    assert_eq!((item.name.as_str(), item.checksum, item.has_full), ("IMG_1.jpg", 5, false));
    assert_eq!(item.last_seen_ms, Some(t0), "last-seen kept");
    assert_eq!(world.photos.thumbnail(1).unwrap(), Some(vec![1, 1, 2]));

    // A changed time overwrites; an item without a thumbnail then clears the cached one.
    world.inbound(media_info(Some("u"), items(&[it(1, 100).times(100, 101).checksum(7).no_thumb()]))).await;
    let item = world.photos.item(1).unwrap().unwrap();
    assert_eq!((item.modified_ms, item.checksum, item.has_thumbnail), (101, 7, false));
}

#[tokio::test(start_paused = true)]
async fn the_delete_permission_from_settings_is_announced() {
    let world = world();
    let mut events = world.events();
    let settings = ValueSet::new().with("mediaHasDeletePermission", true);

    world.inbound(media_info(Some("s"), metadata(&[], &[], None, None).with("mediaSettings", settings))).await;
    assert!(world.h.app.phone_record().unwrap().permission_or(permission::PHOTOS_DELETE, false));
    assert!(drain(&mut events).iter().any(|e| matches!(e, PhotosEvent::StatusChanged { .. })));
}

#[tokio::test(start_paused = true)]
async fn photos_and_thumbnails_content_types_have_no_processor() {
    let world = world();
    for ct in ["photos", "thumbnails"] {
        let vs =
            ValueSet::new().with("contentType", ct).with("contractVersion", 3.99f64).with("correlationVector", "c");
        let meta = message(
            &[("_requestId", "r"), ("_route", "/legacy/phonecontent")],
            valueset::encode(&vs.clone().with("metadata", true)),
        );
        assert_eq!(world.inbound(meta).await, ValueSet::new().with("result", 2i32));

        let content = message(&[("_requestId", "r"), ("_route", "/legacy/phonecontent")], valueset::encode(&vs));
        assert_eq!(world.inbound(content).await, failure());
    }
}

#[tokio::test(start_paused = true)]
async fn a_minus_one_sequence_number_is_none() {
    let world = world();
    world.inbound(media_info(Some("a"), metadata(&[], &[], None, Some(12)))).await;
    assert_eq!(world.photos.status().unwrap().current_sequence, 12);

    world.inbound(media_info(Some("b"), metadata(&[], &[], None, Some(-1)))).await;
    assert_eq!(world.photos.status().unwrap().current_sequence, 12, "-1 is no sequence number");
}

#[tokio::test(start_paused = true)]
async fn the_sync_survives_nothing_but_the_cache_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let location = phonelink_engine::features::StoreLocation::Directory(dir.path().to_path_buf());
    {
        let world = world_at(PhotosConfig::default(), location.clone(), None);
        world.inbound(media_info(Some("s"), metadata(&[1, 2], &[1, 2], None, Some(4)))).await;
        world.inbound(media_info(Some("s"), items(&[it(1, 1), it(2, 2)]))).await;
        assert_eq!(world.photos.status().unwrap().current_sequence, 4);
        world.inbound(media_info(Some("t"), metadata(&[1, 2, 3], &[1, 2, 3], None, Some(5)))).await;
    }

    let world = world_at(PhotosConfig::default(), location, None);
    assert_eq!(world.gallery_ids(), [2, 1]);
    assert_eq!(world.photos.status().unwrap().current_sequence, 4);

    // The open sync was in memory: after the restart its item message is processed on its own.
    world.inbound(media_info(Some("t"), items(&[it(3, 3)]))).await;
    assert_eq!(world.photos.status().unwrap().current_sequence, 4);
    assert_eq!(world.gallery_ids(), [3, 2, 1]);
}

#[tokio::test(start_paused = true)]
async fn a_header_correlation_vector_keys_the_sync_and_is_echoed() {
    let world = world();
    let headers = [("_requestId", "r"), ("_route", "/legacy/phonecontent"), ("correlationVector", "hdr-cv")];
    let base = ValueSet::new().with("contentType", "mediaInfo").with("contractVersion", 3.99f64);
    let with = |extra: ValueSet| {
        let mut vs = base.clone();
        for (k, v) in extra.iter() {
            vs.insert(k, v.clone());
        }
        message(&headers, valueset::encode(&vs))
    };

    let answer = world.inbound(with(metadata(&[1], &[1], None, Some(3)))).await;
    assert_eq!(answer.str("correlationVector"), Some("hdr-cv"));
    assert_eq!(answer.i32("itemCount"), Some(1));

    // The item message under the same header belongs to that sync and completes it.
    assert_eq!(world.inbound(with(items(&[it(1, 1)]))).await, ok());
    assert_eq!(world.photos.status().unwrap().current_sequence, 3);

    // With no open sync, the header's correlationVector is enough to process an item message alone.
    assert_eq!(world.inbound(with(items(&[it(2, 2)]))).await, ok());
    assert_eq!(world.gallery_ids(), [2, 1]);
}

#[tokio::test(start_paused = true)]
async fn an_item_of_an_open_sync_ignores_its_own_sequence_number() {
    let world = world();
    world.inbound(media_info(Some("s"), metadata(&[1], &[1], None, Some(5)))).await;

    let answer = world.inbound(media_info(Some("s"), items(&[it(1, 1)]).with("sequenceNumber", 99i64))).await;
    assert_eq!(answer, ok());
    assert_eq!(world.photos.status().unwrap().current_sequence, 5, "the metadata's sequence number, not the item's");
}

#[tokio::test(start_paused = true)]
async fn a_repeated_id_keeps_the_first_occurrences_checksum() {
    let world = world();
    seed(&world, &[it(1, 1).checksum(7)]).await;
    let answer = world.inbound(media_info(Some("s"), metadata(&[1, 1], &[1, 1], Some(&[7, 8]), None))).await;
    assert_eq!(answer.i32("itemCount"), Some(0), "7 matches the cache; the repeat's 8 is ignored");
}

#[tokio::test(start_paused = true)]
async fn the_reply_with_nothing_pending_as_sent() {
    let world = world();
    world.inbound(media_info(Some("s"), metadata(&[], &[], None, None))).await;

    let sent = world.h.messenger.responses().pop().unwrap();
    let vs = sent.values();
    assert_eq!(
        vs.keys().collect::<Vec<_>>(),
        ["correlationVector", "contentType", "contractVersion", "itemCount", "mediaIds", "mediaRequestedIds", "result"]
    );

    // Empty arrays keep their type on the wire (the value field is omitted, the type is not).
    use phonelink_engine::codec::valueset::Value;
    assert_eq!(vs.get("mediaIds"), Some(&Value::Int64Array(vec![])));
    assert_eq!(vs.get("mediaRequestedIds"), Some(&Value::Int64Array(vec![])));
    assert_eq!(vs.get("itemCount"), Some(&Value::Int32(0)));
    assert_eq!(sent.header("result").as_deref(), Some("0"));
}

#[tokio::test(start_paused = true)]
async fn a_mistyped_item_sequence_number_is_no_sequence_number() {
    let world = world();
    let answer = world.inbound(media_info(Some("s"), items(&[it(1, 1)]).with("sequenceNumber", 9i32))).await;
    assert_eq!(answer, ok());
    assert_eq!(world.photos.status().unwrap().current_sequence, -1);
}
