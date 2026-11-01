//! PC → phone announcements over the platform world — the change announcement (outbound tag 9):
//! change detection (the 250 ms change-count poll), the 200 ms trailing debounce, echo suppression,
//! the gates, the classification, the content hash and its dedupe, and the announcement message.

use crate::support::clipboard_world as clip;
use crate::support::platform_world as fakes;
use phonelink_engine::config::features::ClipboardConfig;

use std::time::Duration;

use clip::*;
use fakes::{Behavior, pc_id, phone_id, settle};
use phonelink_engine::auth::PeerDevice;
use phonelink_engine::edge::LockState;
use phonelink_engine::features::StoreLocation;
use phonelink_engine::features::clipboard::image::standins::{FakeCodec, FakeFormat};
use phonelink_engine::features::clipboard::pasteboard::uti;
use phonelink_engine::features::clipboard::{ClipKind, ClipboardEvent};
use phonelink_engine::platform::route;
use phonelink_engine::proto::clipboard::v1 as cb;
use phonelink_engine::types::{DcgClient, DcgSendStatus, Environment, PartnerDisconnectReason, SendOptions};

/// A world with the toggle on: the phone is supported, so copies are announced.
async fn supported() -> ClipWorld {
    let w = ClipWorld::new().await;
    w.toggle_on().await;
    w
}

fn announced_kinds(w: &mut ClipWorld) -> Vec<ClipKind> {
    w.take_events()
        .into_iter()
        .filter_map(|e| match e {
            ClipboardEvent::Announced { kind } => Some(kind),
            _ => None,
        })
        .collect()
}

fn is_lower_guid(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok() && s.len() == 36 && s == s.to_lowercase()
}

#[tokio::test(start_paused = true)]
async fn a_copy_is_announced_to_the_phone() {
    let mut w = supported().await;
    w.take_events();
    w.pasteboard.copy_text("hello");
    after_copy().await;

    let announcements = w.announcements();
    assert_eq!(announcements.len(), 1);
    let a = &announcements[0];
    assert_eq!(a.msaep.message_tag, 9);
    assert_eq!(a.msaep.dcg_client_id, pc_id().id, "the PC's own dcgClientId");
    assert!(is_lower_guid(&a.msaep.message_id));
    assert_eq!((a.msaep.platform_major_version, a.msaep.platform_minor_version), (1, 1));
    assert!(a.payload.additional.is_empty());
    assert_eq!(a.message.status, cb::ClipboardResponseStatus::ClipboardChange as i32);
    assert!(is_lower_guid(&a.message.correlation_id), "a fresh GUID: {}", a.message.correlation_id);
    assert!(a.message.clipboard_item.is_empty(), "the content is not announced");
    assert_eq!(a.message.error_type, 0);

    // Byte for byte: `PubSubPayload { data }` only, `ClipboardResponseMessage { status, correlation_id }`.
    let cid = a.message.correlation_id.as_bytes();
    assert_eq!(a.msaep.payload, [&[0x0a, 0x28, 0x10, 0x06, 0x1a, 0x24][..], cid].concat());

    let sent = w.w.pair.pc_link.sent_on(route::CONTEXT_PUBLISH).pop().unwrap();
    assert_eq!(sent.message.headers.get("ms-content-type"), Some("application/x-binary"));
    assert_eq!(sent.options, SendOptions { one_way: true, skip_wake_if_not_connected: true, ..SendOptions::NONE });

    assert_eq!(announced_kinds(&mut w), vec![ClipKind::Text]);

    // Every announcement has its own ids.
    w.pasteboard.copy_text("again");
    after_copy().await;
    let announcements = w.announcements();
    assert_eq!(announcements.len(), 2);
    assert_ne!(announcements[0].message.correlation_id, announcements[1].message.correlation_id);
    assert_ne!(announcements[0].msaep.message_id, announcements[1].msaep.message_id);
}

