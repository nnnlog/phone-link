//! The `/media/v1` fetches and their scheduling: the rules common to every request, the thumbnail
//! and full-resolution fetches, request scheduling, and the triggers.

use crate::support::app_world;
use crate::support::common::payload;
use crate::support::photos_world as support;
use phonelink_engine::config::features::PhotosConfig;

use std::time::Duration;

use tokio::time::Instant;

use app_world::*;
use phonelink_engine::codec::valueset::ValueSet;
use phonelink_engine::features::photos::{FileAction, PhotosEvent};
use support::*;

const T: i64 = 1_700_000_000_000;

fn ids(range: std::ops::RangeInclusive<i64>) -> Vec<i64> {
    range.collect()
}

fn is_cv(s: &str) -> bool {
    s.len() == 16 && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

async fn seed_without_thumbnails(world: &World, list: &[i64]) {
    let its: Vec<It> = list.iter().map(|i| it(*i, T + *i).no_thumb()).collect();
    seed(world, &its).await;
}

async fn seed_with_thumbnails(world: &World, list: &[i64]) {
    let its: Vec<It> = list.iter().map(|i| it(*i, T + *i)).collect();
    seed(world, &its).await;
}

// ------------------------------------------------------------------------------------------------
// Thumbnails
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn thumbnails_wait_1000_ms_then_go_in_batches_of_25_one_at_a_time() {
    let world = world();
    seed_without_thumbnails(&world, &ids(1..=30)).await;
    let phone = Phone::install(&world);
    let start = Instant::now();

    for id in 1..=30 {
        world.photos.on_item_visible(id).unwrap();
    }

    tokio::time::sleep(Duration::from_millis(999)).await;
    settle().await;
    assert_eq!(phone.held_count(), 0, "nothing before 1000 ms");

    wait_held(&phone, 1).await;
    assert_eq!(start.elapsed(), Duration::from_millis(1000));
    let first = phone.take();

    // The last trigger is on top.
    assert_eq!(first.ids(), (6..=30).rev().collect::<Vec<_>>());
    assert_eq!(first.content_type(), Some("thumbnails"));
    let cv1 = first.payload.str("correlationVector").unwrap().to_string();
    assert!(is_cv(&cv1));

    // Keys: the feature's, the legacy ones, then the standard keys.
    assert_eq!(
        first.payload.keys().collect::<Vec<_>>(),
        [
            "correlationVector",
            "contentType",
            "isuserinitiated",
            "mediaIds",
            "ignoreResponse",
            "transportType",
            "displayName",
            "appVersion",
            "installationId",
            "contractVersion"
        ]
    );
    assert_eq!(first.payload.bool("isuserinitiated"), Some(false));
    assert_eq!(first.payload.i32("transportType"), Some(0));

    // One request at a time.
    settle().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(phone.held_count(), 0);

    let replies: Vec<It> = first.ids().iter().map(|i| it(*i, T + *i)).collect();
    first.answer(thumbnail_reply(&replies));
    wait_held(&phone, 1).await;
    let second = phone.take();
    assert_eq!(second.ids(), [5, 4, 3, 2, 1]);
    assert_ne!(second.payload.str("correlationVector").unwrap(), cv1, "fresh per request");

    let replies: Vec<It> = second.ids().iter().map(|i| it(*i, T + *i)).collect();
    second.answer(thumbnail_reply(&replies));
    settle().await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(phone.held_count(), 0, "the worker stopped on an empty batch");
    assert!(world.photos.gallery().unwrap().iter().all(|i| i.has_thumbnail));

    // Default options: not user-initiated, priority 100.
    let sent = world.h.messenger.requests_on(MEDIA);
    assert!(sent.iter().all(|r| r.priority == 100 && !r.options.ignore_device_disabled));

    // A later trigger starts a new worker, again after 1000 ms.
    seed_without_thumbnails(&world, &[31]).await;
    let start = Instant::now();
    world.photos.on_item_visible(31).unwrap();
    wait_held(&phone, 1).await;
    assert_eq!(start.elapsed(), Duration::from_millis(1000));
}

#[tokio::test(start_paused = true)]
async fn a_thumbnail_fetch_gives_up_after_four_attempts() {
    let world = world();
    seed_without_thumbnails(&world, &[1]).await;
    let phone = Phone::install(&world);
    world.photos.on_item_visible(1).unwrap();

    for attempt in 1..=4 {
        wait_held(&phone, 1).await;
        let held = phone.take();
        assert_eq!(held.ids(), [1], "attempt {attempt}");
        held.answer(result_response(1));
    }

    settle().await;
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(phone.held_count(), 0, "given up");
    assert_eq!(world.media_of(Some("thumbnails")).len(), 4);
}

#[tokio::test(start_paused = true)]
async fn the_retry_pause_knob_doubles_per_consecutive_failure() {
    let config = PhotosConfig {
        retry_pause_min: Duration::from_secs(3),
        retry_pause_max: Duration::from_secs(60),
        ..Default::default()
    };
    let world = world_with(config, None);
    seed_without_thumbnails(&world, &[1]).await;
    let phone = Phone::install(&world);
    world.photos.on_item_visible(1).unwrap();
    wait_held(&phone, 1).await;

    let sent = Instant::now();
    phone.take().answer(result_response(1));
    wait_held(&phone, 1).await;
    assert_eq!(sent.elapsed(), Duration::from_secs(6), "3 s · 2^1 after the last request");

    let sent = Instant::now();
    phone.take().answer(result_response(1));
    wait_held(&phone, 1).await;
    assert_eq!(sent.elapsed(), Duration::from_secs(12));
}

#[tokio::test(start_paused = true)]
async fn a_stop_all_outcome_clears_the_thumbnail_queue() {
    for code in [7, 9, 12, 13, 21, 23] {
        let world = world();
        seed_without_thumbnails(&world, &ids(1..=30)).await;
        let phone = Phone::install(&world);

        for id in 1..=30 {
            world.photos.on_item_visible(id).unwrap();
        }

        wait_held(&phone, 1).await;
        phone.take().answer(result_response(code));
        settle().await;
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert_eq!(phone.held_count(), 0, "code {code}: no retry, the 5 queued ids are cancelled too");

        // A new trigger requests again.
        world.photos.on_item_visible(3).unwrap();
        wait_held(&phone, 1).await;
        assert_eq!(phone.take().ids(), [3]);
    }
}

#[tokio::test(start_paused = true)]
async fn ids_a_non_empty_reply_omits_stay_in_flight_until_cancelled() {
    let world = world();
    seed_without_thumbnails(&world, &[1, 2]).await;
    let phone = Phone::install(&world);
    world.photos.on_item_visible(1).unwrap();
    world.photos.on_item_visible(2).unwrap();

    wait_held(&phone, 1).await;
    let held = phone.take();
    assert_eq!(held.ids(), [2, 1]);
    held.answer(thumbnail_reply(&[it(2, T)]));
    settle().await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(phone.held_count(), 0);

    // Re-adding only moves it up; it is not requested again.
    world.photos.on_item_visible(1).unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(phone.held_count(), 0);

    // Scrolled out of view: cancelled; a new trigger requests it again.
    world.photos.on_item_hidden(1).unwrap();
    world.photos.on_item_visible(1).unwrap();
    wait_held(&phone, 1).await;
    assert_eq!(phone.take().ids(), [1]);
}

#[tokio::test(start_paused = true)]
async fn not_found_and_items_without_bytes() {
    let world = world();
    seed_without_thumbnails(&world, &[1, 2, 3]).await;
    let phone = Phone::install(&world);

    for id in [1, 2, 3] {
        world.photos.on_item_visible(id).unwrap();
    }

    wait_held(&phone, 1).await;
    let held = phone.take();
    assert_eq!(held.ids(), [3, 2, 1]);

    // 3 without a thumbnail: a failed attempt, re-queued; 2 with one: done; 1 omitted: in flight.
    held.answer(thumbnail_reply(&[it(3, T).no_thumb(), it(2, T)]));
    wait_held(&phone, 1).await;
    let held = phone.take();
    assert_eq!(held.ids(), [3]);

    // A success with no items: not found, no retry.
    held.answer(thumbnail_reply(&[]));
    settle().await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(phone.held_count(), 0);

    // Removed items in a thumbnail reply are ignored.
    world.photos.on_item_hidden(1).unwrap();
    world.photos.on_item_visible(1).unwrap();
    wait_held(&phone, 1).await;
    phone.take().answer(thumbnail_reply(&[it(1, T).removed()]));
    settle().await;
    assert!(world.photos.item(1).unwrap().is_some(), "not deleted by a thumbnail reply");
}

#[tokio::test(start_paused = true)]
async fn a_malformed_thumbnail_reply_is_a_failed_attempt_for_the_batch() {
    let world = world();
    seed_without_thumbnails(&world, &[1, 2]).await;
    let phone = Phone::install(&world);

    world.photos.on_item_visible(1).unwrap();
    world.photos.on_item_visible(2).unwrap();

    wait_held(&phone, 1).await;
    phone.take().answer(response_with(ValueSet::new().with("result", 0i32).with("mediaIds", vec![1i32])));
    wait_held(&phone, 1).await;
    assert_eq!(phone.take().ids(), [2, 1], "both re-queued");
}

#[tokio::test(start_paused = true)]
async fn queued_ids_whose_data_arrived_meanwhile_are_dropped() {
    let world = world();
    seed_without_thumbnails(&world, &[1]).await;
    let phone = Phone::install(&world);
    world.photos.on_item_visible(1).unwrap();

    // The thumbnail arrives with an item message before the worker's first batch.
    seed(&world, &[it(1, T + 1).times(T + 5, T + 5)]).await;
    settle().await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(phone.held_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn visible_items_with_an_image_are_not_fetched() {
    let world = world();
    seed_with_thumbnails(&world, &[1]).await;
    let phone = Phone::install(&world);

    world.photos.on_item_visible(1).unwrap();
    world.photos.on_item_visible(99).unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(phone.held_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn optimized_thumbnails_send_one_batch_per_grid_layout() {
    let world = world_with(PhotosConfig { media_optimized_thumbnails: true, ..Default::default() }, None);
    seed_without_thumbnails(&world, &[1, 2, 4]).await;
    seed_with_thumbnails(&world, &[3]).await;
    let phone = Phone::install(&world);

    world.photos.on_item_visible(1).unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(phone.held_count(), 0, "no per-item lazy thumbnails");

    world.photos.on_grid_layout(&[1, 2, 3, 4]).unwrap();
    wait_held(&phone, 1).await;
    assert_eq!(phone.take().ids(), [1, 2, 4]);
}

// ------------------------------------------------------------------------------------------------
// Full resolution
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn full_resolution_five_in_flight_one_id_each_and_retries_reuse_the_correlation_vector() {
    let world = world();
    seed_with_thumbnails(&world, &ids(1..=7)).await;
    let phone = Phone::install(&world);
    let start = Instant::now();

    for id in 1..=7 {
        assert!(world.photos.open_item(id).unwrap(), "an item with a thumbnail opens the viewer");
    }

    wait_held(&phone, 5).await;
    assert_eq!(start.elapsed(), Duration::ZERO, "no start delay");
    settle().await;
    assert_eq!(phone.held_count(), 5, "at most 5 in flight");
    let mut held: Vec<Held> = (0..5).map(|_| phone.take()).collect();
    for h in &held {
        assert_eq!(h.content_type(), Some("photos"));
        assert_eq!(h.ids().len(), 1);
    }

    // The last trigger first.
    assert_eq!(held.iter().map(|h| h.ids()[0]).collect::<Vec<_>>(), [7, 6, 5, 4, 3]);
    let failing = held.remove(0);
    let cv = failing.payload.str("correlationVector").unwrap().to_string();
    failing.answer(result_response(1));

    // The failed id is queued again in its place on top of the stack, so the freed slot retries it,
    // with the same correlation vector.
    wait_held(&phone, 1).await;
    let retried = phone.take();
    assert_eq!(retried.ids(), [7]);
    assert_eq!(retried.payload.str("correlationVector"), Some(cv.as_str()));
    retried.answer(full_reply(&[(7, b"seven")]));

    wait_held(&phone, 1).await;
    let sixth = phone.take();
    assert_eq!(sixth.ids(), [2]);
    sixth.answer(full_reply(&[(2, b"two")]));

    wait_held(&phone, 1).await;
    assert_eq!(phone.take().ids(), [1]);
    assert_eq!(world.photos.full_image(7).unwrap(), Some(b"seven".to_vec()));
    assert_eq!(world.photos.full_image(2).unwrap(), Some(b"two".to_vec()));

    for h in held {
        h.answer(full_reply(&[]));
    }
}

#[tokio::test(start_paused = true)]
async fn full_resolution_reply_handling() {
    let world = world();
    seed_with_thumbnails(&world, &[1, 2]).await;
    let phone = Phone::install(&world);
    let mut events = world.events();
    world.photos.open_item(1).unwrap();
    wait_held(&phone, 1).await;

    // An empty slice is a failed attempt.
    phone.take().answer(full_reply(&[(1, b"")]));
    wait_held(&phone, 1).await;

    // A reply with no items: not found.
    phone.take().answer(full_reply(&[]));
    settle().await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(phone.held_count(), 0);
    assert_eq!(world.photos.full_image(1).unwrap(), None);

    // The context menu fetches again; a reply for another id stores that id's bytes and leaves 1 in
    // flight, holding its slot until cancelled.
    world.photos.on_context_menu_opened(1).unwrap();
    wait_held(&phone, 1).await;
    phone.take().answer(full_reply(&[(2, b"two")]));
    settle().await;
    assert_eq!(world.photos.full_image(2).unwrap(), Some(b"two".to_vec()));
    assert!(drain(&mut events).iter().any(|e| matches!(e, PhotosEvent::FullImageStored { id: 2, .. })));

    world.photos.on_context_menu_opened(1).unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(phone.held_count(), 0, "1 is still in flight");

    world.photos.on_viewer_closed().unwrap();
    world.photos.on_context_menu_opened(1).unwrap();
    wait_held(&phone, 1).await;

    // A malformed reply is a failed attempt.
    phone.take().answer(response_with(ValueSet::new().with("result", 0i32).with("mediaIds", vec![1i64])));
    wait_held(&phone, 1).await;
    phone.take().answer(full_reply(&[(1, b"one")]));
    settle().await;
    assert_eq!(world.photos.full_image(1).unwrap(), Some(b"one".to_vec()));
    assert!(world.photos.item(1).unwrap().unwrap().has_full);
}

#[tokio::test(start_paused = true)]
async fn a_stop_all_outcome_clears_the_full_resolution_queue() {
    let world = world();
    seed_with_thumbnails(&world, &ids(1..=7)).await;
    let phone = Phone::install(&world);

    for id in 1..=7 {
        world.photos.open_item(id).unwrap();
    }

    wait_held(&phone, 5).await;
    let first = phone.take();
    first.answer(result_response(13));

    settle().await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(phone.held_count(), 4, "the queued ids 2 and 1 are not sent");
}

#[tokio::test(start_paused = true)]
async fn the_viewer_selection_fetches_the_item_and_two_neighbours_each_side() {
    let world = world();
    // Display order: taken DESC → 7, 6, 5, 4, 3, 2, 1.
    seed_with_thumbnails(&world, &ids(1..=7)).await;
    let phone = Phone::install(&world);
    // Give 5 full bytes so the window skips it.
    store_full_image(&world, &phone, 5, b"five").await;

    world.photos.on_viewer_selection(4).unwrap();
    wait_held(&phone, 4).await;
    let order: Vec<i64> = (0..4).map(|_| phone.take().ids()[0]).collect();
    // Selected (4), next (3), previous (5: has full bytes, skipped), second next (2), second previous (6).
    assert_eq!(order, [4, 3, 2, 6]);

    // At the start of the list there is no previous item.
    world.photos.on_viewer_selection(7).unwrap();
    settle().await;
    // 7 and 6 are already queued or in flight: 6 moves up, 7 is new; 5 has bytes.
    wait_held(&phone, 1).await;
    assert_eq!(phone.take().ids(), [7]);
}

#[tokio::test(start_paused = true)]
async fn closing_the_viewer_cancels_the_galleries_full_resolution_fetches() {
    let world = world();
    seed_with_thumbnails(&world, &ids(1..=7)).await;
    let phone = Phone::install(&world);

    for id in 1..=7 {
        world.photos.open_item(id).unwrap();
    }

    wait_held(&phone, 5).await;
    world.photos.on_viewer_closed().unwrap();

    // The in-flight requests complete and their replies are applied; the queued 2 and 1 are not sent.
    let held: Vec<Held> = (0..5).map(|_| phone.take()).collect();
    for h in held {
        let id = h.ids()[0];
        h.answer(full_reply(&[(id, b"x")]));
    }

    settle().await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(phone.held_count(), 0);
    assert!(world.photos.item(7).unwrap().unwrap().has_full);
    assert!(!world.photos.item(1).unwrap().unwrap().has_full);
}

#[tokio::test(start_paused = true)]
async fn leaving_the_gallery_and_scrolling_out_cancel_queued_fetches() {
    let world = world();
    seed_without_thumbnails(&world, &[1, 2]).await;
    let phone = Phone::install(&world);

    world.photos.on_item_visible(1).unwrap();
    world.photos.on_item_visible(2).unwrap();
    world.photos.on_item_hidden(2).unwrap();
    wait_held(&phone, 1).await;
    assert_eq!(phone.take().ids(), [1], "2 scrolled out before the batch");

    world.photos.on_item_visible(2).unwrap();
    world.photos.on_gallery_left().unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(phone.held_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn grid_click_rules() {
    let world = world();
    seed_without_thumbnails(&world, &[1]).await;
    seed_with_thumbnails(&world, &[2]).await;
    let phone = Phone::install(&world);

    assert!(!world.photos.open_item(1).unwrap(), "no image shown: not actionable");
    assert!(world.photos.open_item(99).is_err());
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(phone.held_count(), 0, "nothing fetched");

    assert!(world.photos.open_item(2).unwrap());
    wait_held(&phone, 1).await;
    assert_eq!(phone.take().ids(), [2]);
}

#[tokio::test(start_paused = true)]
async fn file_actions_wait_at_most_30_seconds_and_late_replies_still_apply() {
    let world = world();
    seed_with_thumbnails(&world, &[1, 2]).await;
    let phone = Phone::install(&world);

    let photos = world.photos.clone();
    let start = Instant::now();
    let waiting = tokio::spawn(async move { photos.full_image_for_action(1, FileAction::SaveAs).await });
    wait_held(&phone, 1).await;
    let late = phone.take();
    assert_eq!(waiting.await.unwrap().unwrap(), None);
    assert_eq!(start.elapsed(), Duration::from_secs(30));

    late.answer(full_reply(&[(1, b"late")]));
    settle().await;
    assert_eq!(world.photos.full_image(1).unwrap(), Some(b"late".to_vec()));

    // Cached bytes come back at once, without a request.
    assert_eq!(world.photos.full_image_for_action(1, FileAction::Copy).await.unwrap(), Some(b"late".to_vec()));

    // Bytes that arrive within the wait are returned.
    let photos = world.photos.clone();
    let waiting = tokio::spawn(async move { photos.full_image_for_action(2, FileAction::Share).await });
    wait_held(&phone, 1).await;
    phone.take().answer(full_reply(&[(2, b"two")]));
    assert_eq!(waiting.await.unwrap().unwrap(), Some(b"two".to_vec()));
    assert!(world.photos.full_image_for_action(42, FileAction::Open).await.is_err(), "not cached");
}

#[tokio::test(start_paused = true)]
async fn fetches_are_failed_attempts_while_the_session_is_locked() {
    let world = world();
    seed_without_thumbnails(&world, &[1]).await;
    let phone = Phone::install(&world);

    world.h.lock.set(phonelink_engine::edge::LockState::Locked);
    world.photos.on_item_visible(1).unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(world.media_requests().is_empty(), "the local gate sends nothing");

    // Unlocked again, the item is not retried: its four attempts were used.
    world.h.lock.set(phonelink_engine::edge::LockState::Unlocked);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(world.media_requests().is_empty());

    world.photos.on_item_visible(1).unwrap();
    wait_held(&phone, 1).await;
    assert_eq!(phone.take().ids(), [1], "a new trigger fetches it");
}

#[tokio::test(start_paused = true)]
async fn fetches_never_go_to_a_phone_that_is_no_longer_selected() {
    let world = world();
    seed_without_thumbnails(&world, &[1]).await;
    seed_with_thumbnails(&world, &[2]).await;
    let phone = Phone::install(&world);
    world.photos.on_item_visible(1).unwrap();

    // Another phone is selected before the thumbnail worker's first batch.
    let other = phonelink_engine::types::DcgClient::new("1f8fad5b-d9cb-469f-a165-70867728950f", phone_env());
    let mut item = device_item(Some(ASYNC_TRUST_CUSTOM_DATA));
    item.id = other.id.clone();
    world.h.app.set_phone(other, &item).unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(phone.held_count(), 0, "no photos request reaches the newly selected phone");

    // The new phone's gallery is its own (empty) store.
    assert!(world.gallery_ids().is_empty());
}

fn phone_env() -> phonelink_engine::types::Environment {
    phone().env
}

#[tokio::test(start_paused = true)]
async fn ltw_or_metered_disabled_sends_and_connections_stop_the_queues() {
    use phonelink_engine::proto::platform_sdk::v1::PlatformDisconnectRequestReason;
    use phonelink_engine::seams::{ConnectionFailure, PlatformRequestStatus};
    use phonelink_engine::types::{DcgSendStatus, WakeStatus};

    // A send that failed with LTW disabled: one thumbnail request, then the queue is cleared.
    let ltw = world();
    seed_without_thumbnails(&ltw, &[1, 2, 3]).await;
    ltw.h.messenger.answer_all(status_response(PlatformRequestStatus::RequestSendFailed(DcgSendStatus::WakeFailed {
        status: WakeStatus::LtwDisabled,
        cause: None,
    })));

    for id in 1..=3 {
        ltw.photos.on_item_visible(id).unwrap();
    }

    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(ltw.media_of(Some("thumbnails")).len(), 1);

    // A connection the phone ended with METERED_SYNC_DISABLED: one attempt, no retry.
    let metered = world();
    seed_with_thumbnails(&metered, &[1]).await;
    metered.h.sessions.set_connected(false);
    metered.h.sessions.set_behavior(ConnectBehavior::Fail(ConnectionFailure::SessionEnded {
        reason: PlatformDisconnectRequestReason::MeteredSyncDisabled as i32,
    }));

    metered.photos.open_item(1).unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(metered.h.sessions.connects().len(), 1, "the full-resolution fetch is not retried");

    // Any other ended session is an ordinary failed attempt: 4 attempts.
    let other = world();
    seed_with_thumbnails(&other, &[1]).await;
    other.h.sessions.set_connected(false);
    other.h.sessions.set_behavior(ConnectBehavior::Fail(ConnectionFailure::SessionEnded {
        reason: PlatformDisconnectRequestReason::UserRequested as i32,
    }));

    other.photos.open_item(1).unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(other.h.sessions.connects().len(), 4);
}

// ------------------------------------------------------------------------------------------------
// Trace context names
// ------------------------------------------------------------------------------------------------

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
async fn thumbnail_batches_carry_the_trace_names_of_the_trigger_that_started_the_worker() {
    // A per-item trigger starts the worker: ThumnailRequested (sic) / HydrateThumbnails.
    let world = world();
    seed_without_thumbnails(&world, &[1]).await;
    let phone = Phone::install(&world);
    world.photos.on_item_visible(1).unwrap();
    wait_held(&phone, 1).await;

    let sent = world.h.messenger.requests_on(MEDIA);
    assert_eq!(names(&sent[0]), pair("ThumnailRequested", "HydrateThumbnails"));

    // A grid layout pass with `media_optimized_thumbnails`: MediaViewedInNode / HydrateThumbnails.
    let world = world_with(PhotosConfig { media_optimized_thumbnails: true, ..Default::default() }, None);
    seed_without_thumbnails(&world, &[1, 2]).await;
    let phone = Phone::install(&world);
    world.photos.on_grid_layout(&[1, 2]).unwrap();
    wait_held(&phone, 1).await;

    let sent = world.h.messenger.requests_on(MEDIA);
    assert_eq!(names(&sent[0]), pair("MediaViewedInNode", "HydrateThumbnails"));
}

/// Answers every held and queued full-resolution request until none is left.
async fn drain_full(phone: &Phone) {
    loop {
        settle().await;
        tokio::time::sleep(Duration::from_millis(1)).await;

        if phone.held_count() == 0 {
            return;
        }

        while phone.held_count() > 0 {
            let held = phone.take();
            let id = held.ids()[0];
            held.answer(full_reply(&[(id, b"x")]));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn full_resolution_fetches_carry_the_trace_names_of_their_trigger() {
    let world = world();
    // Display order 20 (top) … 1; every item has a thumbnail and no full bytes.
    seed_with_thumbnails(&world, &ids(1..=20)).await;
    let phone = Phone::install(&world);
    let by_id = |id: i64| {
        let sent = world.h.messenger.requests_on(MEDIA);
        let r = sent.iter().find(|r| payload(r).i64_array("mediaIds") == Some(&[id][..])).cloned();
        names(&r.unwrap_or_else(|| panic!("no request for {id}")))
    };

    // Grid click and context menu: ImageRequested / HydratePhoto. The five slots fill.
    world.photos.open_item(20).unwrap();
    world.photos.on_context_menu_opened(19).unwrap();
    world.photos.open_item(18).unwrap();
    world.photos.open_item(17).unwrap();
    world.photos.open_item(16).unwrap();
    wait_held(&phone, 5).await;

    for id in [20, 19, 18] {
        assert_eq!(by_id(id), pair("ImageRequested", "HydratePhoto"), "{id}");
    }

    // The viewer's selection (window 12, 11, 13, 10, 14) waits for a slot; a grid click that re-adds 11
    // moves it to the top and keeps the viewer's name: MediaViewedInGallery / HydratePhoto.
    world.photos.on_viewer_selection(12).unwrap();
    world.photos.open_item(11).unwrap();

    let first = phone.take();
    let freed = first.ids()[0];
    first.answer(full_reply(&[(freed, b"x")]));

    wait_held(&phone, 5).await;
    assert_eq!(by_id(11), pair("MediaViewedInGallery", "HydratePhoto"));
    drain_full(&phone).await;
    assert_eq!(by_id(12), pair("MediaViewedInGallery", "HydratePhoto"));

    // The file actions: OnCopy, OnSaveAs, OnShare, OnOpen (open and open-with), OnDrag / HydratePhoto.
    let actions = [
        (FileAction::Copy, 9, "OnCopy"),
        (FileAction::SaveAs, 8, "OnSaveAs"),
        (FileAction::Share, 7, "OnShare"),
        (FileAction::Open, 6, "OnOpen"),
        (FileAction::OpenWith, 5, "OnOpen"),
        (FileAction::DragOut, 4, "OnDrag"),
    ];

    let mut waits = Vec::new();
    for (action, id, _) in actions {
        let photos = world.photos.clone();
        waits.push(tokio::spawn(async move { photos.full_image_for_action(id, action).await }));
    }

    drain_full(&phone).await;

    for (_, id, trigger) in actions {
        assert_eq!(by_id(id), pair(trigger, "HydratePhoto"), "{trigger}");
    }

    for w in waits {
        assert!(w.await.unwrap().unwrap().is_some());
    }
}

#[tokio::test(start_paused = true)]
async fn fetches_and_the_media_sync_use_the_default_options() {
    let world = world();
    seed(&world, &[it(1, T + 1).no_thumb(), it(2, T + 2)]).await;
    let phone = Phone::install(&world);

    world.photos.on_item_visible(1).unwrap();
    world.photos.open_item(2).unwrap();
    wait_held(&phone, 1).await;
    world.photos.request_photo_access().await.unwrap();
    wait_held(&phone, 3).await;

    let sent = world.h.messenger.requests_on(MEDIA);
    let kinds: Vec<Option<String>> = sent.iter().map(|r| payload(r).str("contentType").map(str::to_string)).collect();
    assert_eq!(kinds.len(), 3, "{kinds:?}");
    for r in &sent {
        // Not user-initiated (priority 100, no IgnoreDeviceDisabled), the default 120 s timeout.
        assert_eq!((r.timeout, r.priority, r.options.ignore_device_disabled), (Duration::from_secs(120), 100, false));
        assert_eq!(payload(r).bool("isuserinitiated"), Some(false));
    }
}

#[tokio::test(start_paused = true)]
async fn each_queued_full_resolution_fetch_has_its_own_correlation_vector() {
    let world = world();
    seed_with_thumbnails(&world, &[1, 2]).await;
    let phone = Phone::install(&world);

    world.photos.open_item(1).unwrap();
    world.photos.open_item(2).unwrap();
    wait_held(&phone, 2).await;

    let (a, b) = (phone.take(), phone.take());
    assert!(is_cv(a.payload.str("correlationVector").unwrap()));
    assert_ne!(a.payload.str("correlationVector"), b.payload.str("correlationVector"));
}

#[tokio::test(start_paused = true)]
async fn an_accepted_reply_to_a_fetch_is_a_failed_attempt() {
    let world = world();
    seed_with_thumbnails(&world, &[1]).await;
    let phone = Phone::install(&world);

    world.photos.open_item(1).unwrap();
    for _ in 0..4 {
        wait_held(&phone, 1).await;
        phone.take().answer(result_response(4));
    }

    settle().await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(phone.held_count(), 0, "given up after 4 attempts");
    assert_eq!(world.media_of(Some("photos")).len(), 4);
}
