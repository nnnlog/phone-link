//! Phone → PC over the platform world — the change announcement (inbound tag 9), the content pull,
//! response handling, and the echo suppression of the app's own write.

use crate::support::clipboard_world as clip;
use crate::support::platform_world as fakes;

use std::time::Duration;

use clip::*;
use fakes::{phone_id, settle};
use phonelink_engine::edge::LockState;
use phonelink_engine::features::clipboard::image::standins::{FakeCodec, FakeFormat};
use phonelink_engine::features::clipboard::pasteboard::uti;
use phonelink_engine::features::clipboard::{ClipKind, ClipboardEvent, ClipboardPasteboardData};
use phonelink_engine::platform::route;
use phonelink_engine::proto::clipboard::v1 as cb;
use phonelink_engine::proto::platform_sdk::v1 as pb;
use prost::Message;

const CONTENT: i32 = cb::ClipboardRequestType::Content as i32;
const CHANGE: cb::ClipboardResponseStatus = cb::ClipboardResponseStatus::ClipboardChange;

fn rep(uti: &str, data: &[u8]) -> ClipboardPasteboardData {
    ClipboardPasteboardData { uti: uti.into(), data: data.to_vec() }
}

/// A world whose phone answers pulls with `answer`, with the toggle on.
async fn serving(answer: Vec<u8>) -> ClipWorld {
    let w = ClipWorld::with(Setup { drm: PhoneDrm::Answer(Some(answer)), ..Setup::default() }).await;
    w.toggle_on().await;
    w
}

fn pulls(w: &ClipWorld) -> Vec<cb::ClipboardRequestMessage> {
    w.clipboard_requests().into_iter().map(|(_, r)| r).filter(|r| r.request_type == CONTENT).collect()
}

