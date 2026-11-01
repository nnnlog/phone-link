use super::*;
use phonelink_proto::platform_sdk::v1::{CsConnectionInfo, CsErrorInfo, CsIdentityChallenge};

fn trace(id: &str) -> Option<ProtoTraceContext> {
    Some(ProtoTraceContext { trace_id: id.into(), ..Default::default() })
}

const ID: &str = "0af7651916cd43dd8448eb211c80319c";

fn status(t: Option<Type>) -> ReplyStatus {
    handling_status(&ContextSourceMessage { r#type: t }.encode_to_vec())
}

#[test]
fn trace_ids() {
    assert!(is_w3c_trace_id(ID));
    assert!(!is_w3c_trace_id(&"0".repeat(32)), "all zeros");
    assert!(!is_w3c_trace_id(&ID[..31]));
    assert!(!is_w3c_trace_id(&format!("{}-", &ID[..31])));
}

#[test]
fn statuses() {
    assert_eq!(handling_status(&[0xFF]), ReplyStatus::Error, "does not parse");
    assert_eq!(status(None), ReplyStatus::Success, "no case");

    let error_info =
        |t| Some(Type::CsErrorInfo(CsErrorInfo { context_source_error_type: "X".into(), trace_context: t }));
    assert_eq!(status(error_info(trace(ID))), ReplyStatus::Success);
    assert_eq!(status(error_info(None)), ReplyStatus::Error, "no trace context");
    assert_eq!(status(error_info(trace("abc"))), ReplyStatus::Error, "not a W3C id");

    let info = |text: &str| {
        Some(Type::CsConnectionInfo(CsConnectionInfo { connection_info: text.into(), trace_context: trace(ID) }))
    };
    assert_eq!(status(info("{\"connectionid\":\"1\"}")), ReplyStatus::Success);
    assert_eq!(status(info("not json")), ReplyStatus::Success, "a parse failure is handled");
    assert_eq!(status(info("")), ReplyStatus::Error);
    assert_eq!(status(info(" null ")), ReplyStatus::Error);

    let challenge = Some(Type::CsIdentityChallenge(CsIdentityChallenge::default()));
    assert_eq!(status(challenge), ReplyStatus::Success, "unknown case");
}
