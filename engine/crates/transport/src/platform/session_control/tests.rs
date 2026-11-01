use super::*;
use phonelink_proto::platform_sdk::v1::PlatformDisconnectRequestReason;

#[test]
fn reason_mapping() {
    let encode = |reason: i32| PlatformDisconnectRequest { trace_context: None, reason }.encode_to_vec();

    assert_eq!(disconnect_reason(&encode(PlatformDisconnectRequestReason::UserRequested as i32)), 1);
    assert_eq!(disconnect_reason(&encode(PlatformDisconnectRequestReason::TwoPhoneModeEnabled as i32)), 6);
    assert_eq!(disconnect_reason(&encode(17)), 0);
    assert_eq!(disconnect_reason(b"{}"), 0, "the PC's own body fails the protobuf parser");
    assert_eq!(disconnect_reason(&[]), 0);

    assert!(is_persistent_disconnect_reason(0));
    assert!(is_persistent_disconnect_reason(4));
    assert!(!is_persistent_disconnect_reason(5));
    assert!(!is_persistent_disconnect_reason(6));
}
