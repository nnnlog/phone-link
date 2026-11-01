use super::*;

#[tokio::test(start_paused = true)]
async fn breaker_opens_and_probes() {
    let mut b = Breaker::new(3, Duration::from_secs(60));
    for i in 0..3 {
        assert!(b.attempt());
        let change = b.record(&Err(TransferError::Failed("test".into())));
        assert_eq!(change, (i == 2).then_some(BreakerChange::Opened), "the third failure opens it");
    }

    assert!(!b.attempt());
    tokio::time::advance(Duration::from_secs(61)).await;
    assert!(b.attempt());
    assert!(!b.attempt(), "one probe at a time");
    assert_eq!(b.record(&Err(TransferError::FailedPlatformError)), None, "a failed probe re-opens it");
    assert!(!b.attempt());

    tokio::time::advance(Duration::from_secs(61)).await;
    assert!(b.attempt());
    assert_eq!(b.record(&Ok(())), Some(BreakerChange::Closed));
    assert!(b.attempt());
}

#[test]
fn unrecorded_failures_do_not_open() {
    let mut b = Breaker::new(3, Duration::from_secs(60));
    for _ in 0..5 {
        assert_eq!(b.record(&Err(TransferError::LtwOff)), None);
    }
    assert!(b.attempt());
}
