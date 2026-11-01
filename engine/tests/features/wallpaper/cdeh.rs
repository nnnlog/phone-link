//! Wallpaper — the Cross Device contract over the platform layer: the phone wallpaper pull (its
//! triggers, its request, the reply rules and the ids of the next pull), the `/wallpaper` receiver
//! answering the Mac's desktop picture, and the tag-5 subscription.

use crate::support::device_status_world;
use crate::support::platform_world;
use crate::support::wallpaper_world as world;
use phonelink_engine::config::features::WallpaperConfig;

use prost::Message;

use device_status_world::phone_item;
use phonelink_engine::features::PeerDirectory;
use phonelink_engine::features::wallpaper::edge::testing::TINY_JPEG;
use phonelink_engine::features::wallpaper::{PcWallpaper, StoredWallpaper, WallpaperCopy, WallpaperEvent};
use phonelink_engine::proto::device_info::v1::{WallpaperResponseMessage, WallpaperType};
use phonelink_engine::types::PartnerDisconnectReason;
use platform_world::{phone_id, settle};
use world::*;

/// The page's pull request with nothing stored (48 bytes).
const PAGE_PULL_VECTOR: [u8; 48] = [
    0x08, 0x05, 0x10, 0x01, 0x1A, 0x1E, 0x0A, 0x0D, 0x08, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01,
    0x10, 0x01, 0x0A, 0x0D, 0x08, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01, 0x10, 0x02, 0x22, 0x0A,
    0x2F, 0x77, 0x61, 0x6C, 0x6C, 0x70, 0x61, 0x70, 0x65, 0x72,
];

async fn reconnect(ww: &WallpaperWorld) {
    ww.sw.w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::LocalHubDisconnect);
    settle().await;
    ww.sw.w.connect_phone().await;
    settle().await;
}

// ------------------------------------------------------------------------------------------------
// The pull
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn the_pull_is_the_page_vector_and_its_reply_is_stored() {
    let ww = WallpaperWorld::android();
    ww.phone_answers_pulls(&ok_reply(vec![
        entry(WallpaperType::Homescreen, 3, Some(b"home")),
        entry(WallpaperType::Lockscreen, 4, Some(b"lock")),
    ]));

    let wp = ww.install();
    let mut rx = wp.subscribe();
    settle().await;
    assert!(ww.pulls().is_empty(), "not available at start");

    ww.sw.w.connect_phone().await;
    settle().await;

    let received = ww.sw.w.phone_drm.received();
    assert_eq!(received.len(), 1, "one pull when the phone became available");
    assert_eq!(received[0].message.payload, PAGE_PULL_VECTOR);
    assert_eq!(received[0].message.headers.get("ms-content-type"), Some("application/x-binary"));
    assert_eq!(stored(&wp, WallpaperCopy::CrossDevice, 0), Some(StoredWallpaper { id: 3, bytes: b"home".to_vec() }));
    assert_eq!(stored(&wp, WallpaperCopy::CrossDevice, 1), Some(StoredWallpaper { id: 4, bytes: b"lock".to_vec() }));
    assert!(events(&mut rx).contains(&WallpaperEvent::Changed { phone: phone_id() }));

    // The next pull carries the stored ids.
    reconnect(&ww).await;

    let p = ww.pull_payload(1);
    let ids: Vec<(i64, i32)> = p.current_wallpapers.iter().map(|m| (m.id, m.wallpaper_type)).collect();
    assert_eq!(ids, vec![(3, 1), (4, 2)]);
}

#[tokio::test(start_paused = true)]
async fn the_reply_rules() {
    let ww = WallpaperWorld::android();
    ww.phone_answers_pulls(&ok_reply(vec![
        entry(WallpaperType::Homescreen, 3, Some(b"home")),
        entry(WallpaperType::Lockscreen, 4, Some(b"lock")),
    ]));

    let wp = ww.install();
    ww.sw.w.connect_phone().await;
    settle().await;

    // An entry without payload keeps its row; one with payload replaces it.
    ww.phone_answers_pulls(&ok_reply(vec![
        entry(WallpaperType::Homescreen, 3, None),
        entry(WallpaperType::Lockscreen, 5, Some(b"lock2")),
    ]));
    reconnect(&ww).await;
    assert_eq!(stored(&wp, WallpaperCopy::CrossDevice, 0).unwrap().id, 3);
    assert_eq!(stored(&wp, WallpaperCopy::CrossDevice, 1), Some(StoredWallpaper { id: 5, bytes: b"lock2".to_vec() }));

    // An entry of another type: nothing changes, even with a status other than OK.
    ww.phone_answers_pulls(&WallpaperResponseMessage {
        wallpapers: vec![entry(WallpaperType::Homescreen, 8, Some(b"x")), entry(WallpaperType::Unspecified, 0, None)],
        status: 2,
    });
    reconnect(&ww).await;
    assert_eq!(stored(&wp, WallpaperCopy::CrossDevice, 0).unwrap().id, 3);

    // A reply that does not decode: nothing changes.
    ww.sw.phone_answers_pulls(vec![0x0A, 0x05]);
    reconnect(&ww).await;
    assert_eq!(stored(&wp, WallpaperCopy::CrossDevice, 1).unwrap().id, 5);

    // NEED_PERMISSIONS: everything goes; the next pull sends -1 twice.
    ww.phone_answers_pulls(&WallpaperResponseMessage { wallpapers: vec![], status: 3 });
    reconnect(&ww).await;
    assert_eq!(stored(&wp, WallpaperCopy::CrossDevice, 0), None);
    assert_eq!(stored(&wp, WallpaperCopy::CrossDevice, 1), None);

    reconnect(&ww).await;
    let last = ww.pulls().len() - 1;
    assert_eq!(ww.pulls()[last].payload, PAGE_PULL_VECTOR[6..36].to_vec());
    assert_eq!(shown(&wp), None);
}

