use super::*;
use phonelink_app::RequestKind;
use phonelink_app::consts::content_type;

#[test]
fn record_types_are_in_processing_order_with_catalog_batch_names() {
    let order: Vec<_> = RecordType::ALL.iter().map(|t| t.sequence_key()).collect();
    assert_eq!(order, ["contact", "phonenumber", "emailaddress", "postaladdress", "contactdate", "contacturl"]);
    for (i, t) in RecordType::ALL.iter().enumerate() {
        assert_eq!(t.index(), i);
    }

    // The sub-record batch keys are the catalog sub-types; `contactsV2` is in the catalog.
    assert!(content_type::is_catalog_name(CONTENT_TYPE));
    for t in &RecordType::ALL[1..] {
        assert!(content_type::is_catalog_name(t.batch_key()), "{}", t.batch_key());
    }
    for s in SUBCONTENT_TYPES {
        assert!(content_type::is_catalog_name(s), "{s}");
    }
}

#[test]
fn sync_request_shape() {
    let trace = TraceContext::new_root(Vec::new());
    let cv = trace.correlation_id.clone();
    let r = sync_request(trace, RequestOptions::default());

    assert_eq!(r.route, "/contacts/v2");
    assert_eq!(r.kind, RequestKind::Legacy);
    assert_eq!(r.trace.as_ref().map(|t| t.correlation_id.as_str()), Some(cv.as_str()));

    assert_eq!(
        r.payload.keys().collect::<Vec<_>>(),
        ["correlationVector", "contentType", "subcontentTypes", "configuration"]
    );
    assert_eq!(r.payload.str("correlationVector"), Some(cv.as_str()));
    assert_eq!(
        r.payload.string_array("subcontentTypes").unwrap(),
        [
            "contactsV2",
            "contactThumbs",
            "phoneNumbers",
            "emailAddresses",
            "postalAddresses",
            "contactDates",
            "contactUrls"
        ]
    );

    let config = r.payload.object("configuration").unwrap();
    assert_eq!(config.keys().collect::<Vec<_>>(), ["ContactsV2Sync", "ContactThumbnails"]);
    assert_eq!(config.bool("ContactsV2Sync"), Some(true));
    assert_eq!(config.bool("ContactThumbnails"), Some(true));
}

#[test]
fn enums_map_their_values() {
    assert_eq!(PhoneNumberType::from_i32(8), Some(PhoneNumberType::PhoneCustom));
    assert_eq!(PhoneNumberType::from_i32(9), None);
    assert_eq!(EmailAddressType::from_i32(6), Some(EmailAddressType::Mobile));
    assert_eq!(PostalAddressType::from_i32(5), Some(PostalAddressType::Custom));
    assert_eq!(ContactDateType::from_i32(1), Some(ContactDateType::Birthday));
    assert_eq!(ContactUrlType::from_i32(1), Some(ContactUrlType::Homepage));
    assert_eq!(ContactUrlType::from_i32(-1), None);
}
