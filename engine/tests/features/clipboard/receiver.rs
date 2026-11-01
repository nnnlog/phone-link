//! The PC's `/clipboard` resource over the platform world — serving `/clipboard` to the phone (each
//! of the ten answers, the envelope of every answer), the content answer, image re-encoding,
//! correlation ids, and the error answer to a request without a `_requestId`.

use crate::support::clipboard_world as clip;
use crate::support::platform_world as fakes;

use std::time::{Duration, UNIX_EPOCH};

use clip::*;
use fakes::{phone_id, settle};
use phonelink_engine::edge::LockState;
use phonelink_engine::features::clipboard::image::standins::{FakeCodec, FakeFormat};
use phonelink_engine::features::clipboard::pasteboard::uti;
use phonelink_engine::platform::route;
use phonelink_engine::proto::clipboard::v1 as cb;
use phonelink_engine::proto::platform_sdk::v1 as pb;
use phonelink_engine::seams::PlatformRequestStatus;
use prost::Message;

use cb::ClipboardRequestType as Req;
use cb::ClipboardResponseStatus as Status;

/// A world with the toggle on, so requests reach rows 9 and 10.
async fn serving() -> ClipWorld {
    let w = ClipWorld::new().await;
    w.toggle_on().await;
    w
}

/// Sends a phone `GET /clipboard` with `{ request_type, correlation_id }` and returns the answer.
async fn get(w: &ClipWorld, request_type: Req, correlation_id: &str) -> cb::ClipboardResponseMessage {
    let response = ask(&w.w, phone_get(&clipboard_request(request_type as i32, correlation_id))).await;
    assert_eq!(response.status, PlatformRequestStatus::Success, "{request_type:?} is answered");
    let (drm, answer) = answer_of(&response);
    assert_eq!(drm.response_type, Some(pb::DeviceResourceManagerResponseMessageType::Success as i32));
    answer
}

fn status_of(answer: &cb::ClipboardResponseMessage) -> (i32, &str) {
    (answer.status, answer.correlation_id.as_str())
}

#[tokio::test(start_paused = true)]
async fn status_rows_and_their_correlation_ids() {
    let w = serving().await;
    // Row 1: a DRM request type other than GET.
    let response = ask(&w.w, phone_drm_message(2, "/clipboard", &clipboard_request(Req::Status as i32, CID))).await;
    let (_, answer) = answer_of(&response);
    assert_eq!(status_of(&answer), (Status::InvalidDeviceResourceRequestType as i32, ""));

    // Row 3: no payload.
    let (_, answer) = answer_of(&ask(&w.w, phone_get(b"")).await);
    assert_eq!(status_of(&answer), (Status::UnrecognizedPayload as i32, ""));

    // Row 4 (differs from Phone Link on Windows): a payload that does not parse.
    let (_, answer) = answer_of(&ask(&w.w, phone_get(&[0xFF, 0xFF, 0xFF])).await);
    assert_eq!(status_of(&answer), (Status::UnrecognizedPayload as i32, ""));

    // Row 5: request types the PC does not serve, with the request's id or a fresh one.
    for request_type in [Req::FeatureOn, Req::FeatureOff, Req::FeatureDisable, Req::Unspecified] {
        assert_eq!(
            status_of(&get(&w, request_type, CID).await),
            (Status::InvalidClipboardRequestType as i32, CID),
            "{request_type:?}"
        );
    }

    let unknown = ask(&w.w, phone_get(&clipboard_request(99, ""))).await;
    let (_, answer) = answer_of(&unknown);
    assert_eq!(answer.status, Status::InvalidClipboardRequestType as i32);
    assert!(uuid::Uuid::parse_str(&answer.correlation_id).is_ok());

    // Row 9: STATUS on a supported phone — the page's vector.
    let response = ask(&w.w, phone_get(&clipboard_request(Req::Status as i32, CID))).await;
    let (drm, answer) = answer_of(&response);
    assert_eq!(drm.payload, [&[0x10, 0x08, 0x1a, 0x24][..], CID.as_bytes()].concat());
    assert_eq!(status_of(&answer), (Status::FeatureOn as i32, CID));
    assert!(answer.clipboard_item.is_empty());

    // A STATUS without an id gets a fresh lower-case GUID.
    let answer = get(&w, Req::Status, "").await;
    assert_eq!(answer.correlation_id, answer.correlation_id.to_lowercase());
    assert!(uuid::Uuid::parse_str(&answer.correlation_id).is_ok());
}

