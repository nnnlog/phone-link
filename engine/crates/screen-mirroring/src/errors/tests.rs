use super::*;

#[test]
fn codes_and_takeovers() {
    assert_eq!(error_kind(34), (ErrorKind::AppNotAvailable, ErrorAction::Close));
    assert_eq!(error_kind(35).1, ErrorAction::AllowAndRetry);
    assert_eq!(error_kind(33), (ErrorKind::Overheated, ErrorAction::Close));
    assert_eq!(error_kind(38), (ErrorKind::PowerSaving, ErrorAction::Relaunch), "launch again");
    assert_eq!(error_kind(96).0, ErrorKind::MeteredConnection);
    assert_eq!(error_kind(37).0, ErrorKind::Generic);
    assert_eq!(error_kind(1104).0, ErrorKind::Generic);

    assert_eq!(launch_result_code("appUnavailable"), 39);
    assert_eq!(launch_result_code("somethingNew"), 32);

    assert_eq!(setup_code(808, false), 96);
    assert_eq!(setup_code(808, true), 808);

    assert_eq!(attempt_status(&AttemptError::Rejected { result: 10, reason: None }), 810);
    assert_eq!(attempt_status(&AttemptError::Connection(CloseReason::TimeoutExceeded)), 267);
    assert_eq!(attempt_status(&AttemptError::Connection(CloseReason::HostnameResolutionError)), 273);
    assert_eq!(attempt_status(&AttemptError::Transport { status: 603 }), 603);

    assert_eq!(close_code(&CloseReason::KeepAliveTimeout, None, false), 81);
    assert_eq!(close_code(&CloseReason::PeerEnded, None, true), 82);
    assert_eq!(close_code(&CloseReason::PeerEnded, None, false), 85);
    assert_eq!(close_code(&CloseReason::UserEnded, Some(84), false), 84);
    assert_eq!(close_code(&CloseReason::UserEnded, None, false), 80);
}
