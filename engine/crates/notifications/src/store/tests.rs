use super::*;
use phonelink_core::types::{DcgClient, Environment};
use phonelink_features::shared::db::StoreLocation;

fn db() -> FeatureDb {
    let phone = DcgClient::new("0f8fad5b-d9cb-469f-a165-70867728950e", Environment::Prod);
    FeatureDb::open_phone(&StoreLocation::Memory, &phone, "notifications", MIGRATIONS).unwrap()
}

fn item(key: &str, post_time: i64) -> (NotificationItem, String) {
    let json = format!(r#"{{"key":"{key}","packageName":"com.a","postTime":{post_time}}}"#);
    (NotificationItem::parse(&json).unwrap(), json)
}

#[test]
fn upserts_keep_pinned_state_and_local_id() {
    let db = db();
    let (a, ja) = item("a", 10);
    let effects = apply_batch(&db, &BatchPlan { upserts: vec![(&a, &ja, 1)], ..Default::default() }).unwrap();
    assert!(effects.upserted[0].is_new);

    let before = row_by_key(&db, "a").unwrap().unwrap();
    set_state(&db, "a", pinned::PINNED).unwrap();

    let (a2, ja2) = item("a", 20);
    let effects = apply_batch(&db, &BatchPlan { upserts: vec![(&a2, &ja2, 3)], ..Default::default() }).unwrap();
    assert!(!effects.upserted[0].is_new);

    let after = row_by_key(&db, "a").unwrap().unwrap();
    assert_eq!((after.post_time, after.state, &after.anonymous_id), (20, pinned::PINNED, &before.anonymous_id));
    assert_eq!(after.json, ja2);
}

#[test]
fn session_change_matches_case_insensitively_and_literally() {
    let db = db();
    let (a, ja) = item("0|S1|x", 1);
    let (b, jb) = item("0|s1|y", 2);
    let (c, jc) = item("0|S2|z", 3);
    let (d, jd) = item("0|S_1|w", 4);

    apply_batch(
        &db,
        &BatchPlan { upserts: vec![(&a, &ja, 1), (&b, &jb, 1), (&c, &jc, 1), (&d, &jd, 1)], ..Default::default() },
    )
    .unwrap();

    enqueue(&db, "r1", "0|S2|z", "0", "{}", 0).unwrap();
    enqueue(&db, "r2", "0|s1|y", "inline_reply", "{}", 0).unwrap();

    // `_` is literal: "S_1" does not match "S1".
    let effects = apply_batch(&db, &BatchPlan { session_changes: vec!["S1"], ..Default::default() }).unwrap();
    let mut purged: Vec<_> = effects.purged.iter().map(|r| r.key.as_str()).collect();
    purged.sort_unstable();
    assert_eq!(purged, ["0|S2|z", "0|S_1|w"]);

    let (keys, times) = keys_and_post_times(&db).unwrap();
    assert_eq!(keys, ["0|s1|y", "0|S1|x"]);
    assert_eq!(times, [2, 1]);
    assert_eq!(next_unsent(&db).unwrap().unwrap().request_id, "r2");
}

#[test]
fn removal_drops_queue_and_a_failed_batch_changes_nothing() {
    let db = db();
    let (a, ja) = item("a", 1);
    apply_batch(&db, &BatchPlan { upserts: vec![(&a, &ja, 1)], ..Default::default() }).unwrap();
    enqueue(&db, "r", "a", "0", "{}", 0).unwrap();

    let effects = apply_batch(&db, &BatchPlan { removals: vec!["a", "missing"], ..Default::default() }).unwrap();
    assert_eq!(effects.removed.len(), 1);
    assert!(next_unsent(&db).unwrap().is_none());
    assert!(all_rows(&db).unwrap().is_empty());
}
