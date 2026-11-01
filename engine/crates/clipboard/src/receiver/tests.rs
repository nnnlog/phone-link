use super::*;
use phonelink_core::types::{DcgClient, Environment};
use phonelink_proto::clipboard::v1::ClipboardResponseMessage;
use phonelink_proto::platform_sdk::v1::ProtoDeviceResourceType;
use phonelink_transport::platform::DrmChannel;

fn request(request_type: ProtoDeviceResourceRequestType, path: &str, payload: Vec<u8>) -> DrmRequest {
    DrmRequest {
        source: DcgClient::new("phone-1", Environment::Prod),
        request_id: Some("1".into()),
        path: path.into(),
        resource_type: ProtoDeviceResourceType::Unknown,
        request_type,
        payload,
        channel: DrmChannel::Platform,
    }
}

fn rejected(r: Result<(ClipboardRequestType, String), Vec<u8>>) -> ClipboardResponseMessage {
    ClipboardResponseMessage::decode(r.expect_err("a status answer").as_slice()).unwrap()
}

#[test]
fn rows_one_to_five() {
    use ClipboardResponseStatus as S;
    let status = wire::request(ClipboardRequestType::Status, "c-1");
    let get = ProtoDeviceResourceRequestType::Get;

    let r = rejected(check_request(&request(ProtoDeviceResourceRequestType::Update, "/clipboard", status.clone())));
    assert_eq!((r.status, r.correlation_id.as_str()), (S::InvalidDeviceResourceRequestType as i32, ""));

    for path in ["", "/other", "/clipboar"] {
        let r = rejected(check_request(&request(get, path, status.clone())));
        assert_eq!((r.status, r.correlation_id.as_str()), (S::InvalidDeviceResourceRequestPath as i32, ""), "{path:?}");
    }

    let r = rejected(check_request(&request(get, "/clipboard", Vec::new())));
    assert_eq!((r.status, r.correlation_id.as_str()), (S::UnrecognizedPayload as i32, ""));

    let r = rejected(check_request(&request(get, "/clipboard", vec![0xFF, 0xFF])));
    assert_eq!((r.status, r.correlation_id.as_str()), (S::UnrecognizedPayload as i32, ""));

    let feature_on = wire::request(ClipboardRequestType::FeatureOn, "c-2");
    let r = rejected(check_request(&request(get, "/clipboard", feature_on)));
    assert_eq!((r.status, r.correlation_id.as_str()), (S::InvalidClipboardRequestType as i32, "c-2"));

    let unknown = ClipboardRequestMessage { request_type: 99, correlation_id: String::new() }.encode_to_vec();
    let r = rejected(check_request(&request(get, "/clipboard", unknown)));
    assert_eq!(r.status, S::InvalidClipboardRequestType as i32);
    assert!(uuid::Uuid::parse_str(&r.correlation_id).is_ok(), "a fresh GUID: {}", r.correlation_id);
    assert_eq!(r.correlation_id, r.correlation_id.to_lowercase());

    assert_eq!(
        check_request(&request(get, "/clipboard?x=1", status)),
        Ok((ClipboardRequestType::Status, "c-1".to_owned())),
        "the query does not matter"
    );

    let content = wire::request(ClipboardRequestType::Content, "");
    let (t, cid) = check_request(&request(get, "/clipboard", content)).unwrap();
    assert_eq!(t, ClipboardRequestType::Content);
    assert!(uuid::Uuid::parse_str(&cid).is_ok());
}
