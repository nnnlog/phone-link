use super::*;

fn entry(index: u32, incoming: bool, status: u8, number: Option<&str>) -> ClccEntry {
    ClccEntry { index, incoming, status, number: number.map(str::to_string) }
}

#[test]
fn an_incoming_call_takes_the_clip_number_and_starts_talking() {
    let mut book = CallBook::default();
    book.set_caller(Some("+15550100".into()));
    book.apply(&[entry(1, true, 4, None)], 10);

    let call = book.calls()[0].clone();
    assert_eq!(
        (call.status, call.number.as_str(), call.role),
        (CallStatus::Incoming, "+15550100", CallRole::Untracked)
    );
    assert!(book.track(call.id).is_none());

    book.apply(&[entry(1, true, 0, None)], 20);
    let id = call.id;
    let call = book.get(id).unwrap();
    assert_eq!((call.status, call.talking_since_ms, call.role), (CallStatus::Talking, Some(20), CallRole::Primary));

    book.apply(&[entry(1, true, 0, None)], 30);
    assert_eq!(book.get(id).unwrap().talking_since_ms, Some(20), "the start stays");
}

#[test]
fn a_call_ends_when_its_index_leaves_the_list() {
    let mut book = CallBook::default();
    book.apply(&[entry(1, false, 0, Some("1"))], 0);
    let id = book.calls()[0].id;
    book.apply(&[], 5);
    assert!(book.get(id).is_none());

    book.apply(&[entry(1, false, 2, Some("2"))], 6);
    assert_ne!(book.calls()[0].id, id, "a reused index is a new call");
}

#[test]
fn the_dialed_call_is_tracked_and_older_calls_are_not() {
    let mut book = CallBook::default();
    book.apply(&[entry(1, false, 0, Some("1"))], 0);
    book.expect_dialed();
    book.apply(&[entry(1, false, 1, Some("1")), entry(2, false, 2, Some("2"))], 1);

    let calls = book.calls();
    assert_eq!(calls[0].role, CallRole::Untracked);
    assert_eq!((calls[1].role, calls[1].status), (CallRole::Primary, CallStatus::Dialing));
}

#[test]
fn a_waiting_call_answered_demotes_the_primary_and_resumes_it_when_it_ends() {
    let mut book = CallBook::default();
    book.expect_dialed();
    book.apply(&[entry(1, false, 0, Some("1"))], 0);
    let first = book.calls()[0].id;

    book.set_caller(Some("2".into()));
    book.apply(&[entry(1, false, 0, Some("1")), entry(2, true, 5, None)], 1);
    let waiting = book.calls()[1].clone();
    assert!(waiting.waiting);
    assert_eq!(waiting.number, "2");
    assert!(book.track(waiting.id).is_none());

    book.apply(&[entry(1, false, 1, Some("1")), entry(2, true, 0, Some("2"))], 2);
    assert_eq!(book.get(first).unwrap().role, CallRole::Secondary);
    assert_eq!(book.get(waiting.id).unwrap().role, CallRole::Primary);

    let follow = book.apply(&[entry(1, false, 1, Some("1"))], 3);
    assert_eq!(follow, vec![FollowUp::Resume(first)]);
    assert_eq!(book.get(first).unwrap().role, CallRole::Primary);
}

#[test]
fn a_third_tracked_call_ends_the_secondary() {
    let mut book = CallBook::default();
    book.apply(&[entry(1, true, 0, Some("1")), entry(2, true, 1, Some("2")), entry(3, true, 5, Some("3"))], 0);
    let ids: Vec<u64> = book.calls().iter().map(|c| c.id).collect();
    assert_eq!(book.track(ids[1]), None);
    assert_eq!(book.track(ids[0]), None);
    assert_eq!(book.track(ids[2]), Some(FollowUp::End(ids[1])));
    assert_eq!(book.get(ids[0]).unwrap().role, CallRole::Secondary);

    book.swap_roles();
    assert_eq!(book.get(ids[0]).unwrap().role, CallRole::Primary);
}

#[test]
fn unknown_states_are_skipped_and_clear_ends_everything() {
    let mut book = CallBook::default();
    book.apply(&[entry(1, true, 9, None), entry(2, false, 3, Some("5"))], 0);
    assert_eq!(book.calls().len(), 1);
    assert_eq!(book.calls()[0].status, CallStatus::Dialing);
    book.clear();
    assert!(book.calls().is_empty());
}