#[tokio::test(start_paused = true)]
async fn changes_are_debounced_and_polling_reads_no_content() {
    let config = ClipboardConfig {
        poll_interval: Duration::from_millis(50),
        debounce: Duration::from_millis(200),
        ..Default::default()
    };
    let w = ClipWorld::with(Setup { config, ..Setup::default() }).await;
    w.toggle_on().await;

    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(w.pasteboard.data_reads(), 0, "polling the change count reads no content");

    // Three copies 100 ms apart: each re-arms the debounce.
    for text in ["a", "b", "c"] {
        w.pasteboard.copy_text(text);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // The last change was seen at most 50 ms after it; the debounce has not fired 100 ms later.
    assert!(w.announcements().is_empty());
    tokio::time::sleep(Duration::from_millis(200)).await;
    settle().await;
    assert_eq!(w.announcements().len(), 1, "one announcement once 200 ms passed without a change");

    // A single change is announced 200 ms after it is seen, not before.
    w.pasteboard.copy_text("d");
    tokio::time::sleep(Duration::from_millis(60)).await; // seen by now
    tokio::time::sleep(Duration::from_millis(130)).await;
    assert_eq!(w.announcements().len(), 1);
    tokio::time::sleep(Duration::from_millis(80)).await;
    settle().await;
    assert_eq!(w.announcements().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn content_present_at_startup_is_not_a_change() {
    let pasteboard = phonelink_engine::features::clipboard::pasteboard::standins::MemoryPasteboard::new();
    pasteboard.copy_text("already there");
    let w = ClipWorld::with(Setup { pasteboard: Some(pasteboard), ..Setup::default() }).await;
    w.toggle_on().await;

    after_copy().await;
    assert!(w.announcements().is_empty(), "the first change count read is the baseline");

    w.pasteboard.copy_text("copied after the start");
    after_copy().await;
    assert_eq!(w.announcements().len(), 1);

    // A change of ownership without recognized content announces nothing.
    w.pasteboard.clear();
    after_copy().await;
    assert_eq!(w.announcements().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn the_ports_own_write_is_not_a_change() {
    let w = ClipWorld::with(Setup {
        drm: PhoneDrm::Answer(Some(content(item(cb::ClipboardItemType::TextPlain, Some("from the phone"), None)))),
        ..Setup::default()
    })
    .await;
    w.toggle_on().await;

    phone_announces(&w.w, "m-1", CID).await;
    assert_eq!(w.pasteboard.writes().len(), 1);

    after_copy().await;
    assert!(w.announcements().is_empty(), "the change count of the engine's write is ignored");

    // The app writes, then the user copies before the next poll: the user's count is a change.
    phone_announces(&w.w, "m-2", CID).await;
    w.pasteboard.copy_text("user");
    after_copy().await;
    assert_eq!(w.announcements().len(), 1);

    // The user copies, then the app writes before the next poll: the latest count is the app's
    // own write, and the pasteboard holds the phone's clip — nothing is announced.
    w.pasteboard.copy_text("user again");
    phone_announces(&w.w, "m-3", CID).await;
    after_copy().await;
    assert_eq!(w.announcements().len(), 1);
    assert_eq!(w.pasteboard.writes().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn the_last_announced_hash_dedupes() {
    let w = ClipWorld::with(Setup {
        drm: PhoneDrm::Answer(Some(content(item(cb::ClipboardItemType::TextPlain, Some("phone"), None)))),
        ..Setup::default()
    })
    .await;
    w.toggle_on().await;

    for (text, announced) in [("a", 1), ("a", 1), ("b", 2), ("a", 3)] {
        w.pasteboard.copy_text(text);
        after_copy().await;
        assert_eq!(w.announcements().len(), announced, "after copying {text:?}");
    }

    // The phone's clip written in between does not update the hash: re-copying "a" is skipped.
    phone_announces(&w.w, "m-1", CID).await;
    w.pasteboard.copy_text("a");
    after_copy().await;
    assert_eq!(w.announcements().len(), 3);

    // HTML is hashed by its plain text: same text, different HTML is a duplicate.
    w.pasteboard.copy(&[(uti::HTML, b"<b>a</b>"), (uti::PLAIN_TEXT, b"a")]);
    after_copy().await;
    assert_eq!(w.announcements().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn the_hash_is_stored_before_sending_whatever_the_result() {
    let w = supported().await;
    w.w.pair.pc_link.script(Behavior::Fail(DcgSendStatus::TimedOut));

    w.pasteboard.copy_text("lost");
    after_copy().await;
    assert_eq!(w.w.pair.pc_link.sent_on(route::CONTEXT_PUBLISH).len(), 1, "sent once, failed");

    w.pasteboard.copy_text("lost");
    after_copy().await;
    tokio::time::sleep(Duration::from_secs(300)).await;
    assert_eq!(w.w.pair.pc_link.sent_on(route::CONTEXT_PUBLISH).len(), 1, "no retry, and the hash was stored");
}

#[tokio::test(start_paused = true)]
async fn the_gates_stop_announcing() {
    let w = ClipWorld::new().await;

    // The toggle is off: no phone is supported.
    w.pasteboard.copy_text("1");
    after_copy().await;
    assert!(w.announcements().is_empty());

    w.toggle_on().await;
    w.clipboard.set_global_switch(false).unwrap();
    w.pasteboard.copy_text("2");
    after_copy().await;
    w.clipboard.set_global_switch(true).unwrap();

    w.switches.set_enabled(&phone_id(), false).unwrap();
    w.pasteboard.copy_text("3");
    after_copy().await;
    w.switches.set_enabled(&phone_id(), true).unwrap();

    let mut device = phone_device();
    device.capabilities = Some(vec![]);
    w.peers.upsert(phone_peer(device));
    w.pasteboard.copy_text("4");
    after_copy().await;
    w.peers.upsert(phone_peer(phone_device()));

    w.peers.set_enrolled(false);
    w.pasteboard.copy_text("5");
    after_copy().await;
    w.peers.set_enrolled(true);

    w.sessions.override_available(Some(false));
    w.pasteboard.copy_text("6");
    after_copy().await;
    w.sessions.override_available(None);
    assert!(w.announcements().is_empty(), "no gate let anything through");

    // A gated change stored no hash: the same content is announced once the gates open.
    w.pasteboard.copy_text("6");
    after_copy().await;
    assert_eq!(w.announcements().len(), 1);
}

/// Gate matrix, PC-announcement column: the PC's lock state and the phone's app version are not
/// checked.
#[tokio::test(start_paused = true)]
async fn the_announcement_ignores_the_lock_and_the_phone_version() {
    let w = supported().await;
    w.w.lock.set(LockState::Locked);
    let mut device = phone_device();
    device.client_version = Some("1.0.0.1".into());
    w.peers.upsert(phone_peer(device));
    settle().await;

    w.pasteboard.copy_text("locked, old phone app");
    after_copy().await;
    assert_eq!(w.announcements().len(), 1);
}

/// The announcement message: every connected phone with a context source gets it, whatever its own
/// toggle; one message, the same ids for every phone.
#[tokio::test(start_paused = true)]
async fn every_connected_phone_gets_the_announcement_whatever_its_toggle() {
    let w = supported().await;
    let second = DcgClient::new("phone-33333333", Environment::Prod);
    let mut device = phone_device();
    device.id = second.id.clone();
    w.peers.upsert(PeerDevice { client: second.clone(), info: device });
    w.w.hub.partner_joins(&second);
    settle().await;

    assert!(!w.clipboard.phone_toggle(&second).unwrap(), "the second phone's toggle is off");

    w.pasteboard.copy_text("for every phone");
    after_copy().await;

    let sent = w.w.pair.pc_link.sent_on(route::CONTEXT_PUBLISH);
    let mut targets: Vec<DcgClient> = sent.iter().map(|m| m.target.clone()).collect();
    targets.sort();
    let mut expected = vec![phone_id(), second];
    expected.sort();
    assert_eq!(targets, expected);
    assert_eq!(sent[0].message.payload, sent[1].message.payload, "one publication");
}

/// *Any phone supported* counts Android phones only.
#[tokio::test(start_paused = true)]
async fn a_peer_that_is_not_an_android_phone_is_never_supported() {
    let mut device = phone_device();
    device.os_name = Some("Windows".into());
    let w = ClipWorld::with(Setup { device, ..Setup::default() }).await;
    w.toggle_on().await;
    assert!(w.clipboard.phones().unwrap().is_empty());

    w.pasteboard.copy_text("nobody supported");
    after_copy().await;
    assert!(w.announcements().is_empty());
}

/// The last announced hash lives in memory only: a new start announces the same content again.
#[tokio::test(start_paused = true)]
async fn the_last_announced_hash_does_not_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let stores = StoreLocation::Directory(dir.path().to_path_buf());

    {
        let w = ClipWorld::with(Setup { stores: stores.clone(), ..Setup::default() }).await;
        w.toggle_on().await;

        for _ in 0..2 {
            w.pasteboard.copy_text("same");
            after_copy().await;
        }
        assert_eq!(w.announcements().len(), 1, "a duplicate within one run");
        w.clipboard.shutdown();
    }

    let w = ClipWorld::with(Setup { stores, ..Setup::default() }).await;
    assert!(w.clipboard.phone_toggle(&phone_id()).unwrap(), "the toggle persisted");

    w.pasteboard.copy_text("same");
    after_copy().await;
    assert_eq!(w.announcements().len(), 1, "announced again after the restart");
}

/// Timers and limits: the defaults, and an announcement 200 ms after the change is seen,
/// which the 250 ms poll sees at most 250 ms after the copy.
#[tokio::test(start_paused = true)]
async fn the_default_timers_and_limits() {
    let config = ClipboardConfig::default();
    assert_eq!(config.poll_interval, Duration::from_millis(250));
    assert_eq!(config.debounce, Duration::from_millis(200));
    assert_eq!(config.text_read_limit, 131_072);
    assert_eq!(config.image_byte_limit, 1_048_576);
    assert_eq!(config.image_max_passes, 5);
    assert_eq!(config.request_timeout, Duration::from_secs(120));

    let w = supported().await;
    for n in 0..4 {
        // Copies at different phases of the poll.
        tokio::time::sleep(Duration::from_millis(70)).await;
        w.pasteboard.copy_text(&format!("copy {n}"));

        tokio::time::sleep(Duration::from_millis(199)).await;
        assert_eq!(w.announcements().len(), n, "not before the debounce");
        tokio::time::sleep(Duration::from_millis(251)).await;
        settle().await;
        assert_eq!(w.announcements().len(), n + 1, "within poll + debounce");
    }
}

#[tokio::test(start_paused = true)]
async fn a_shutdown_during_the_announcement_read_announces_nothing() {
    let w = ClipWorld::with(Setup { gated: true, ..Setup::default() }).await;
    w.toggle_on().await;
    let gate = w.gate.clone().unwrap();

    // The feature stops while the announcement reads the pasteboard (from the reading thread).
    let clipboard = w.clipboard.clone();
    gate.on_next_read(move || clipboard.shutdown());

    w.pasteboard.copy_text("copied while the feature stops");
    after_copy().await;
    assert!(gate.fired(), "the announcement read the pasteboard");
    assert!(w.w.pair.pc_link.sent_on(route::CONTEXT_PUBLISH).is_empty(), "nothing is sent after shutdown");
}

#[tokio::test(start_paused = true)]
async fn an_unavailable_phone_is_not_announced_to() {
    let w = supported().await;
    w.w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::NoticeFromHub);
    settle().await;

    w.pasteboard.copy_text("nobody listens");
    after_copy().await;
    assert!(w.announcements().is_empty());
    assert!(w.w.pair.pc_link.sent_on(route::CONTEXT_PUBLISH).is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_kind_is_image_then_html_then_text() {
    let mut w = supported().await;
    w.take_events();
    let tiff = FakeCodec::image(FakeFormat::Tiff, 10, 10);

    // Declared text first, HTML, then a raster image: announced as an image.
    w.pasteboard.copy(&[(uti::PLAIN_TEXT, b"t"), (uti::HTML, b"<i>t</i>"), (uti::TIFF, &tiff)]);
    after_copy().await;
    w.pasteboard.copy(&[(uti::PLAIN_TEXT, b"t2"), (uti::HTML, b"<i>t2</i>")]);
    after_copy().await;
    w.pasteboard.copy(&[("public.rtf", b"{\\rtf1 x}"), (uti::PLAIN_TEXT, b"t3")]);
    after_copy().await;
    assert_eq!(announced_kinds(&mut w), vec![ClipKind::Image, ClipKind::Html, ClipKind::Text]);
}

/// `(uti, bytes)` pairs in declared order.
type OwnedItems = Vec<(&'static str, Vec<u8>)>;

#[tokio::test(start_paused = true)]
async fn unrecognized_and_unreadable_content_is_not_announced() {
    let w = supported().await;
    let cases: Vec<(&str, OwnedItems)> = vec![
        ("HTML without plain text", vec![(uti::HTML, b"<b>x</b>".to_vec())]),
        ("a file copy", vec![(uti::FILE_URL, b"file:///x".to_vec()), (uti::PLAIN_TEXT, b"x".to_vec())]),
        ("concealed", vec![(uti::PLAIN_TEXT, b"secret".to_vec()), (uti::CONCEALED, Vec::new())]),
        ("transient", vec![(uti::TRANSIENT, Vec::new()), (uti::PLAIN_TEXT, b"t".to_vec())]),
        ("PDF only", vec![("com.adobe.pdf", b"%PDF".to_vec())]),
        ("RTF only", vec![("public.rtf", b"{\\rtf1}".to_vec())]),
        ("an image that does not decode", vec![(uti::PNG, b"garbage".to_vec())]),
        ("text over the limit", vec![(uti::PLAIN_TEXT, "x".repeat(131_073).into_bytes())]),
        // 65,537 two-unit characters: 131,074 UTF-16 code units.
        ("UTF-16 units count", vec![(uti::PLAIN_TEXT, "\u{1F600}".repeat(65_537).into_bytes())]),
    ];

    for (name, items) in &cases {
        let items: Vec<(&str, &[u8])> = items.iter().map(|(u, d)| (*u, d.as_slice())).collect();
        w.pasteboard.copy(&items);
        after_copy().await;
        assert!(w.announcements().is_empty(), "{name}");
    }

    w.pasteboard.copy_text(&"x".repeat(131_072));
    after_copy().await;
    assert_eq!(w.announcements().len(), 1, "exactly at the limit");
}

#[tokio::test(start_paused = true)]
async fn image_hashes() {
    let w = supported().await;
    // A zero-sized image hashes no bytes and is announced …
    w.pasteboard.copy(&[(uti::PNG, &FakeCodec::image(FakeFormat::Png, 0, 5))]);
    after_copy().await;
    assert_eq!(w.announcements().len(), 1);

    // … and so does an image whose PNG encode fails: the same hash, so a duplicate.
    w.codec.fail_png(true);
    w.pasteboard.copy(&[(uti::TIFF, &FakeCodec::image(FakeFormat::Tiff, 3, 3))]);
    after_copy().await;
    assert_eq!(w.announcements().len(), 1);
    w.codec.fail_png(false);

    // A real image: the hash of its full-size PNG; the same image again is a duplicate even from
    // another representation.
    w.pasteboard.copy(&[(uti::TIFF, &FakeCodec::image(FakeFormat::Tiff, 3, 3))]);
    after_copy().await;
    assert_eq!(w.announcements().len(), 2);

    w.pasteboard.copy(&[(uti::PNG, &FakeCodec::image(FakeFormat::Png, 3, 3))]);
    after_copy().await;
    assert_eq!(w.announcements().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_failing_pasteboard_is_isolated() {
    let w = supported().await;
    w.pasteboard.set_failing(true);
    w.pasteboard.copy_text("x");
    tokio::time::sleep(Duration::from_secs(2)).await;

    w.pasteboard.set_failing(false);
    after_copy().await;
    assert_eq!(w.announcements().len(), 1, "the change is seen once the pasteboard reads again");
}
