use super::*;

#[test]
fn a_send_permission_answer_says_whether_the_phone_holds_the_permission() {
    let cases = [
        (PermissionResult::Granted, Some(true)),
        (PermissionResult::Required, Some(false)),
        (PermissionResult::RequestStatus(2), None),
        (PermissionResult::ParseError, None),
        (PermissionResult::ResponseStatus(DrmSendOutcome::ResponseTimeout), None),
        (PermissionResult::ResponseStatus(DrmSendOutcome::Canceled), None),
    ];

    for (result, want) in cases {
        assert_eq!(permission_held(&result), want, "{result:?}");
    }
}
