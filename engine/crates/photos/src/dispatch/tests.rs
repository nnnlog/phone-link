use super::*;

fn t() -> TraceContext {
    TraceContext::new_root(Vec::new())
}

const TRIGGER: &str = "Test";

#[test]
fn first_id_of_a_call_ends_on_top_and_readding_moves_up() {
    let mut p = Pool::default();
    p.add(&[1, 2, 3], TRIGGER, t);
    assert_eq!(p.ids(), [1, 2, 3]);

    p.add(&[4], TRIGGER, t);
    assert_eq!(p.ids(), [4, 1, 2, 3]);

    let lacking = HashSet::from([1, 2, 3, 4]);
    let (picked, _) = p.pick(&lacking);
    assert_eq!(picked.map(|x| x.id), Some(4));

    let cv = p.entry(4).unwrap().trace.correlation_id.clone();
    // Re-adding keeps the phase, the trace and the trigger.
    p.add(&[2, 4], "Other", t);
    assert_eq!(p.ids(), [2, 4, 1, 3]);
    assert_eq!(p.entry(4).unwrap().phase, Phase::Requested);
    assert_eq!(p.entry(4).unwrap().trace.correlation_id, cv);
    assert_eq!(p.entry(4).unwrap().trigger, TRIGGER);
    assert_eq!(p.requested_count(), 1);
}

#[test]
fn pick_skips_requested_and_drops_passed_over_queued() {
    let mut p = Pool::default();
    p.add(&[1, 2, 3, 4, 5], TRIGGER, t);
    let (picked, dropped) = p.pick(&HashSet::from([1]));
    assert_eq!(picked.map(|x| x.id), Some(1));
    assert!(dropped.is_empty());

    // 1 is requested (skipped); 2 and 3 are passed over and dropped; 4 is picked; 5 stays.
    let (picked, dropped) = p.pick(&HashSet::from([4, 5]));
    assert_eq!(picked.map(|x| x.id), Some(4));
    assert_eq!(dropped, [2, 3]);
    assert_eq!(p.ids(), [1, 4, 5]);

    let (picked, dropped) = p.pick(&HashSet::new());
    assert_eq!((picked.map(|x| x.id), dropped), (None, vec![5]));
    assert_eq!(p.ids(), [1, 4]);
    assert!(!p.has_queued());
}

#[test]
fn the_fourth_failure_gives_up() {
    let mut p = Pool::default();
    p.add(&[7], TRIGGER, t);
    let lacking = HashSet::from([7]);
    for _ in 0..3 {
        assert!(p.pick(&lacking).0.is_some());
        assert_eq!(p.fail(7, 2), Failed::Requeued);
        assert_eq!(p.entry(7).unwrap().phase, Phase::Queued);
    }

    assert!(p.pick(&lacking).0.is_some());
    assert_eq!(p.fail(7, 2), Failed::GaveUp);
    assert!(p.entry(7).is_none());
    assert_eq!(p.fail(7, 2), Failed::NotQueued);
}

#[test]
fn stop_all_outcomes() {
    use phonelink_core::seams::ConnectionFailure;
    use phonelink_core::types::{DcgSendStatus, WakeStatus};
    for o in [
        AppOutcome::NeedsPermission,
        AppOutcome::NeedsInitialPermission,
        AppOutcome::MeteredSyncDisabled,
        AppOutcome::AllFeaturesDisabled,
        AppOutcome::TwoPhoneMode,
        AppOutcome::NonDefaultProfile,
        AppOutcome::SendFailed(DcgSendStatus::WakeFailed { status: WakeStatus::LtwDisabled, cause: None }),
        AppOutcome::ConnectionFailed(ConnectionFailure::Wake(WakeStatus::LtwDisabled)),
    ] {
        assert!(is_stop_all(&o), "{o:?}");
    }

    for o in [AppOutcome::Failure, AppOutcome::TimedOut, AppOutcome::Denied, AppOutcome::Accepted, AppOutcome::Canceled]
    {
        assert!(!is_stop_all(&o), "{o:?}");
    }
}

#[test]
fn pause_doubles_and_caps() {
    let (min, max) = (Duration::from_secs(3), Duration::from_secs(60));
    assert_eq!(pause(min, max, 0, None), Duration::ZERO);
    let now = Instant::now();
    assert_eq!(pause(Duration::ZERO, Duration::ZERO, 5, Some(now)), Duration::ZERO);
    assert!(pause(min, max, 1, Some(now)) <= Duration::from_secs(6));
    assert!(pause(min, max, 1, Some(now)) > Duration::from_secs(5));
    assert!(pause(min, max, 40, Some(now)) <= max);
}