/// Row 4 with the strict proto3 decode (differs from Phone Link on Windows): what a lenient parser
/// would accept — invalid UTF-8 in `correlation_id`, a known field with another wire type — and
/// what both parsers reject (an end-group tag) are all answered `UNRECOGNIZED_PAYLOAD`, no id.
#[tokio::test(start_paused = true)]
async fn a_payload_failing_the_strict_decode_is_answered_unrecognized_payload() {
    let w = serving().await;
    for (name, payload) in [
        ("STATUS, correlation_id not UTF-8", vec![0x08, 0x05, 0x12, 0x01, 0xff]),
        ("STATUS, field 2 written as a varint", vec![0x08, 0x05, 0x10, 0x07]),
        ("STATUS, then an end-group tag", vec![0x08, 0x05, 0x0c]),
    ] {
        let (_, answer) = answer_of(&ask(&w.w, phone_get(&payload)).await);
        assert_eq!(status_of(&answer), (Status::UnrecognizedPayload as i32, ""), "{name}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_shutdown_while_the_content_answer_is_built_answers_nothing() {
    let w = ClipWorld::with(Setup { gated: true, ..Setup::default() }).await;
    w.toggle_on().await;

    // Copied and announced first, so the next pasteboard read is the content answer's.
    w.pasteboard.copy_text("x");
    after_copy().await;

    let gate = w.gate.clone().unwrap();
    let clipboard = w.clipboard.clone();
    gate.on_next_read(move || clipboard.shutdown());

    let response = ask(&w.w, phone_get(&clipboard_request(Req::Content as i32, CID))).await;
    assert!(gate.fired(), "the answer read the pasteboard");
    assert_eq!(response.status, PlatformRequestStatus::ResponseTimedOut, "no answer after shutdown");
}

#[tokio::test(start_paused = true)]
async fn the_answer_envelope() {
    let w = serving().await;
    let response = ask(&w.w, phone_get(&clipboard_request(Req::Status as i32, CID))).await;

    let message = response.message.unwrap();
    assert_eq!(message.headers.get("_route"), Some(route::INTERNAL_RESPONSE));
    assert_eq!(message.headers.get("ms-content-type"), Some("application/x-binary"));
    assert!(message.headers.get("_originalRequestId").is_some());
    let body = pb::DeviceResourceManagerResponseMessage::decode(message.payload.as_slice()).unwrap();
    assert_eq!(body.response_type, Some(1), "the clipboard status carries the outcome, never the DRM type");
}

#[tokio::test(start_paused = true)]
async fn the_gate_rows() {
    let w = ClipWorld::new().await;
    // Row 8: the toggle is off by default.
    assert_eq!(status_of(&get(&w, Req::Content, CID).await), (Status::FeatureOff as i32, CID));

    w.toggle_on().await;
    w.switches.set_enabled(&phone_id(), false).unwrap();
    assert_eq!(status_of(&get(&w, Req::Status, CID).await), (Status::FeatureOff as i32, CID));

    // Row 7 comes before row 8: global switch off.
    w.clipboard.set_global_switch(false).unwrap();
    assert_eq!(status_of(&get(&w, Req::Status, CID).await), (Status::FeatureDisable as i32, CID));

    w.clipboard.set_global_switch(true).unwrap();
    w.switches.set_enabled(&phone_id(), true).unwrap();

    // Row 7: the phone lacks CLIPBOARD.
    let mut device = phone_device();
    device.capabilities = Some(vec!["DEVICE_STATUS".into()]);
    w.peers.upsert(phone_peer(device));
    settle().await;
    assert_eq!(status_of(&get(&w, Req::Content, CID).await), (Status::FeatureDisable as i32, CID));

    // Row 6: the sender is not a phone of the account.
    w.peers.remove(&phone_id());
    settle().await;
    assert_eq!(status_of(&get(&w, Req::Status, CID).await), (Status::InvalidContent as i32, CID));

    // Rows 5 come first, whatever the gates.
    assert_eq!(status_of(&get(&w, Req::FeatureOn, CID).await), (Status::InvalidClipboardRequestType as i32, CID));
}

#[tokio::test(start_paused = true)]
async fn the_lock_state_and_the_phone_version_are_not_consulted() {
    let w = serving().await;
    w.w.lock.set(LockState::Locked);
    let mut device = phone_device();
    device.client_version = Some("1.0.0.1".into());
    w.peers.upsert(phone_peer(device));
    settle().await;

    assert_eq!(status_of(&get(&w, Req::Status, CID).await), (Status::FeatureOn as i32, CID));

    w.pasteboard.copy_text("still served");
    let answer = get(&w, Req::Content, CID).await;
    assert_eq!(answer.clipboard_item[0].text.as_deref(), Some("still served"));
}

#[tokio::test(start_paused = true)]
async fn a_request_without_request_id_is_dropped() {
    let w = serving().await;
    w.pasteboard.copy_text("x");
    let before = w.w.pair.pc_link.sent_on(route::INTERNAL_RESPONSE).len();

    let mut message = phone_get(&clipboard_request(Req::Content as i32, CID));
    message.headers.insert("_rejectionVersion", "1");
    w.w.pair.inject(message);
    settle().await;
    assert_eq!(w.w.pair.pc_link.sent_on(route::INTERNAL_RESPONSE).len(), before, "no answer");

    // The session continues.
    assert_eq!(status_of(&get(&w, Req::Status, CID).await), (Status::FeatureOn as i32, CID));
}

#[tokio::test(start_paused = true)]
async fn text_content_and_the_page_vector() {
    let w = serving().await;
    w.clock.set(UNIX_EPOCH + Duration::from_secs(1_760_000_000));
    w.pasteboard.copy_text("hi");

    let response = ask(&w.w, phone_get(&clipboard_request(Req::Content as i32, CID))).await;
    let message = response.message.unwrap();

    let item = [0x08, 0x02, 0x12, 0x02, 0x68, 0x69, 0x1a, 0x00, 0x22, 0x06, 0x08, 0x80, 0xf0, 0x9d, 0xc7, 0x06];
    let answer = [&[0x0a, 0x10][..], &item, &[0x10, 0x01, 0x1a, 0x24], CID.as_bytes()].concat();
    assert_eq!(answer.len(), 58);
    assert_eq!(message.payload, [&[0x0a, 0x3a][..], &answer, &[0x10, 0x01]].concat(), "byte for byte");

    // created_time is the time of the answer, at 100 ns resolution.
    w.clock.set(UNIX_EPOCH + Duration::new(1_800_000_000, 123_456_789));
    let answer = get(&w, Req::Content, CID).await;
    let created = answer.clipboard_item[0].created_time.unwrap();
    assert_eq!((created.seconds, created.nanos), (1_800_000_000, 123_456_700));
}

#[tokio::test(start_paused = true)]
async fn the_answer_is_the_pasteboard_when_the_request_arrives() {
    let w = serving().await;
    w.pasteboard.copy_text("first");
    assert_eq!(get(&w, Req::Content, CID).await.clipboard_item[0].text.as_deref(), Some("first"));

    w.pasteboard.copy_text("second");
    assert_eq!(get(&w, Req::Content, CID).await.clipboard_item[0].text.as_deref(), Some("second"));
}

#[tokio::test(start_paused = true)]
async fn text_and_html_answers() {
    let w = serving().await;
    let only = |answer: &cb::ClipboardResponseMessage| {
        assert_eq!(answer.status, Status::Ok as i32);
        assert_eq!(answer.correlation_id, CID);
        assert_eq!(answer.clipboard_item.len(), 1);
        answer.clipboard_item[0].clone()
    };

    // Text at the limit is sent; one unit more is INVALID_CONTENT (without error fields).
    let limit = "x".repeat(131_072);
    w.pasteboard.copy_text(&limit);
    let item = only(&get(&w, Req::Content, CID).await);
    assert_eq!(item.item_type, cb::ClipboardItemType::TextPlain as i32);
    assert_eq!(item.text.as_deref(), Some(limit.as_str()));
    assert_eq!(item.image_bytes, Some(Vec::new()), "image_bytes is set, empty (1a 00)");

    w.pasteboard.copy_text(&"x".repeat(131_073));
    let answer = get(&w, Req::Content, CID).await;
    assert_eq!(status_of(&answer), (Status::InvalidContent as i32, CID));
    assert_eq!((answer.error_type, answer.error_detail.as_str()), (0, ""), "never on the wire");

    // An empty text is still a text item.
    w.pasteboard.copy_text("");
    assert_eq!(only(&get(&w, Req::Content, CID).await).text.as_deref(), Some(""));

    // HTML: the HTML string itself.
    w.pasteboard.copy(&[(uti::PLAIN_TEXT, b"bold"), (uti::HTML, b"<b>bold</b>")]);
    let item = only(&get(&w, Req::Content, CID).await);
    assert_eq!(item.item_type, cb::ClipboardItemType::TextHtml as i32);
    assert_eq!(item.text.as_deref(), Some("<b>bold</b>"));
    assert_eq!(item.image_bytes, Some(Vec::new()));

    // Empty or too long HTML falls back to the plain text.
    w.pasteboard.copy(&[(uti::HTML, b""), (uti::PLAIN_TEXT, b"plain")]);
    let item = only(&get(&w, Req::Content, CID).await);
    assert_eq!((item.item_type, item.text.as_deref()), (cb::ClipboardItemType::TextPlain as i32, Some("plain")));

    let long_html = format!("<p>{}</p>", "y".repeat(131_072));
    w.pasteboard.copy(&[(uti::HTML, long_html.as_bytes()), (uti::PLAIN_TEXT, b"short")]);
    let item = only(&get(&w, Req::Content, CID).await);
    assert_eq!((item.item_type, item.text.as_deref()), (cb::ClipboardItemType::TextPlain as i32, Some("short")));

    // Neither readable: INVALID_CONTENT.
    w.pasteboard.copy(&[(uti::HTML, b"")]);
    assert_eq!(status_of(&get(&w, Req::Content, CID).await), (Status::InvalidContent as i32, CID));
}

#[tokio::test(start_paused = true)]
async fn image_answers() {
    let w = serving().await;
    // A small image: its full-size PNG; text is set, empty (12 00).
    w.pasteboard.copy(&[(uti::PLAIN_TEXT, b"caption"), (uti::TIFF, &FakeCodec::image(FakeFormat::Tiff, 100, 50))]);

    let response = ask(&w.w, phone_get(&clipboard_request(Req::Content as i32, CID))).await;
    let (drm, answer) = answer_of(&response);
    let item = &answer.clipboard_item[0];
    assert_eq!(item.item_type, cb::ClipboardItemType::Image as i32);
    assert_eq!(item.text.as_deref(), Some(""));
    let png = item.image_bytes.clone().unwrap();
    assert_eq!(FakeCodec::decode(&png), Some((FakeFormat::Png, 100, 50)));
    assert_eq!(png.len(), 16 + 5000);
    let item_bytes = item.encode_to_vec();
    assert_eq!(item_bytes[..4], [0x08, 0x01, 0x12, 0x00]);
    assert!(drm.payload.starts_with(&[0x0a]), "the item comes first");

    // Over 1 MiB: downscaled.
    w.pasteboard.copy(&[(uti::PNG, &FakeCodec::image(FakeFormat::Png, 4000, 3000))]);
    let answer = get(&w, Req::Content, CID).await;
    let png = answer.clipboard_item[0].image_bytes.clone().unwrap();
    assert_eq!(FakeCodec::decode(&png).map(|(_, w, h)| (w, h)), Some((349, 262)));

    // Zero-sized: an IMAGE item with empty bytes.
    w.pasteboard.copy(&[(uti::PNG, &FakeCodec::image(FakeFormat::Png, 0, 0))]);
    let answer = get(&w, Req::Content, CID).await;
    assert_eq!(answer.status, Status::Ok as i32);
    assert_eq!(answer.clipboard_item[0].image_bytes, Some(Vec::new()));

    // Failures are INVALID_CONTENT: too large after five passes, an encoder failure (differs from
    // Phone Link on Windows), an image that does not decode.
    w.codec.set_png_size(|_, _| 2 * 1_048_576);
    w.pasteboard.copy(&[(uti::PNG, &FakeCodec::image(FakeFormat::Png, 4000, 3000))]);
    assert_eq!(status_of(&get(&w, Req::Content, CID).await), (Status::InvalidContent as i32, CID));

    w.codec.set_png_size(|w, h| 16 + w as usize * h as usize);
    w.codec.fail_png(true);
    w.pasteboard.copy(&[(uti::TIFF, &FakeCodec::image(FakeFormat::Tiff, 10, 10))]);
    assert_eq!(status_of(&get(&w, Req::Content, CID).await), (Status::InvalidContent as i32, CID));

    w.codec.fail_png(false);
    w.pasteboard.copy(&[(uti::PNG, b"garbage")]);
    assert_eq!(status_of(&get(&w, Req::Content, CID).await), (Status::InvalidContent as i32, CID));
}

/// `(uti, bytes)` pairs in declared order.
type PasteboardItems = Vec<(&'static str, &'static [u8])>;

#[tokio::test(start_paused = true)]
async fn nothing_recognized_gets_no_answer() {
    let w = serving().await;
    let cases: Vec<(&str, PasteboardItems)> = vec![
        ("empty pasteboard", vec![]),
        ("PDF only", vec![("com.adobe.pdf", b"%PDF")]),
        ("RTF only", vec![("public.rtf", b"{\\rtf1}")]),
        ("a file copy", vec![(uti::FILE_URL, b"file:///x"), (uti::PLAIN_TEXT, b"x"), (uti::TIFF, b"MM\0*")]),
        ("concealed", vec![(uti::PLAIN_TEXT, b"password"), (uti::CONCEALED, b"")]),
        ("transient", vec![(uti::PLAIN_TEXT, b"t"), (uti::TRANSIENT, b"")]),
    ];

    for (name, items) in cases {
        w.pasteboard.copy(&items);
        let response = ask(&w.w, phone_get(&clipboard_request(Req::Content as i32, CID))).await;
        assert_eq!(response.status, PlatformRequestStatus::ResponseTimedOut, "{name}: the phone's request times out");
    }

    // STATUS is still answered.
    assert_eq!(status_of(&get(&w, Req::Status, CID).await), (Status::FeatureOn as i32, CID));
}

#[tokio::test(start_paused = true)]
async fn a_failing_pasteboard_is_isolated() {
    let w = serving().await;
    w.pasteboard.copy_text("x");
    w.pasteboard.set_failing(true);

    let response = ask(&w.w, phone_get(&clipboard_request(Req::Content as i32, CID))).await;
    assert_eq!(response.status, PlatformRequestStatus::ResponseTimedOut);

    w.pasteboard.set_failing(false);
    assert_eq!(get(&w, Req::Content, CID).await.clipboard_item[0].text.as_deref(), Some("x"));
}
