use super::*;

fn refused(reason: AcceptanceReason) -> ChannelOpenError {
    ChannelOpenError::Channel(PlatformChannelError::RemoteScenarioRejectedChannel(reason))
}

/// The kind of a send failure, for comparing without the channel error it carries.
fn kind(e: &SendError) -> &'static str {
    match e {
        SendError::AnotherTransferInProgress => "another transfer",
        SendError::Metered => "metered",
        SendError::ChannelRejected(_) => "rejected",
        SendError::ChannelFailed(_) => "failed",
        SendError::ChannelTimedOut => "timed out",
        _ => "other",
    }
}

#[test]
fn channel_refusals_map_to_send_failures() {
    // A status the Mac does not know, such as the phone's context source being dropped (8), reaches
    // the feature as `Unspecified`.
    let cases = [
        (refused(AcceptanceReason::NotSupported), false, true, "another transfer"),
        (refused(AcceptanceReason::NotSupported), false, false, "rejected"),
        (refused(AcceptanceReason::Unspecified), false, true, "rejected"),
        (refused(AcceptanceReason::PermissionRevoked), false, true, "rejected"),
        (refused(AcceptanceReason::AppDisabled), false, true, "rejected"),
        (refused(AcceptanceReason::Unspecified), true, true, "metered"),
        (refused(AcceptanceReason::NotSupported), true, true, "another transfer"),
        (ChannelOpenError::Channel(PlatformChannelError::CreateRequestTimedOut), false, true, "timed out"),
        (ChannelOpenError::Channel(PlatformChannelError::Timeout), false, true, "failed"),
        (ChannelOpenError::Channel(PlatformChannelError::DeviceConnectionFailed), false, true, "rejected"),
        (ChannelOpenError::Channel(PlatformChannelError::DeviceConnectionFailed), true, true, "metered"),
    ];

    for (e, metered, send, want) in cases {
        let what = format!("{e:?} metered={metered} send={send}");
        assert_eq!(kind(&channel_failure(e, metered, send)), want, "{what}");
    }
}
