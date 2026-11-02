//! The inbound `callLogs` content: metadata and its answer, item messages, incremental pushes,
//! refusals, and what the calls view reads afterwards.

use crate::support::app_world;
use crate::support::calling_world;

use app_world::*;
use calling_world::*;
use phonelink_engine::codec::valueset::{self, Value, ValueSet};
use phonelink_engine::config::features::CallingConfig;
use phonelink_engine::features::calling::{CallingEvent, CallsAccess};

fn result(code: i32) -> ValueSet {
    ValueSet::new().with("result", code)
}

fn history_ids(world: &World) -> Vec<i64> {
    world.calling.history(&phone()).unwrap().iter().map(|e| e.call.id).collect()
}

#[tokio::test(start_paused = true)]
async fn a_full_sync_asks_for_every_listed_id_and_stores_the_items() {
    let world = world();
    world.grant();

    let answer = send(&world.h, metadata(&[(1, 10), (2, 20)], ValueSet::new().with("sequenceNumber", 7i64))).await;
    assert_eq!(answer.str("correlationVector"), Some("cv-1"));
    assert_eq!(answer.str("contentType"), Some("callLogs"));
    assert_eq!(answer.get("contractVersion"), Some(&Value::Double(3.99)));
    assert_eq!(answer.i32("itemCount"), Some(2));
    assert_eq!(answer.i64_array("callLogsIds"), Some(&[1i64, 2][..]));
    assert_eq!(answer.i32("result"), Some(0));

    let calls = [(1, "+15550100", 1, 1_000, 10), (2, "+15550101", 3, 2_000, 20)];
    assert_eq!(send(&world.h, items(&calls, ValueSet::new())).await, result(0));

    assert_eq!(history_ids(&world), vec![2, 1], "newest first");
    assert!(world.calling.status(&phone()).unwrap().last_updated_ms.is_some(), "no sync time recorded");
}

#[tokio::test(start_paused = true)]
async fn only_new_or_changed_ids_are_asked_for_when_configured() {
    let world = world_with(CallingConfig { request_every_listed_id: false });
    world.grant();
    send(&world.h, items(&[(1, "+15550100", 1, 1_000, 10), (2, "+15550101", 1, 2_000, 20)], ValueSet::new())).await;

    let answer = send(&world.h, metadata(&[(1, 10), (2, 21), (3, 30)], ValueSet::new())).await;

    assert_eq!(answer.i64_array("callLogsIds"), Some(&[2i64, 3][..]));
}

#[tokio::test(start_paused = true)]
async fn metadata_deletes_the_calls_the_phone_no_longer_lists() {
    let world = world();
    world.grant();
    send(&world.h, items(&[(1, "+15550100", 1, 1_000, 10), (2, "+15550101", 1, 2_000, 20)], ValueSet::new())).await;
    let mut events = world.calling.subscribe();

    let answer = send(&world.h, metadata(&[(2, 20)], ValueSet::new())).await;
    assert_eq!(answer.i64_array("callLogsIds"), Some(&[2i64][..]));
    assert_eq!(history_ids(&world), vec![2]);
    assert_eq!(events.try_recv().unwrap(), CallingEvent::HistoryChanged { phone: phone() });

    let empty = send(&world.h, metadata(&[], ValueSet::new())).await;
    assert_eq!(empty.i32("itemCount"), Some(0));
    assert_eq!(history_ids(&world), Vec::<i64>::new(), "an empty list keeps a call");
}

#[tokio::test(start_paused = true)]
async fn an_incremental_push_applies_its_actions() {
    let world = world();
    world.grant();
    send(&world.h, items(&[(1, "+15550100", 1, 1_000, 10)], ValueSet::new())).await;

    let push = items(
        &[(1, "", 0, 0, 0), (2, "+15550101", 2, 3_000, 30)],
        ValueSet::new().with("actions", Value::Int32Array(vec![2, 0])).with("incremental", 1i32),
    );
    assert_eq!(send(&world.h, push).await, result(0));

    assert_eq!(history_ids(&world), vec![2]);
}

#[tokio::test(start_paused = true)]
async fn a_malformed_item_message_is_refused_and_stores_nothing() {
    let world = world();
    world.grant();

    let short = items(&[(1, "+15550100", 1, 1_000, 10)], ValueSet::new().with("itemCount", 2i32));
    assert_eq!(send(&world.h, short).await.i32("result"), Some(1));

    let mut missing = items_payload(&[(1, "+15550100", 1, 1_000, 10)]);
    missing.insert("callLogsDate", Value::Empty);
    assert_eq!(send(&world.h, phone_content("callLogs", missing)).await.i32("result"), Some(1));

    assert_eq!(history_ids(&world), Vec::<i64>::new());
}

#[tokio::test(start_paused = true)]
async fn the_history_names_numbers_through_contacts_and_hides_behind_the_prompt() {
    let world = world();
    send(&world.h, items(&[(1, "+15550100", 1, 1_000, 10), (2, "", 3, 2_000, 20)], ValueSet::new())).await;

    assert_eq!(world.calling.status(&phone()).unwrap().access, CallsAccess::PcConsentRequired);
    assert!(world.calling.history(&phone()).unwrap().is_empty(), "history shown behind the prompt");

    world.grant();
    let history = world.calling.history(&phone()).unwrap();
    assert_eq!(history[1].contact.as_ref().and_then(|c| c.display_name.as_deref()), Some("Ada"));
    assert_eq!(history[0].contact, None, "a hidden number names no contact");
    assert_eq!(world.contacts.lookups.lock().unwrap().len(), 1, "a hidden number was looked up");
}

#[tokio::test(start_paused = true)]
async fn the_badge_counts_unseen_missed_calls_until_the_view_marks_them_seen() {
    let world = world();
    world.grant();
    send(&world.h, items(&[(1, "+15550100", 3, 1_000, 10), (2, "+15550101", 1, 2_000, 20)], ValueSet::new())).await;
    assert_eq!(world.calling.status(&phone()).unwrap().unseen_missed, 1);

    world.calling.mark_seen(&phone()).unwrap();
    assert_eq!(world.calling.status(&phone()).unwrap().unseen_missed, 0);

    send(&world.h, items(&[(3, "+15550100", 3, 3_000, 30)], ValueSet::new())).await;
    assert_eq!(world.calling.status(&phone()).unwrap().unseen_missed, 1);
}

#[tokio::test(start_paused = true)]
async fn a_message_without_a_correlation_vector_is_refused() {
    let world = world();
    world.grant();
    let mut vs = items_payload(&[(1, "+15550100", 1, 1_000, 10)]).with("contentType", "callLogs");
    vs.insert("contractVersion", 3.99f64);
    let msg = message(&[("_requestId", "req-1"), ("_route", "/legacy/phonecontent")], valueset::encode(&vs));

    assert_eq!(send(&world.h, msg).await.i32("result"), Some(1));
    assert_eq!(history_ids(&world), Vec::<i64>::new());
}
