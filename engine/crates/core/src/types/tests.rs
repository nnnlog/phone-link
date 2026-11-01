use super::*;

#[test]
fn a_request_lies_within_its_operations_trace() {
    let op = TraceContext::new_root(vec![("ypScenarioId".into(), "Op".into())]);
    let r = op.for_request(vec![("ypScenarioId".into(), "Request".into())]);
    assert_eq!(r.trace_id, op.trace_id);
    assert_eq!(r.parent_id.as_deref(), Some(op.correlation_id.as_str()));
    assert_ne!(r.correlation_id, op.correlation_id);
    assert_eq!(r.trace_state, vec![("ypScenarioId".to_owned(), "Request".to_owned())]);
}

#[test]
fn header_map_keeps_insertion_order_and_replaces_in_place() {
    let mut h = HeaderMap::new();
    h.insert("_route", "/a");
    h.insert("_requestId", "1");
    h.insert("_route", "/b");
    let pairs: Vec<_> = h.iter().collect();
    assert_eq!(pairs, vec![("_route", "/b"), ("_requestId", "1")]);
    assert!(HeaderMap::from_pairs([("a", "1"), ("a", "2")]).is_err());
}

#[test]
fn trace_context_renders_headers() {
    let ctx = TraceContext {
        trace_id: "0123456789abcdef0123456789abcdef".into(),
        parent_id: None,
        correlation_id: "0011223344556677".into(),
        flags: 0,
        trace_state: vec![("ypScenarioId".into(), "S".into())],
    };

    assert_eq!(ctx.traceparent(), "00-0123456789abcdef0123456789abcdef-0011223344556677-00");
    assert_eq!(ctx.tracestate(), "ypScenarioId=S,yppHttpParent=0011223344556677");

    let child = ctx.child();
    assert_eq!(child.trace_id, ctx.trace_id);
    assert_eq!(child.parent_id.as_deref(), Some("0011223344556677"));
    assert_eq!(child.correlation_id.len(), 16);

    let id = hex::decode(random_trace_id()).unwrap();
    assert_eq!(id.len(), 16);
    assert!(id.iter().all(|b| *b != 0), "every byte of a trace id is non-zero");
    assert!(hex::decode(&child.correlation_id).unwrap().iter().all(|b| *b != 0));
}

#[test]
fn message_type_receive_rule() {
    assert_eq!(MessageType::from_wire(Some(1)), MessageType::Platform);
    assert_eq!(MessageType::from_wire(Some(0)), MessageType::App);
    assert_eq!(MessageType::from_wire(Some(2)), MessageType::App);
    assert_eq!(MessageType::from_wire(None), MessageType::App);
}

#[test]
fn device_certificates_reject_unknown_keys() {
    let ok: DeviceCertificates = serde_json::from_str(r#"{"SelfSigned":["AA=="]}"#).unwrap();
    assert_eq!(ok.self_signed.unwrap(), vec!["AA==".to_string()]);
    assert!(serde_json::from_str::<DeviceCertificates>(r#"{"Other":[]}"#).is_err());
}

/// A failed open or wake keeps its cause: `Debug`, which logs show under the public name `status`,
/// leaves it out; `Display` and `cause` give it.
#[test]
fn a_send_status_keeps_the_cause_out_of_its_debug() {
    let opened = Opened { status: OpenStatus::InternetError, cause: Some("connect refused".into()) };
    let status = DcgSendStatus::open_failed(opened);
    assert_eq!(
        status,
        DcgSendStatus::HubSendFailed { result: HubSendResult::InternetError, cause: Some("connect refused".into()) }
    );

    assert_eq!(format!("{status:?}"), "HubSendFailed(InternetError)");
    assert_eq!(status.to_string(), "HubSendFailed(InternetError): connect refused");
    assert_eq!(status.cause(), Some("connect refused"));

    assert_eq!(DcgSendStatus::open_failed(Opened::from(OpenStatus::Cancelled)), DcgSendStatus::Canceled);
    let woken = DcgSendStatus::wake_failed(Woken::from(WakeStatus::DeviceUnreachable));
    assert_eq!(woken.to_string(), "WakeFailed(DeviceUnreachable)");
    assert_eq!(woken.cause(), None);
}
