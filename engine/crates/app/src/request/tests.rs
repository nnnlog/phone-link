use super::*;

fn std_keys() -> StandardKeys {
    StandardKeys {
        display_name: "mac".into(),
        app_version: "1.26082.121.0".into(),
        installation_id: "0F8FAD5B-D9CB-469F-A165-70867728950E".into(),
        contract_version: 3.99,
    }
}

#[test]
fn routes_this_layer_must_not_build() {
    for r in ["", "/", "x", "/internal/response", "/DeviceResourceManager", "/legacy/phonecontent"] {
        let req = AppRequest::kvp(r, ValueSet::new());
        assert!(prepare(req, &std_keys(), false, Duration::from_secs(120)).is_err(), "{r}");
    }
}

#[test]
fn headers_are_request_id_then_route() {
    let p = prepare(AppRequest::kvp("/x", ValueSet::new()), &std_keys(), true, Duration::from_secs(5)).unwrap();

    let pairs: Vec<_> = p.message.headers.iter().collect();
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0].0, "_requestId");
    assert_eq!(pairs[0].1.len(), 36);
    assert_eq!(pairs[0].1, pairs[0].1.to_lowercase());
    assert_eq!(pairs[1], ("_route", "/x"));
    assert_eq!(p.timeout, Duration::from_secs(5));
    assert_eq!(p.priority, 100);
    assert_eq!(p.trace.trace_state[3], ("isForeground".to_string(), "True".to_string()));
}
