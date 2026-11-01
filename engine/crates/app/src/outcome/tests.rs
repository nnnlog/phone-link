use super::*;

#[test]
fn result_table() {
    let o = |r| outcome_for_result(r, 3.99, Some(3.5));
    for r in [0, 10, 11, 16] {
        assert_eq!(o(r), AppOutcome::Success, "{r}");
    }

    assert_eq!(o(1), AppOutcome::Failure);
    assert_eq!(o(4), AppOutcome::Accepted);
    assert_eq!(o(5), AppOutcome::Denied);
    assert_eq!(o(6), AppOutcome::PermanentlyDenied);
    assert_eq!(o(7), AppOutcome::NeedsPermission);
    assert_eq!(o(9), AppOutcome::NeedsInitialPermission);
    assert_eq!(o(8), AppOutcome::MeteredConnection);
    assert_eq!(o(12), AppOutcome::MeteredSyncDisabled);
    assert_eq!(o(13), AppOutcome::AllFeaturesDisabled);
    assert_eq!(o(14), AppOutcome::InvalidRequest);
    assert_eq!(o(15), AppOutcome::DatabaseFailure);
    assert_eq!(o(18), AppOutcome::DeviceNotSupported);
    assert_eq!(o(20), AppOutcome::AppLaunched);
    assert_eq!(o(21), AppOutcome::TwoPhoneMode);
    assert_eq!(o(23), AppOutcome::NonDefaultProfile);

    for r in [2, 17, 19, 22, -1, 24, 99, i32::MIN] {
        assert_eq!(o(r), AppOutcome::Unknown, "{r}");
    }
}

#[test]
fn incompatible_versions_compare_integer_parts() {
    assert_eq!(outcome_for_result(3, 3.99, Some(3.0)), AppOutcome::Success);
    assert_eq!(outcome_for_result(3, 3.99, Some(3.999)), AppOutcome::Success);
    assert_eq!(outcome_for_result(3, 3.99, Some(2.99)), AppOutcome::PhoneContractIncompatible);
    assert_eq!(outcome_for_result(3, 3.99, None), AppOutcome::PhoneContractIncompatible);
    assert_eq!(outcome_for_result(3, 3.99, Some(4.0)), AppOutcome::PcContractIncompatible);
}

#[test]
fn platform_statuses() {
    use PlatformRequestStatus as S;
    for s in [
        S::NoResponse,
        S::RemoteSyncNotAllowed,
        S::RemoteVersionTooLow,
        S::RemoteVersionTooHigh,
        S::RemoteAllSyncDisabled,
        S::RemoteMeteredSyncDisabled,
        S::RemoteNotSubscribedToRoute,
    ] {
        assert_eq!(outcome_for_platform_status(&s), AppOutcome::Unknown);
    }

    assert_eq!(outcome_for_platform_status(&S::Canceled), AppOutcome::Canceled);
    assert_eq!(outcome_for_platform_status(&S::ResponseTimedOut), AppOutcome::TimedOut);

    assert_eq!(
        outcome_for_platform_status(&S::RequestSendFailed(DcgSendStatus::HubSendFailed {
            result: HubSendResult::InternetError,
            cause: None
        })),
        AppOutcome::InternetError
    );
    assert_eq!(
        outcome_for_platform_status(&S::RequestSendFailed(DcgSendStatus::PartnerNotConnected)),
        AppOutcome::SendFailed(DcgSendStatus::PartnerNotConnected)
    );

    assert!(
        AppOutcome::SendFailed(DcgSendStatus::WakeFailed { status: WakeStatus::LtwDisabled, cause: None })
            .is_ltw_disabled()
    );
    assert!(outcome_for_connection_failure(ConnectionFailure::Wake(WakeStatus::LtwDisabled)).is_ltw_disabled());
    assert_eq!(
        outcome_for_connection_failure(ConnectionFailure::Hub(OpenStatus::InternetError)),
        AppOutcome::InternetError
    );
}

#[test]
fn a_session_the_phone_ended_for_sync_reasons_is_ltw_or_metered_disabled() {
    let ended = |reason: PlatformDisconnectRequestReason| {
        outcome_for_connection_failure(ConnectionFailure::SessionEnded { reason: reason as i32 })
    };

    let all = ended(PlatformDisconnectRequestReason::AllSyncDisabled);
    assert!(all.is_ltw_disabled() && !all.is_metered_sync_disabled());

    let metered = ended(PlatformDisconnectRequestReason::MeteredSyncDisabled);
    assert!(metered.is_metered_sync_disabled() && !metered.is_ltw_disabled());
    assert!(AppOutcome::MeteredSyncDisabled.is_metered_sync_disabled());

    for other in [
        PlatformDisconnectRequestReason::Unspecified,
        PlatformDisconnectRequestReason::UserRequested,
        PlatformDisconnectRequestReason::ActiveSessionReplaced,
        PlatformDisconnectRequestReason::TwoPhoneModeEnabled,
    ] {
        let o = ended(other);
        assert!(!o.is_ltw_disabled() && !o.is_metered_sync_disabled(), "{other:?}");
    }

    // A rejection is still *unknown* (the reason does not reach the feature).
    assert!(!outcome_for_platform_status(&PlatformRequestStatus::RemoteMeteredSyncDisabled).is_metered_sync_disabled());
}
