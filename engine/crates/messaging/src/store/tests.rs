use super::*;
use crate::parse::{ConversationRow, MmsPartRow};

fn db() -> Connection {
    let mut c = Connection::open_in_memory().unwrap();
    let tx = c.transaction().unwrap();
    tx.execute_batch(MIGRATIONS[0]).unwrap();
    tx.commit().unwrap();
    ensure_phone_state(&c).unwrap();
    c
}

fn conv(thread_id: i64, checksum: i64) -> ConversationRow {
    ConversationRow { thread_id, checksum, recipients: vec!["+1".into()], has_rcs: false }
}

#[test]
fn delta_deletes_absent_and_requests_new_or_changed() {
    let c = db();
    for (t, cs) in [(1, 10), (2, 20), (3, 30)] {
        upsert_conversation(&c, &conv(t, cs)).unwrap();
    }

    let requested = delta(&c, Table::Conversation, &[2, 3, 4], Some(&[20, 31, 40])).unwrap();
    assert_eq!(requested, vec![3, 4]);
    let left: Vec<i64> = ids_and_checksums(&c, Table::Conversation).unwrap().into_iter().map(|(i, _)| i).collect();
    assert_eq!(left, vec![2, 3]);

    // An empty list deletes everything; without checksums every checksum is 0.
    assert_eq!(delta(&c, Table::Conversation, &[], None).unwrap(), Vec::<i64>::new());
    assert!(ids_and_checksums(&c, Table::Conversation).unwrap().is_empty());

    upsert_conversation(&c, &conv(5, 0)).unwrap();
    upsert_conversation(&c, &conv(6, 7)).unwrap();
    assert_eq!(delta(&c, Table::Conversation, &[5, 6], None).unwrap(), vec![6]);
}

#[test]
fn anchors_complete_and_up_to_date() {
    let c = db();
    set_expected_anchor(&c, "sms", 5).unwrap();
    set_expected_anchor(&c, "mms", -1).unwrap();
    assert!(!is_up_to_date(&c).unwrap());

    complete_anchors(&c, &[("sms", 5)]).unwrap();
    let rows = anchors(&c).unwrap();
    assert_eq!(rows, vec![AnchorRow { content_type: "sms".into(), current: 5, expected: 5 }]);
    assert!(is_up_to_date(&c).unwrap());

    complete_anchors(&c, &[]).unwrap();
    assert!(anchors(&c).unwrap().is_empty(), "no anchors deletes every stored anchor");
    assert_eq!(current_anchor(&c, "sms").unwrap(), -1);
}

#[test]
fn pending_send_ids_are_never_reused() {
    let c = db();
    let a = insert_pending_send(&c, "a", 1, 0, SendOrigin::Conversation, "x", &[], -1, &[]).unwrap();
    assert_eq!(a, 1, "the counter starts at 1");
    assert_eq!(delete_pending_sends(&c, &[a, 99]).unwrap(), vec![a]);
    let b = insert_pending_send(&c, "b", 1, 0, SendOrigin::Conversation, "y", &[], -1, &[]).unwrap();
    assert_eq!(b, 2);
}

#[test]
fn rcs_participants_replace_missing_ones() {
    let c = db();
    upsert_participant(&c, 7, "010", "+8210").unwrap();
    upsert_participant(&c, 7, "x@rcs", "x@rcs").unwrap();

    set_rcs_participants(&c, 7, &["x@rcs".into(), "y@rcs".into()]).unwrap();

    assert_eq!(
        participants(&c, 7).unwrap(),
        vec![("x@rcs".to_string(), "x@rcs".to_string()), ("y@rcs".to_string(), "y@rcs".to_string())]
    );
}

#[test]
fn mms_parts_keep_bytes_and_file_transfers_replace_them() {
    let c = db();
    let mut p = MmsPartRow {
        part_id: 1,
        message_id: 9,
        sequence: 0,
        content_id: String::new(),
        content_type: "image/png".into(),
        name: String::new(),
        text: String::new(),
        charset: 106,
        size: 3,
        bytes: Some(vec![1, 2, 3]),
    };

    upsert_mms_part(&c, &p).unwrap();
    p.bytes = None;
    upsert_mms_part(&c, &p).unwrap();

    assert_eq!(mms_part(&c, 1).unwrap().unwrap().bytes, Some(vec![1, 2, 3]));
}
