use super::*;

#[test]
fn v2_types_and_answer_bytes() {
    let v2: Vec<i32> = (0..=10).filter(|t| requires_v2(*t)).collect();
    assert_eq!(v2, [3, 4, 5, 6, 7, 8]);

    assert_eq!(answer(RemoteControlResult::Success, None), [0x08, 0x01]);
    assert_eq!(answer(RemoteControlResult::Failed, None), [0x08, 0x02]);
    assert_eq!(answer(RemoteControlResult::Disabled, None), [0x08, 0x03]);
    assert_eq!(answer(RemoteControlResult::Success, Some(false)), [0x08, 0x01, 0x10, 0x00]);

    assert_eq!(
        RemoteControlRequestMessage { control_type: RemoteControlType::Lock as i32 }.encode_to_vec(),
        [0x08, 0x01]
    );
    assert!(
        RemoteControlRequestMessage { control_type: RemoteControlType::Unspecified as i32 }.encode_to_vec().is_empty(),
        "UNSPECIFIED encodes as zero bytes"
    );
}
