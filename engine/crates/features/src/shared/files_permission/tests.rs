use super::*;

fn reply(status: PermissionsRequestStatus, files: bool) -> DrmSendOutcome {
    let payload = PermissionsResponseMessage {
        request_status: status as i32,
        feature_permissions: if files { vec![FeaturePermissions::Files as i32] } else { vec![] },
        ..Default::default()
    }
    .encode_to_vec();
    DrmSendOutcome::Success { payload }
}

#[test]
fn the_request_asks_for_files() {
    assert_eq!(request(), [0x0a, 0x01, 0x02]);
}

#[test]
fn replies_read_as_their_result() {
    let read = PermissionResult::from_outcome;

    assert_eq!(read(reply(PermissionsRequestStatus::Accepted, true)), PermissionResult::Granted);
    assert_eq!(read(reply(PermissionsRequestStatus::Accepted, false)), PermissionResult::Required);
    assert_eq!(read(reply(PermissionsRequestStatus::OtherFailure, true)), PermissionResult::RequestStatus(3));
    assert_eq!(read(DrmSendOutcome::Success { payload: vec![0xff] }), PermissionResult::ParseError);
    assert_eq!(
        read(DrmSendOutcome::InvalidRequest("x")),
        PermissionResult::ResponseStatus(DrmSendOutcome::InvalidRequest("x"))
    );
}
