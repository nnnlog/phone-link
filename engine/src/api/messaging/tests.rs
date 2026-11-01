use super::*;

#[test]
fn failure_reasons_map_by_code_and_keep_unknown_codes() {
    assert_eq!(SendFailure::from(1), SendFailure::AirplaneMode);
    assert_eq!(SendFailure::from(19), SendFailure::BluetoothNotConnected);
    assert_eq!(SendFailure::from(77), SendFailure::Other { code: 77 });
    assert_eq!(MessageSendState::from(SendStatus::Failed(2)), MessageSendState::Failed { reason: SendFailure::NoSim });
}
