use super::*;
use phonelink_proto::platform_sdk::v1::{DeviceResourceManagerMessage, DeviceResourceManagerResponseMessage};
use std::time::Duration;

/// The synthetic correlation id of the example messages.
const CID: &str = "00000000-0000-4000-8000-000000000001";

fn hex(s: &str) -> Vec<u8> {
    hex::decode(s.split_whitespace().collect::<String>()).unwrap()
}

fn cid() -> Vec<u8> {
    hex("30 30 30 30 30 30 30 30 2d 30 30 30 30 2d 34 30 30 30 2d 38 30 30 30 2d 30 30 30 30 30 30 30 30 30 30 30 31")
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

fn created() -> Timestamp {
    timestamp(UNIX_EPOCH + Duration::from_secs(1_760_000_000))
}

#[test]
fn the_page_vectors() {
    assert_eq!(cid(), CID.as_bytes());

    let content = content_request(CID);
    assert_eq!(content, cat(&[&hex("08 01 12 24"), &cid()]));
    assert_eq!(content.len(), 40);

    let drm = DeviceResourceManagerMessage {
        resource_type: 4,
        request_type: 1,
        payload: content.clone(),
        resource_path: RESOURCE_PATH.into(),
    }
    .encode_to_vec();
    assert_eq!(drm, cat(&[&hex("08 04 10 01 1a 28"), &content, &hex("22 0a 2f 63 6c 69 70 62 6f 61 72 64")]));

    let feature_on = feature_state_request(ClipboardRequestType::FeatureOn);
    assert_eq!(feature_on, hex("08 02"));
    let drm = DeviceResourceManagerMessage {
        resource_type: 4,
        request_type: 1,
        payload: feature_on,
        resource_path: RESOURCE_PATH.into(),
    }
    .encode_to_vec();
    assert_eq!(drm, hex("08 04 10 01 1a 02 08 02 22 0a 2f 63 6c 69 70 62 6f 61 72 64"));

    let announced = announcement(CID);
    assert_eq!(announced.data, cat(&[&hex("10 06 1a 24"), &cid()]));
    assert!(announced.additional.is_empty());
    assert_eq!(announced.encode_to_vec(), cat(&[&hex("0a 28 10 06 1a 24"), &cid()]));

    assert_eq!(status_answer(ClipboardResponseStatus::FeatureOn, Some(CID)), cat(&[&hex("10 08 1a 24"), &cid()]));

    let item = text_item(ClipboardItemType::TextPlain, "hi".into(), created());
    let item_bytes = item.encode_to_vec();
    assert_eq!(item_bytes, hex("08 02 12 02 68 69 1a 00 22 06 08 80 f0 9d c7 06"));

    let answer = content_answer(item, CID);
    assert_eq!(answer, cat(&[&hex("0a 10"), &item_bytes, &hex("10 01 1a 24"), &cid()]));
    assert_eq!(answer.len(), 58);

    let response =
        DeviceResourceManagerResponseMessage { payload: answer.clone(), response_type: Some(1) }.encode_to_vec();
    assert_eq!(response, cat(&[&hex("0a 3a"), &answer, &hex("10 01")]));
}

#[test]
fn optional_fields_are_written_when_set_even_empty() {
    let item = image_item(Vec::new(), Timestamp { seconds: 0, nanos: 0 });
    // item_type 1, text "" (12 00), image_bytes empty (1a 00), created_time empty (22 00).
    assert_eq!(item.encode_to_vec(), hex("08 01 12 00 1a 00 22 00"));

    let item = image_item(vec![0x89, 0x50], created());
    assert_eq!(item.encode_to_vec(), hex("08 01 12 00 1a 02 89 50 22 06 08 80 f0 9d c7 06"));

    let html = text_item(ClipboardItemType::TextHtml, "<b>x</b>".into(), created());
    assert_eq!(html.encode_to_vec()[..2], hex("08 03")[..]);
}

#[test]
fn defaults_are_omitted() {
    assert_eq!(content_request(""), hex("08 01"), "an announcement without an id gives a pull without one");
    assert_eq!(status_answer(ClipboardResponseStatus::InvalidDeviceResourceRequestType, None), hex("10 02"));
    assert_eq!(status_answer(ClipboardResponseStatus::UnrecognizedPayload, Some("")), hex("10 05"));
    assert!(feature_state_request(ClipboardRequestType::Unspecified).is_empty());
}

#[test]
fn timestamps_have_100ns_resolution() {
    let t = timestamp(UNIX_EPOCH + Duration::new(5, 123_456_789));
    assert_eq!((t.seconds, t.nanos), (5, 123_456_700));
    let t = timestamp(UNIX_EPOCH - Duration::new(1, 500_000_000));
    assert_eq!((t.seconds, t.nanos), (-2, 500_000_000));
    let t = timestamp(UNIX_EPOCH - Duration::from_secs(3));
    assert_eq!((t.seconds, t.nanos), (-3, 0));
}