#[tokio::test(start_paused = true)]
async fn a_failed_pull_changes_nothing() {
    let ww = WallpaperWorld::android();
    ww.phone_answers_pulls(&ok_reply(vec![entry(WallpaperType::Homescreen, 3, Some(b"home"))]));
    let wp = ww.install();
    ww.sw.w.connect_phone().await;
    settle().await;

    // The phone has no handler for the resource type.
    ww.sw.w.phone_drm.set_reply(Box::new(|_| Some(platform_world::drm_answer(b"", Some(2)))));
    reconnect(&ww).await;
    assert_eq!(stored(&wp, WallpaperCopy::CrossDevice, 0).unwrap().id, 3);
}

#[tokio::test(start_paused = true)]
async fn only_android_phones_of_the_account_are_pulled_and_the_start_pulls_available_ones() {
    // Already available at start.
    let ww = WallpaperWorld::android();
    ww.phone_answers_pulls(&ok_reply(vec![]));
    ww.sw.w.connect_phone().await;
    let _wp = ww.install();
    settle().await;
    assert_eq!(ww.pulls().len(), 1);

    // An iOS phone: never.
    let mut item = phone_item();
    item.os_name = Some("iOS".into());
    let ww = WallpaperWorld::new(item);
    ww.phone_answers_pulls(&ok_reply(vec![]));
    ww.sw.w.connect_phone().await;
    let _wp = ww.install();
    settle().await;
    reconnect(&ww).await;
    assert!(ww.pulls().is_empty());

    // Not in the account's device list until it is added while available.
    let ww = WallpaperWorld::android();
    ww.phone_answers_pulls(&ok_reply(vec![]));
    let peer = ww.sw.peers.peer(&phone_id()).unwrap();
    ww.sw.peers.remove(&phone_id());

    let _wp = ww.install();
    ww.sw.w.connect_phone().await;
    settle().await;
    assert!(ww.pulls().is_empty());

    ww.sw.peers.upsert(peer.clone());
    settle().await;
    assert_eq!(ww.pulls().len(), 1, "added while available");
    ww.sw.peers.upsert(peer);
    settle().await;
    assert_eq!(ww.pulls().len(), 1, "an update is not an addition");

    // The per-phone switch off: the wrapper sends nothing.
    let ww = WallpaperWorld::android();
    ww.sw.switches.set_enabled(&phone_id(), false).unwrap();
    let _wp = ww.install();
    ww.sw.w.connect_phone().await;
    settle().await;
    assert!(ww.pulls().is_empty());
}

// ------------------------------------------------------------------------------------------------
// The Mac's wallpaper for the phone
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn wallpaper_answers_the_macs_desktop_picture() {
    let ww = WallpaperWorld::android();
    let _wp = ww.install();
    ww.sw.w.connect_phone().await;
    settle().await;

    // Path-addressed, and type-addressed without a path; whatever the payload.
    for (resource_type, path) in [(4, "/wallpaper"), (5, ""), (4, "/wallpaper?x=1")] {
        let answer = ww.phone_asks(resource_type, path).await.expect("answered");
        assert_eq!(answer.status, 1, "OK");
        assert_eq!(answer.wallpapers.len(), 1);
        assert_eq!(answer.wallpapers[0].wallpaper_type, WallpaperType::Homescreen as i32);
        assert_eq!(answer.wallpapers[0].payload.as_deref(), Some(TINY_JPEG.as_slice()));
        assert_eq!(answer.wallpapers[0].id, 0);
    }

    ww.pc.set(PcWallpaper::Unreadable);
    let answer = ww.phone_asks(5, "").await.unwrap();
    assert_eq!(answer.encode_to_vec(), [0x0A, 0x02, 0x10, 0x01]);

    ww.pc.set(PcWallpaper::NotSet);
    let answer = ww.phone_asks(5, "").await.unwrap();
    assert_eq!(answer.encode_to_vec(), [0x0A, 0x00]);
}

#[tokio::test(start_paused = true)]
async fn with_the_option_off_the_phone_gets_no_answer() {
    let ww = WallpaperWorld::android();
    let _wp = ww.install_with(WallpaperConfig { pc_wallpaper_sync_enabled: false });
    ww.sw.w.connect_phone().await;
    settle().await;
    assert_eq!(ww.phone_asks(5, "").await, None);
}

#[tokio::test(start_paused = true)]
async fn tag_5_publications_are_ignored() {
    let ww = WallpaperWorld::android();
    let wp = ww.install();
    let mut rx = wp.subscribe();
    ww.sw.w.connect_phone().await;
    settle().await;

    // The App layer's start connect has been applied by now (its reply re-reads the shown wallpaper).
    events(&mut rx);
    ww.phone_publishes_tag_5().await;
    assert!(events(&mut rx).is_empty());
    assert_eq!(shown(&wp), None);
}
