//! The selected phone, end to end over the fakes (`support/engine_world`): no phone is chosen
//! automatically, the user's selection starts the phone session and is restored by the next start,
//! another phone can be selected while the engine runs, and an id that names no linked phone is
//! refused.

use crate::support::auth_world;
use crate::support::engine_world as world;

use std::time::Duration;

use auth_world::harness::eventually;
use phonelink_engine::edge::SecureStore;
use phonelink_engine::engine::{EngineCore, EngineError, SELECTED_PHONE_KEY};
use phonelink_engine::tap::MemoryTap;
use world::World;

fn stored_selection(w: &World) -> Option<String> {
    w.h.store.load(SELECTED_PHONE_KEY.into()).unwrap().map(|b| String::from_utf8(b).unwrap())
}

fn build(w: &World) -> std::sync::Arc<EngineCore> {
    EngineCore::build(w.config(), w.h.edges(), World::feature_edges(), MemoryTap::new(), w.h.core_events())
        .expect("engine builds")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_a_selection_the_start_waits_and_the_users_selection_starts_the_phone() {
    let w = World::new(None).await;
    w.h.store.delete(SELECTED_PHONE_KEY.into()).unwrap();

    let engine = build(&w);
    let started = engine.start(None).await.expect("sign-in and enrollment");

    assert!(started.phone.is_none(), "no phone is chosen automatically");
    assert!(engine.phone().is_none());
    assert!(engine.features().is_none(), "nothing runs for a phone before one is selected");
    assert_eq!(engine.phones().len(), 1, "the account's phone is offered");
    // What connects a phone starts with the selection: the App layer, whose triggers send nothing
    // before its start, and the CDEH session, which serves the selected phone only.
    assert!(!engine.app.is_started(), "the App layer starts with the selection");
    assert!(w.hub.negotiates().is_empty(), "no phone is connected before the selection");

    assert!(matches!(engine.select_phone("not-a-phone").await, Err(EngineError::NoPhone)));

    let phone = engine.select_phone(&w.phone_id).await.expect("the user's selection");
    assert_eq!(phone.client.id, w.phone_id);
    assert_eq!(engine.phone().map(|p| p.id), Some(w.phone_id.clone()));
    assert!(engine.features().is_some(), "the selection starts the phone session");
    assert_eq!(stored_selection(&w), Some(w.phone_id.clone()), "remembered across restarts");
    assert!(eventually(Duration::from_secs(20), || !w.hub.negotiates().is_empty()).await, "the phone's session opens");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_phone_can_be_selected_and_the_next_start_restores_it() {
    let w = World::new(None).await;
    let other = w.h.phone("Galaxy");
    w.h.dcg.add_peer(other.item.clone());
    let engine = build(&w);
    let started = engine.start(None).await.expect("start sequence");
    assert_eq!(started.phone.map(|p| p.client.id), Some(w.phone_id.clone()), "the stored selection");
    assert_eq!(engine.phones().len(), 2);

    engine.select_phone(&other.id).await.expect("the user selects the other phone");
    assert_eq!(engine.phone().map(|p| p.id), Some(other.id.clone()));
    assert_eq!(engine.app.phone_record().map(|r| r.phone().id), Some(other.id.clone()), "the App layer serves it");
    assert_eq!(stored_selection(&w), Some(other.id.clone()));
    engine.select_phone(&other.id).await.expect("selecting the selected phone changes nothing");

    let rebuilt = build(&w);
    let again = rebuilt.start(None).await.expect("the next start");
    assert_eq!(again.phone.map(|p| p.client.id), Some(other.id.clone()), "the selection is restored");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_selected_phone_that_left_the_account_is_selected_again_by_the_user() {
    let w = World::new(None).await;
    w.h.select_phone("a-phone-no-longer-linked");
    let engine = build(&w);
    let started = engine.start(None).await.expect("sign-in and enrollment");
    assert!(started.phone.is_none(), "another phone is not chosen automatically");
    assert!(engine.features().is_none());
}
