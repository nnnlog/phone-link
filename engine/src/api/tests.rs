use super::*;
use crate::features::messaging::MessagingError;
use crate::features::notifications::NotificationsError;
use crate::proto::platform_sdk::v1::PlatformDisconnectRequestReason as Reason;
use crate::seams::ConnectionFailure;
use crate::types::{DcgSendStatus, HubSendResult, PartnerDisconnectReason, WakeStatus};

#[test]
fn outcomes_keep_their_kind_and_the_transport_cause_as_text() {
    assert_eq!(RequestOutcome::from(AppOutcome::Accepted), RequestOutcome::Accepted);
    assert_eq!(RequestOutcome::from(AppOutcome::TimedOut), RequestOutcome::TimedOut);
    assert_eq!(
        RequestOutcome::from(AppOutcome::ConnectionFailed(ConnectionFailure::PartnerDisconnect(
            PartnerDisconnectReason::SystemReset
        ))),
        RequestOutcome::ConnectionFailed { reason: "PartnerDisconnect(SystemReset)".into() }
    );
    assert_eq!(
        RequestOutcome::from(AppOutcome::SendFailed(DcgSendStatus::HubSendFailed {
            result: HubSendResult::Failure,
            cause: None
        })),
        RequestOutcome::SendFailed { reason: "HubSendFailed(Failure)".into() }
    );
    assert_eq!(
        RequestOutcome::from(AppOutcome::SendFailed(DcgSendStatus::WakeFailed {
            status: WakeStatus::DeviceUnreachable,
            cause: Some("the Dispatcher answered 503".into())
        })),
        RequestOutcome::SendFailed { reason: "WakeFailed(DeviceUnreachable): the Dispatcher answered 503".into() }
    );
}

/// A request that waited for a connection the phone ended for a sync reason sees what the
/// matching app-response `result` gives — *LTW disabled* (as a wake that failed with LTW
/// disabled) is *all features disabled*, as `result` 13; `METERED_SYNC_DISABLED` is *metered sync
/// disabled*, as `result` 12.
#[test]
fn sync_disabled_failures_reach_the_ui_as_their_outcome() {
    let ended = |reason: Reason| {
        RequestOutcome::from(AppOutcome::ConnectionFailed(ConnectionFailure::SessionEnded { reason: reason as i32 }))
    };

    assert_eq!(ended(Reason::AllSyncDisabled), RequestOutcome::AllFeaturesDisabled);
    assert_eq!(ended(Reason::MeteredSyncDisabled), RequestOutcome::MeteredSyncDisabled);
    assert_eq!(
        RequestOutcome::from(AppOutcome::ConnectionFailed(ConnectionFailure::Wake(WakeStatus::LtwDisabled))),
        RequestOutcome::AllFeaturesDisabled
    );
    assert_eq!(
        RequestOutcome::from(AppOutcome::SendFailed(DcgSendStatus::WakeFailed {
            status: WakeStatus::LtwDisabled,
            cause: None
        })),
        RequestOutcome::AllFeaturesDisabled
    );
    assert_eq!(RequestOutcome::from(AppOutcome::AllFeaturesDisabled), RequestOutcome::AllFeaturesDisabled);
    assert_eq!(RequestOutcome::from(AppOutcome::MeteredSyncDisabled), RequestOutcome::MeteredSyncDisabled);

    // Every other session end keeps its cause.
    assert_eq!(
        ended(Reason::UserRequested),
        RequestOutcome::ConnectionFailed {
            reason: format!("SessionEnded {{ reason: {} }}", Reason::UserRequested as i32)
        }
    );
}

#[test]
fn feature_errors_map_to_what_the_ui_can_act_on() {
    assert_eq!(FeatureApiError::from(NotificationsError::AppGone), FeatureApiError::NotStarted);
    assert_eq!(FeatureApiError::from(NotificationsError::NoPhone), FeatureApiError::NoPhone);
    assert_eq!(
        FeatureApiError::from(NotificationsError::UnknownNotification),
        FeatureApiError::UnknownItem { what: "notification".into() }
    );
    assert_eq!(
        FeatureApiError::from(MessagingError::TooManyAttachments { max: 2 }),
        FeatureApiError::InvalidInput { reason: "at most 2 attachments per message".into() }
    );
    assert_eq!(
        FeatureApiError::from(MessagingError::NotAllowed("group MMS is off")),
        FeatureApiError::NotAvailable { reason: "group MMS is off".into() }
    );
    assert!(matches!(FeatureApiError::from(StoreError::Corrupt("x".into())), FeatureApiError::Store { .. }));
}
