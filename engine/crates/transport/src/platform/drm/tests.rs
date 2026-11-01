use super::*;

#[test]
fn inbound_mappings() {
    use ProtoDeviceResourceRequestType as R;
    use ProtoDeviceResourceType as T;

    assert_eq!(inbound_request_type(0), Err(DrmFault::RequestType(0)));
    assert_eq!(inbound_request_type(1), Ok(R::Get));
    assert_eq!(inbound_request_type(4), Ok(R::Sync));
    assert_eq!(inbound_request_type(5), Err(DrmFault::RequestType(5)));

    assert_eq!(inbound_resource_type(0), Ok(T::Unknown));
    assert_eq!(inbound_resource_type(4), Ok(T::Unknown));
    assert_eq!(inbound_resource_type(7), Err(DrmFault::ResourceType(7)));
    assert_eq!(inbound_resource_type(8), Err(DrmFault::ResourceType(8)));
    assert_eq!(inbound_resource_type(11), Err(DrmFault::ResourceType(11)));
    assert_eq!(inbound_resource_type(9), Ok(T::FileShare));

    assert_eq!(resource_type_path(T::FilesMetadata), "/filesmetadata");
    assert_eq!(resource_type_path(T::RingMyPhone), "/ringmyphone");
    assert_eq!(resource_type_path(T::Unknown), "/unknown");
}

#[test]
fn relative_uri_check() {
    for ok in ["/clipboard", "/appcontext?since=5", "/phoneapps?since=12", "/a%20b", "clipboard", "/x#frag"] {
        assert!(is_well_formed_relative_uri(ok), "{ok}");
    }

    for bad in ["", "https://example.com/x", "mailto:x", "//host/x", "/a b", "/a%2", "/a%zz", "\\x", "/ä"] {
        assert!(!is_well_formed_relative_uri(bad), "{bad}");
    }
}

#[test]
fn response_builder() {
    let m = response_message(vec![1, 2]);
    assert_eq!(m.headers.get("ms-content-type"), Some("application/x-binary"));

    let decoded = DeviceResourceManagerResponseMessage::decode(m.payload.as_slice()).unwrap();
    assert_eq!(decoded.payload, vec![1, 2]);
    assert_eq!(decoded.response_type, Some(1));
}

#[test]
fn a_success_outcome_shows_its_payload_by_length_only() {
    let outcome = DrmSendOutcome::Success { payload: b"phone content".to_vec() };
    assert_eq!(format!("{outcome:?}"), "Success { payload_len: 13 }");
}
