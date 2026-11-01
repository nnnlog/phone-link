use super::*;

#[test]
fn failure_reasons_round_trip() {
    for v in -2..25 {
        assert_eq!(SendMessageFailureReason::from_i32(v).as_i32(), v);
    }
    assert_eq!(SendMessageFailureReason::from_i32(11), SendMessageFailureReason::Unknown);
    assert_eq!(SendMessageFailureReason::from_i32(19), SendMessageFailureReason::BluetoothNotConnected);
}

#[test]
fn rcs_types_without_support() {
    assert_eq!(RCS_UNSUPPORTED_MESSAGE_TYPES, [10, 9]);
}
