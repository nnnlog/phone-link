//! Unit tests of the pairing view's pages, limits and effects.

use super::*;

fn code(value: &str) -> PairingCode {
    PairingCode { value: value.to_owned(), expires_at: 1_800_000_000, expires_in_minutes: 10 }
}

fn update(session: u64, step: PairingStep) -> PairingUpdate {
    PairingUpdate { session, step }
}

fn scan(session: u64) -> PairingUpdate {
    update(session, PairingStep::WaitingForScan { qr_code: code("qr"), manual_link: "https://link".to_owned() })
}

#[test]
fn a_started_session_loads_with_its_limit() {
    let mut pairing = Pairing::default();
    let effects = pairing.started(PairingFlow::QrCode, Ok(7));
    assert_eq!(pairing.page(), &Page::Loading);
    assert_eq!(effects.timer.map(|t| t.after), Some(LOADING_LIMIT));
}

#[test]
fn a_session_that_does_not_start_shows_why() {
    let mut pairing = Pairing::default();
    pairing.started(PairingFlow::QrCode, Err("not connected".to_owned()));
    assert_eq!(pairing.json()["page"], "failed");
    assert_eq!(pairing.json()["message"], "not connected");
}

#[test]
fn steps_of_another_session_are_ignored() {
    let mut pairing = Pairing::default();
    pairing.started(PairingFlow::QrCode, Ok(7));
    assert_eq!(pairing.apply(scan(6)), Effects::default());
    assert_eq!(pairing.page(), &Page::Loading);
}

#[test]
fn the_manual_instructions_stay_up_while_the_qr_session_goes_on() {
    let mut pairing = Pairing::default();
    pairing.started(PairingFlow::QrCode, Ok(7));
    pairing.apply(scan(7));
    pairing.show_manual(Some("someone@example.com".to_owned()));
    pairing.apply(scan(7));
    assert_eq!(pairing.json()["page"], "manualInstructions");
    assert_eq!(pairing.json()["signInWith"], "Sign in with someone@example.com.");
    assert!(pairing.back_to_qr().is_some());
    assert_eq!(pairing.json()["page"], "qrCode");
}

#[test]
fn back_to_the_qr_code_without_a_qr_session_starts_one() {
    let mut pairing = Pairing::default();
    pairing.started(PairingFlow::ManualPin, Ok(8));
    assert_eq!(pairing.back_to_qr(), None);
}

#[test]
fn a_paired_session_selects_the_phone() {
    let mut pairing = Pairing::default();
    pairing.started(PairingFlow::QrCode, Ok(7));
    let effects =
        pairing.apply(update(7, PairingStep::Ended { result: PairingResult::Success { phone_id: "p1".into() } }));
    assert_eq!(effects.paired.as_deref(), Some("p1"));
    assert_eq!(pairing.page(), &Page::Paired);
    // The session is over: a repeated step changes nothing.
    let again =
        pairing.apply(update(7, PairingStep::Ended { result: PairingResult::Success { phone_id: "p1".into() } }));
    assert_eq!(again, Effects::default());
}

#[test]
fn an_expired_session_offers_a_new_code() {
    let mut pairing = Pairing::default();
    pairing.started(PairingFlow::QrCode, Ok(7));
    pairing.apply(update(7, PairingStep::Ended { result: PairingResult::SessionExpired }));
    assert_eq!(pairing.json()["expired"], true);
    assert_eq!(pairing.json()["message"], "The code expired. Get a new one to try again.");
}

#[test]
fn a_code_that_does_not_arrive_in_time_cancels_the_session() {
    let mut pairing = Pairing::default();
    let timer = pairing.started(PairingFlow::QrCode, Ok(7)).timer.expect("a loading limit");
    let effects = pairing.limit_reached(timer.generation);
    assert!(effects.cancel_session);
    assert_eq!(pairing.page(), &Page::Failed(Failure::LoadingTookTooLong));
    // Steps of the cancelled session no longer apply.
    assert_eq!(pairing.apply(scan(7)), Effects::default());
}

#[test]
fn a_limit_of_an_earlier_page_is_ignored() {
    let mut pairing = Pairing::default();
    let timer = pairing.started(PairingFlow::QrCode, Ok(7)).timer.expect("a loading limit");
    pairing.apply(scan(7));
    assert_eq!(pairing.limit_reached(timer.generation), Effects::default());
    assert_eq!(pairing.json()["page"], "qrCode");
}

#[test]
fn a_long_wait_for_permissions_fails_but_the_session_goes_on() {
    let mut pairing = Pairing::default();
    pairing.started(PairingFlow::QrCode, Ok(7));
    let timer = pairing.apply(update(7, PairingStep::WaitingForPermissions)).timer.expect("a permissions limit");
    assert_eq!(timer.after, PERMISSIONS_LIMIT);
    // A repeated step keeps the running limit.
    assert_eq!(pairing.apply(update(7, PairingStep::WaitingForPermissions)), Effects::default());
    assert!(!pairing.limit_reached(timer.generation).cancel_session);
    assert_eq!(pairing.page(), &Page::Failed(Failure::PermissionsTookTooLong));
    pairing.apply(update(7, PairingStep::Completing));
    assert_eq!(pairing.page(), &Page::Completing);
}

#[test]
fn only_a_challenge_pin_that_no_longer_works_can_be_refreshed() {
    let mut pairing = Pairing::default();
    pairing.started(PairingFlow::QrCode, Ok(7));
    pairing.apply(update(7, PairingStep::WaitingForPin { pin: code("123456"), kind: PairingPinKind::Challenge }));
    assert!(!pairing.can_refresh_pin());
    pairing.apply(update(
        7,
        PairingStep::PinRefresh {
            pin: code("123456"),
            kind: PairingPinKind::Challenge,
            problem: PairingPinProblem::Wrong,
            refreshing: false,
            failure: Some(PairingRefreshFailure::TimeoutError),
        },
    ));
    assert!(pairing.can_refresh_pin());
    pairing.refreshing();
    assert!(!pairing.can_refresh_pin());
    assert_eq!(pairing.json()["refreshing"], true);
    assert_eq!(pairing.json()["message"], "Microsoft didn't answer in time. Try again.");
}

#[test]
fn a_cancel_returns_to_idle_and_ignores_the_session() {
    let mut pairing = Pairing::default();
    pairing.started(PairingFlow::QrCode, Ok(7));
    pairing.cancel();
    assert_eq!(pairing.page(), &Page::Idle);
    assert_eq!(pairing.apply(update(7, PairingStep::Ended { result: PairingResult::Canceled })), Effects::default());
}
