use phonelink_features::shared::db::{FeatureDb, StoreLocation};

use super::*;
use crate::wire::CallEntry;

fn db() -> FeatureDb {
    FeatureDb::open_global(&StoreLocation::Memory, FEATURE, MIGRATIONS).unwrap()
}

fn call(id: i64, call_type: i32, date: i64, last_updated: i64) -> CallEntry {
    CallEntry {
        id,
        number: format!("+1555010{id}"),
        duration: 10,
        call_type,
        date,
        read: false,
        phone_account_id: String::new(),
        last_updated,
    }
}

fn store(db: &FeatureDb, upserts: Vec<CallEntry>) {
    db.transaction(|tx| apply_items(tx, &Items { upserts, deletes: Vec::new(), sequence: None })).unwrap();
}

#[test]
fn synchronize_deletes_the_unlisted_calls_and_asks_for_new_or_changed_ones() {
    let db = db();
    store(&db, vec![call(1, 1, 100, 7), call(2, 1, 200, 7), call(3, 1, 300, 7)]);

    let only_changed = db.transaction(|tx| synchronize(tx, &[2, 3, 4], &[7, 8, 9], false)).unwrap();
    assert_eq!(only_changed, Synchronized { deleted: 1, wanted: vec![3, 4] });
    assert_eq!(history(&db).unwrap().iter().map(|c| c.id).collect::<Vec<_>>(), vec![3, 2]);

    let every = db.transaction(|tx| synchronize(tx, &[2, 3], &[7, 8], true)).unwrap();
    assert_eq!(every, Synchronized { deleted: 0, wanted: vec![2, 3] });
}

#[test]
fn an_empty_id_list_deletes_every_call() {
    let db = db();
    store(&db, vec![call(1, 1, 100, 7)]);

    db.transaction(|tx| synchronize(tx, &[], &[], true)).unwrap();

    assert!(is_empty(&db).unwrap(), "a call is left");
}

#[test]
fn an_item_upsert_replaces_the_row_and_a_delete_removes_it() {
    let db = db();
    store(&db, vec![call(1, 1, 100, 7), call(2, 2, 200, 7)]);

    let changed = CallEntry { number: "+15550199".into(), read: true, ..call(1, 3, 150, 8) };
    db.transaction(|tx| apply_items(tx, &Items { upserts: vec![changed.clone()], deletes: vec![2], sequence: None }))
        .unwrap();

    assert_eq!(history(&db).unwrap(), vec![changed]);
}

#[test]
fn completion_records_the_time_and_the_sequence_number() {
    let db = db();
    set_expected(&db, 5).unwrap();
    assert!(!sync_state(&db).unwrap().up_to_date(), "up to date before the sync");

    db.transaction(|tx| complete(tx, 1_000, Some(5))).unwrap();

    let s = sync_state(&db).unwrap();
    assert_eq!(s, SyncState { current: 5, expected: 5, last_updated_ms: Some(1_000) });
    assert!(s.up_to_date(), "not up to date after the sync");
}

#[test]
fn the_badge_counts_missed_calls_after_the_mark() {
    let db = db();
    store(&db, vec![call(1, 3, 100, 1), call(2, 1, 200, 1), call(3, 3, 300, 1)]);
    assert_eq!(unseen_missed(&db).unwrap(), 2);

    assert!(mark_seen(&db).unwrap(), "the mark did not move");
    assert_eq!(unseen_missed(&db).unwrap(), 0);
    assert!(!mark_seen(&db).unwrap(), "the mark moved again");

    store(&db, vec![call(4, 3, 400, 1), call(5, 3, 250, 1)]);
    assert_eq!(unseen_missed(&db).unwrap(), 1);
}

#[test]
fn consent_and_the_auto_refresh_claim_change_once() {
    let db = db();
    assert_eq!(state(&db).unwrap(), PhoneState { pc_consent: false, auto_refresh_done: false, badge_mark_ms: None });

    assert!(set_pc_consent(&db, true).unwrap(), "consent unchanged");
    assert!(!set_pc_consent(&db, true).unwrap(), "consent changed twice");
    assert!(claim_auto_refresh(&db).unwrap(), "refresh not claimed");
    assert!(!claim_auto_refresh(&db).unwrap(), "refresh claimed twice");
}
