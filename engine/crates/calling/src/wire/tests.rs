use phonelink_core::codec::valueset::{Value, ValueSet};

use super::*;

fn items_payload() -> ValueSet {
    ValueSet::new()
        .with(key::ITEM_COUNT, 2i32)
        .with(key::IDS, Value::Int64Array(vec![10, 11]))
        .with(key::PHONE_NUMBERS, Value::StringArray(vec!["+15550100".into(), String::new()]))
        .with(key::DURATION, Value::Int64Array(vec![61, 0]))
        .with(key::TYPE, Value::Int32Array(vec![1, 3]))
        .with(key::DATE, Value::Int64Array(vec![1_700_000_000_000, 1_700_000_100_000]))
        .with(key::READ, Value::BoolArray(vec![true, false]))
        .with(key::PHONE_ACCOUNT_IDS, Value::StringArray(vec!["1".into(), String::new()]))
        .with(key::LAST_UPDATED_TIMES, Value::Int64Array(vec![5, 6]))
}

#[test]
fn an_item_message_without_actions_upserts_every_entry() {
    let items = parse_items(&items_payload().with(key::SEQUENCE_NUMBER, 9i64)).unwrap();

    assert_eq!(items.deletes, Vec::<i64>::new());
    assert_eq!(items.sequence, Some(9));
    assert_eq!(
        items.upserts[0],
        CallEntry {
            id: 10,
            number: "+15550100".into(),
            duration: 61,
            call_type: 1,
            date: 1_700_000_000_000,
            read: true,
            phone_account_id: "1".into(),
            last_updated: 5,
        }
    );
    assert_eq!(items.upserts[1].call_type, TYPE_MISSED);
}

#[test]
fn action_two_deletes_and_every_other_action_upserts() {
    let payload = items_payload().with(key::ACTIONS, Value::Int32Array(vec![2, 1]));
    let items = parse_items(&payload).unwrap();

    assert_eq!(items.deletes, vec![10]);
    assert_eq!(items.upserts.iter().map(|e| e.id).collect::<Vec<_>>(), vec![11]);
    assert_eq!(items.ids().collect::<Vec<_>>(), vec![11, 10]);
}

#[test]
fn item_message_refusals() {
    let cases: &[(&str, ValueSet)] = &[
        ("no itemCount", {
            let mut p = items_payload();
            p.insert(key::ITEM_COUNT, Value::Int64(2));
            p
        }),
        ("a missing array", {
            let mut p = items_payload();
            p.insert(key::READ, Value::Empty);
            p
        }),
        ("a short array", items_payload().with(key::DATE, Value::Int64Array(vec![1]))),
        ("a short actions array", items_payload().with(key::ACTIONS, Value::Int32Array(vec![0]))),
        ("itemCount above the arrays", items_payload().with(key::ITEM_COUNT, 3i32)),
    ];

    for (case, payload) in cases {
        assert!(parse_items(payload).is_err(), "{case}: parsed");
    }
}

#[test]
fn item_count_below_the_arrays_processes_only_item_count_entries() {
    let items = parse_items(&items_payload().with(key::ITEM_COUNT, 1i32)).unwrap();

    assert_eq!(items.ids().collect::<Vec<_>>(), vec![10]);
}

#[test]
fn metadata_needs_ids_and_as_many_last_updated_times() {
    let ok = ValueSet::new()
        .with(app_key::CORRELATION_VECTOR, "cv")
        .with(key::IDS, Value::Int64Array(vec![1, 2]))
        .with(key::LAST_UPDATED_TIMES, Value::Int64Array(vec![7, 8, 9]))
        .with(key::SEQUENCE_NUMBER, 4i64);
    let m = parse_metadata(&ok).unwrap();
    assert_eq!(
        m,
        Metadata {
            correlation_vector: Some("cv".into()),
            ids: vec![1, 2],
            last_updated: vec![7, 8],
            sequence: Some(4)
        }
    );

    let short = ok.clone().with(key::LAST_UPDATED_TIMES, Value::Int64Array(vec![7]));
    assert!(parse_metadata(&short).is_err(), "short times parsed");
    let no_ids = ValueSet::new().with(key::LAST_UPDATED_TIMES, Value::Int64Array(vec![]));
    assert!(parse_metadata(&no_ids).is_err(), "no ids parsed");
}

#[test]
fn the_metadata_reply_lists_the_wanted_ids() {
    let reply = metadata_reply(Some("cv"), 3.99, &[4, 5]);

    assert_eq!(reply.str(app_key::CORRELATION_VECTOR), Some("cv"));
    assert_eq!(reply.str(app_key::CONTENT_TYPE), Some("callLogs"));
    assert_eq!(reply.get(app_key::CONTRACT_VERSION), Some(&Value::Double(3.99)));
    assert_eq!(reply.i32(key::ITEM_COUNT), Some(2));
    assert_eq!(reply.i64_array(key::IDS), Some(&[4i64, 5][..]));
    assert_eq!(reply.i32(app_key::RESULT), Some(0));
    assert_eq!(metadata_reply(None, 3.99, &[]).get(app_key::CORRELATION_VECTOR), Some(&Value::Empty));
}

#[test]
fn the_history_request_names_its_correlation_vector_and_content() {
    let trace = TraceContext::new_root(Vec::new());
    let cv = trace.correlation_id.clone();
    let request = history_request(trace, RequestOptions::default());

    assert_eq!(request.route, "/calling");
    assert_eq!(request.payload.str(app_key::CORRELATION_VECTOR), Some(cv.as_str()));
    assert_eq!(request.payload.str(app_key::CONTENT_TYPE), Some("callLogs"));
    assert_eq!(request.payload.string_array(key::RELATED_CORRELATION_IDS), Some(&[cv][..]));
    assert_eq!(request.payload.string_array(key::SUBCONTENT_TYPES), Some(&["callLogs".to_string()][..]));
}