#[tokio::test(start_paused = true)]
async fn an_announcement_is_pulled_and_written_without_an_echo() {
    let mut w = serving(content(item(cb::ClipboardItemType::TextPlain, Some("hello"), None))).await;
    w.take_events();
    phone_announces(&w.w, "m-1", CID).await;

    let requests = w.clipboard_requests();
    let (drm, pull) = requests.last().unwrap();
    assert_eq!(pull.request_type, CONTENT);
    assert_eq!(pull.correlation_id, CID, "the announcement's correlation id");
    assert_eq!(drm.payload, [&[0x08, 0x01, 0x12, 0x24][..], CID.as_bytes()].concat());
    assert_eq!(drm.resource_path, "/clipboard");
    assert_eq!(w.pasteboard.writes(), vec![vec![rep(uti::PLAIN_TEXT, b"hello")]]);
    assert_eq!(w.take_events(), vec![ClipboardEvent::Received { phone: phone_id(), kind: ClipKind::Text }]);

    // The app's own write is not announced back.
    after_copy().await;
    assert!(w.announcements().is_empty());

    // A copy by the user after it is.
    w.pasteboard.copy_text("typed on the Mac");
    after_copy().await;
    assert_eq!(w.announcements().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn an_announcement_without_a_correlation_id_pulls_without_one() {
    let w = serving(content(item(cb::ClipboardItemType::TextPlain, Some("x"), None))).await;
    phone_announces(&w.w, "m-1", "").await;
    let requests = w.clipboard_requests();
    assert_eq!(requests.last().unwrap().0.payload, vec![0x08, 0x01]);
}

#[tokio::test(start_paused = true)]
async fn the_gates_drop_announcements_without_a_reply() {
    let answer = content(item(cb::ClipboardItemType::TextPlain, Some("x"), None));
    let w = ClipWorld::with(Setup { drm: PhoneDrm::Answer(Some(answer)), ..Setup::default() }).await;

    // The toggle is off by default.
    phone_announces(&w.w, "m-1", CID).await;
    assert!(pulls(&w).is_empty());

    w.toggle_on().await;
    w.clipboard.set_global_switch(false).unwrap();
    phone_announces(&w.w, "m-2", CID).await;
    w.clipboard.set_global_switch(true).unwrap();
    settle().await;

    w.switches.set_enabled(&phone_id(), false).unwrap();
    phone_announces(&w.w, "m-3", CID).await;
    w.switches.set_enabled(&phone_id(), true).unwrap();

    let mut device = phone_device();
    device.capabilities = Some(vec!["DEVICE_STATUS".into()]);
    w.peers.upsert(phone_peer(device));
    settle().await;
    phone_announces(&w.w, "m-4", CID).await;

    w.peers.remove(&phone_id());
    settle().await;
    phone_announces(&w.w, "m-5", CID).await;
    assert!(pulls(&w).is_empty(), "nothing pulled through a closed gate");
    assert!(w.pasteboard.writes().is_empty());

    w.peers.upsert(phone_peer(phone_device()));
    settle().await;
    phone_announces(&w.w, "m-6", CID).await;
    assert_eq!(pulls(&w).len(), 1, "the gates are read per announcement");
}

#[tokio::test(start_paused = true)]
async fn malformed_and_other_announcements_are_dropped() {
    let w = serving(content(item(cb::ClipboardItemType::TextPlain, Some("x"), None))).await;
    let change = clipboard_message(CHANGE, CID);

    // The message in `data` only: `additional` parses as an empty message (UNSPECIFIED).
    phone_publishes(&w.w, phone_publication("m-1", &change, b"")).await;

    // `additional` that does not parse.
    phone_publishes(&w.w, phone_publication("m-2", b"", &[0xFF, 0xFF, 0xFF])).await;

    // Other statuses.
    for (id, status) in [("m-3", cb::ClipboardResponseStatus::Ok), ("m-4", cb::ClipboardResponseStatus::FeatureOn)] {
        phone_publishes(&w.w, phone_publication(id, b"", &clipboard_message(status, CID))).await;
    }

    // Another tag.
    phone_publishes(&w.w, phone_publication_tag(8, "m-5", b"", &change)).await;

    assert!(pulls(&w).is_empty());

    // A CLIPBOARD_CHANGE in `additional` with other fields set is still a change.
    let mut with_item = cb::ClipboardResponseMessage::decode(change.as_slice()).unwrap();
    with_item.clipboard_item.push(item(cb::ClipboardItemType::TextPlain, Some("ignored"), None));
    with_item.error_detail = "ignored".into();
    phone_publishes(&w.w, phone_publication("m-6", b"data is ignored", &with_item.encode_to_vec())).await;
    assert_eq!(pulls(&w).len(), 1);
    assert_eq!(w.pasteboard.writes(), vec![vec![rep(uti::PLAIN_TEXT, b"x")]], "the pulled content, not the item");
}

/// `message_tag` matches as a full `int32`: 9 + 65536 is not 9.
#[tokio::test(start_paused = true)]
async fn the_tag_matches_as_a_full_int32() {
    let w = serving(content(item(cb::ClipboardItemType::TextPlain, Some("x"), None))).await;
    let change = clipboard_message(CHANGE, CID);

    phone_publishes(&w.w, phone_publication_tag(9 + 65_536, "m-1", b"", &change)).await;
    assert!(pulls(&w).is_empty());

    phone_publishes(&w.w, phone_publication_tag(9, "m-2", b"", &change)).await;
    assert_eq!(pulls(&w).len(), 1);
}

/// Step 1: the phone is the partner of the connection, not `MsaepMessage.dcg_client_id`.
#[tokio::test(start_paused = true)]
async fn the_phone_is_the_connection_partner_not_the_declared_publisher() {
    let w = serving(content(item(cb::ClipboardItemType::TextPlain, Some("x"), None))).await;
    let payload = pb::PubSubPayload { data: Vec::new(), additional: clipboard_message(CHANGE, CID) }.encode_to_vec();
    let body = pb::MsaepMessage {
        message_tag: 9,
        dcg_client_id: "not-a-device-of-the-account".into(),
        message_id: "m-1".into(),
        payload,
        platform_major_version: 1,
        platform_minor_version: 1,
    }
    .encode_to_vec();

    let message =
        fakes::platform(&[("_route", route::CONTEXT_PUBLISH), ("ms-content-type", "application/x-binary")], &body);

    phone_publishes(&w.w, message).await;
    assert_eq!(pulls(&w).len(), 1, "identified by its connection: the gates pass and the pull goes out");
}

/// Step 3 with the strict proto3 decode (differs from Phone Link on Windows): what a lenient parser
/// would accept — invalid UTF-8 in `correlation_id`, a known field with another wire type — does not
/// parse, so the announcement is dropped.
#[tokio::test(start_paused = true)]
async fn an_announcement_failing_the_strict_decode_is_dropped() {
    let w = serving(content(item(cb::ClipboardItemType::TextPlain, Some("x"), None))).await;

    // CLIPBOARD_CHANGE + correlation_id holding a byte that is not UTF-8.
    phone_publishes(&w.w, phone_publication("m-1", b"", &[0x10, 0x06, 0x1a, 0x01, 0xff])).await;

    // CLIPBOARD_CHANGE + field 3 (`correlation_id`) written as a varint.
    phone_publishes(&w.w, phone_publication("m-2", b"", &[0x10, 0x06, 0x18, 0x01])).await;
    assert!(pulls(&w).is_empty());

    phone_publishes(&w.w, phone_publication("m-3", b"", &[0x10, 0x06])).await;
    assert_eq!(pulls(&w).len(), 1, "a well-formed announcement still pulls");
}

/// The content pull reads the device list again: a phone that left it after its announcement is
/// not pulled from.
#[tokio::test(start_paused = true)]
async fn the_pull_rechecks_the_device_list() {
    let w = serving(content(item(cb::ClipboardItemType::TextPlain, Some("x"), None))).await;

    // Found by the announcement (step 1), gone when the pull looks it up.
    w.directory.vanish_after(&phone_id(), 1);
    phone_announces(&w.w, "m-1", CID).await;
    assert!(pulls(&w).is_empty(), "nothing is sent for a phone no longer in the device list");
    assert!(w.pasteboard.writes().is_empty());
}

#[tokio::test(start_paused = true)]
async fn each_announcement_starts_its_own_pull() {
    let w = serving(content(item(cb::ClipboardItemType::TextPlain, Some("x"), None))).await;
    phone_announces(&w.w, "m-1", "c-1").await;
    phone_announces(&w.w, "m-2", "c-2").await;

    // The same message id again is dropped by the MSAEP layer (the last id delivered).
    phone_announces(&w.w, "m-2", "c-3").await;

    let ids: Vec<String> = pulls(&w).into_iter().map(|r| r.correlation_id).collect();
    assert_eq!(ids, vec!["c-1", "c-2"]);
    assert_eq!(w.pasteboard.writes().len(), 2);
}

/// A case name, the phone's content answer, and what is written (`None`: nothing).
type ResponseCase = (&'static str, Vec<u8>, Option<Vec<ClipboardPasteboardData>>);

#[tokio::test(start_paused = true)]
async fn response_handling() {
    let png = FakeCodec::image_with_len(FakeFormat::Png, 4, 3, 64);
    let bmp = FakeCodec::image(FakeFormat::Bmp, 4, 3);

    let ok_with = |items: Vec<cb::ClipboardItem>| {
        cb::ClipboardResponseMessage {
            clipboard_item: items,
            status: cb::ClipboardResponseStatus::Ok as i32,
            ..Default::default()
        }
        .encode_to_vec()
    };

    let invalid = cb::ClipboardResponseMessage {
        status: cb::ClipboardResponseStatus::InvalidContent as i32,
        error_type: cb::ClipboardErrorType::Reject as i32,
        error_detail: "too big".into(),
        ..Default::default()
    }
    .encode_to_vec();

    let text = |t: &str| item(cb::ClipboardItemType::TextPlain, Some(t), None);

    // A TEXT_PLAIN item whose text holds a byte that is not UTF-8 (`61 ff 62`).
    let invalid_utf8_item = [0x08, 0x02, 0x12, 0x03, 0x61, 0xff, 0x62, 0x1a, 0x00];
    let cases: Vec<ResponseCase> = vec![
        ("no payload", Vec::new(), None),
        ("unparsable", vec![0xFF, 0xFF], None),
        // The strict proto3 decode (differs from Phone Link on Windows): a lenient parser would
        // accept both.
        ("invalid UTF-8 text", [&[0x0a, 0x09][..], &invalid_utf8_item, &[0x10, 0x01]].concat(), None),
        ("status written as length-delimited", vec![0x12, 0x01, 0x01], None),
        ("INVALID_CONTENT", invalid, None),
        ("FEATURE_OFF", clipboard_message(cb::ClipboardResponseStatus::FeatureOff, CID), None),
        ("OK without items", ok_with(vec![]), None),
        ("OK with two items", ok_with(vec![text("a"), text("b")]), None),
        ("empty text", content(text("")), None),
        ("absent text", content(item(cb::ClipboardItemType::TextPlain, None, Some(b""))), None),
        ("text", content(text("hi there")), Some(vec![rep(uti::PLAIN_TEXT, b"hi there")])),
        (
            "HTML with its derived plain text",
            content(item(cb::ClipboardItemType::TextHtml, Some("<p>a &amp; <b>b</b></p><script>x()</script>"), None)),
            Some(vec![rep(uti::HTML, b"<p>a &amp; <b>b</b></p><script>x()</script>"), rep(uti::PLAIN_TEXT, b"a & b")]),
        ),
        (
            "HTML whose plain text is empty",
            content(item(cb::ClipboardItemType::TextHtml, Some("<img src=x>"), None)),
            Some(vec![rep(uti::HTML, b"<img src=x>")]),
        ),
        ("empty HTML", content(item(cb::ClipboardItemType::TextHtml, Some(""), None)), None),
        (
            "PNG image",
            content(item(cb::ClipboardItemType::Image, Some(""), Some(&png))),
            Some(vec![rep(uti::PNG, &png), rep(uti::TIFF, &FakeCodec::image(FakeFormat::Tiff, 4, 3))]),
        ),
        (
            "other image format",
            content(item(cb::ClipboardItemType::Image, Some(""), Some(&bmp))),
            Some(vec![
                rep(uti::PNG, &FakeCodec::image_with_len(FakeFormat::Png, 4, 3, 16 + 12)),
                rep(uti::TIFF, &FakeCodec::image(FakeFormat::Tiff, 4, 3)),
            ]),
        ),
        ("undecodable image", content(item(cb::ClipboardItemType::Image, None, Some(b"not an image"))), None),
        ("empty image", content(item(cb::ClipboardItemType::Image, Some(""), Some(b""))), None),
        ("image without bytes", content(item(cb::ClipboardItemType::Image, Some("x"), None)), None),
        ("UNSPECIFIED item", content(item(cb::ClipboardItemType::Unspecified, Some("x"), Some(b"x"))), None),
        (
            "unknown item type",
            content(cb::ClipboardItem { item_type: 7, text: Some("x".into()), ..Default::default() }),
            None,
        ),
    ];

    for (n, (name, answer, written)) in cases.into_iter().enumerate() {
        let w = serving(answer).await;
        phone_announces(&w.w, &format!("m-{n}"), CID).await;
        assert_eq!(pulls(&w).len(), 1, "{name}: pulled");
        match written {
            Some(representations) => assert_eq!(w.pasteboard.writes(), vec![representations], "{name}"),
            None => {
                assert!(w.pasteboard.writes().is_empty(), "{name}: nothing written");
                assert_eq!(w.pasteboard.current_change_count(), 0, "{name}: the pasteboard is untouched");
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_large_inbound_image_is_written_whole() {
    let big = FakeCodec::image_with_len(FakeFormat::Png, 8000, 6000, 3 * 1_048_576);
    let w = serving(content(item(cb::ClipboardItemType::Image, Some(""), Some(&big)))).await;

    phone_announces(&w.w, "m-1", CID).await;

    let writes = w.pasteboard.writes();
    assert_eq!(writes[0][0], rep(uti::PNG, &big), "no size limit and no resize");
}

#[tokio::test(start_paused = true)]
async fn the_last_write_to_complete_wins() {
    let w = serving(content(item(cb::ClipboardItemType::TextPlain, Some("first"), None))).await;
    phone_announces(&w.w, "m-1", "c-1").await;
    phone_drm(&w.w, PhoneDrm::Answer(Some(content(item(cb::ClipboardItemType::TextPlain, Some("second"), None)))));
    phone_announces(&w.w, "m-2", "c-2").await;
    assert_eq!(w.pasteboard.contents(), vec![rep(uti::PLAIN_TEXT, b"second")]);

    // Neither write is announced back.
    after_copy().await;
    assert!(w.announcements().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_failed_pull_writes_nothing_and_is_not_retried() {
    let w = ClipWorld::with(Setup { drm: PhoneDrm::Silent, ..Setup::default() }).await;
    w.toggle_on().await;
    phone_announces(&w.w, "m-1", CID).await;

    tokio::time::sleep(Duration::from_secs(119)).await;
    assert_eq!(pulls(&w).len(), 1);

    tokio::time::sleep(Duration::from_secs(300)).await;
    assert_eq!(pulls(&w).len(), 1, "timed out after 120 s, no retry");
    assert!(w.pasteboard.writes().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_pull_goes_through_the_request_wrapper() {
    let w = serving(content(item(cb::ClipboardItemType::TextPlain, Some("x"), None))).await;
    // A locked PC: no pull (the receiver of the phone's own requests does not check the lock).
    w.w.lock.set(LockState::Locked);
    phone_announces(&w.w, "m-1", CID).await;
    assert!(pulls(&w).is_empty());
    w.w.lock.set(LockState::Unlocked);

    // A phone app below the minimum version: no pull.
    let mut device = phone_device();
    device.client_version = Some("1.23112.188".into());
    w.peers.upsert(phone_peer(device));
    settle().await;
    phone_announces(&w.w, "m-2", CID).await;
    assert!(pulls(&w).is_empty());
    assert!(w.pasteboard.writes().is_empty());

    w.peers.upsert(phone_peer(phone_device()));
    settle().await;
    phone_announces(&w.w, "m-3", CID).await;
    assert_eq!(pulls(&w).len(), 1);
}

/// A write the pasteboard stops part-way changed it all the same; its change count is recorded, so the
/// phone's clip is not announced back.
#[tokio::test(start_paused = true)]
async fn a_write_stopped_part_way_is_not_announced_back() {
    let png = FakeCodec::image_with_len(FakeFormat::Png, 4, 3, 64);
    let mut w = serving(content(item(cb::ClipboardItemType::Image, Some(""), Some(&png)))).await;
    w.take_events();
    w.pasteboard.refuse_type(Some(uti::TIFF));

    phone_announces(&w.w, "m-1", CID).await;
    assert_eq!(w.pasteboard.contents(), vec![rep(uti::PNG, &png)], "the PNG written before the refused TIFF");
    assert!(w.take_events().is_empty(), "not reported as received");
    after_copy().await;
    assert!(w.announcements().is_empty(), "the count the write reached is the engine's own");

    // A copy by the user after it is announced.
    w.pasteboard.refuse_type(None);
    w.pasteboard.copy_text("typed on the Mac");
    after_copy().await;
    assert_eq!(w.announcements().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_failing_pasteboard_write_is_isolated() {
    let w = serving(content(item(cb::ClipboardItemType::TextPlain, Some("x"), None))).await;
    w.pasteboard.set_failing(true);

    phone_announces(&w.w, "m-1", CID).await;
    assert!(w.pasteboard.writes().is_empty());

    w.pasteboard.set_failing(false);
    phone_announces(&w.w, "m-2", CID).await;
    assert_eq!(w.pasteboard.writes().len(), 1, "the next announcement is handled");
}
