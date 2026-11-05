use super::*;

#[test]
fn link_to_windows_off_is_read_from_the_request_outcome() {
    let wake = |status| {
        DrmSendOutcome::Request(PlatformRequestStatus::RequestSendFailed(DcgSendStatus::WakeFailed {
            status,
            cause: None,
        }))
    };

    let cases = [
        (DrmSendOutcome::Request(PlatformRequestStatus::RemoteAllSyncDisabled), true),
        (wake(WakeStatus::LtwDisabled), true),
        (wake(WakeStatus::DeviceUnreachable), false),
        (DrmSendOutcome::Request(PlatformRequestStatus::RemoteMeteredSyncDisabled), false),
        (DrmSendOutcome::Request(PlatformRequestStatus::ResponseTimedOut), false),
        (DrmSendOutcome::ResponseTimeout, false),
        (DrmSendOutcome::UserDisabled, false),
        (DrmSendOutcome::Success { payload: Vec::new() }, false),
    ];

    for (outcome, want) in cases {
        assert_eq!(ltw_disabled(&outcome), want, "{outcome:?}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_handle_arrival_ends_only_on_a_new_value() {
    let (tx, mut rx) = watch::channel(0u64);
    let waiting = tokio::spawn(async move {
        handle_arrival(&mut rx).await;
        rx
    });

    tokio::task::yield_now().await;
    assert!(!waiting.is_finished(), "nothing arrived yet");

    tx.send_modify(|n| *n += 1);
    let rx = waiting.await.expect("the wait ends");
    assert!(!rx.has_changed().expect("the sender lives"), "the arrival was taken");
}

#[tokio::test(start_paused = true)]
async fn a_handle_arrival_never_ends_once_the_phone_is_gone() {
    let (tx, mut rx) = watch::channel(0u64);
    drop(tx);

    let ended = tokio::time::timeout(std::time::Duration::from_secs(3600), handle_arrival(&mut rx)).await;
    assert!(ended.is_err(), "a gone phone delivers no handle");
}
