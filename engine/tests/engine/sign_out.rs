//! Sign-in and the user's sign-out with the engine's two identities, end to end over the fakes
//! (`support/engine_world`): one device-bound web sign-in serves both identities, each of which
//! then exchanges its own tokens under its own client id (differs from Phone Link on Windows); a
//! sign-out stops the session and leaves no account state of either identity, no selected phone and
//! no phone data, keeps only the machine's ids and its device credential and ticket, and the next
//! start signs in through the web view again.

use std::collections::BTreeSet;

use crate::support::auth_world::harness::USER_KEY;
use crate::support::engine_world as world;

use phonelink_engine::api::engine::Engine;
use phonelink_engine::app::phone_record::INSTALLATION_ID_KEY;
use phonelink_engine::constants::{PROFILE_PL, PROFILE_WEA};
use phonelink_engine::edge::{SecureStore, WebSignInKind};
use phonelink_engine::engine::{EngineCore, SELECTED_PHONE_KEY};
use phonelink_engine::features::StoreLocation;
use phonelink_engine::identification::LOGICAL_DEVICE_ID_KEY;
use phonelink_engine::tap::MemoryTap;
use world::World;

/// Store keys of the machine's device registration, which a sign-out keeps.
const DEVICE_KEYS: [&str; 2] = ["auth/live/device", "auth/live/device-ticket"];

/// The client ids the exchanges so far ran under.
fn exchange_clients(w: &World) -> BTreeSet<String> {
    w.h.msa.exchanges().into_iter().map(|e| e.client_id).collect()
}

fn both_client_ids() -> BTreeSet<String> {
    [PROFILE_PL.msa_client_id, PROFILE_WEA.msa_client_id].into_iter().map(str::to_string).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_web_sign_in_signs_both_identities_in() {
    let w = World::new(None).await;
    w.h.store.delete(USER_KEY.into()).unwrap();
    let engine =
        EngineCore::build(w.two_identities(), w.h.edges(), World::feature_edges(), MemoryTap::new(), w.h.core_events())
            .expect("engine builds");
    engine.start(None).await.expect("start sequence");

    assert_eq!(w.h.web.kinds(), [WebSignInKind::Login], "one sign-in page for both identities");
    assert_eq!(exchange_clients(&w), both_client_ids(), "each identity exchanges under its own client id");
    let interactive: Vec<_> = w.h.msa.exchanges().into_iter().filter(|e| e.is_interactive()).collect();
    assert_eq!(interactive.len(), 1, "the page's inline flow token is spent once: {interactive:#?}");
    assert_eq!(interactive[0].inline_ft.as_deref(), Some("ft-1"));
    assert!(engine.auth.own_client().is_some() && engine.cdeh.as_ref().unwrap().auth.own_client().is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stored_session_signs_both_identities_in_without_the_web_view() {
    let w = World::new(None).await;
    let engine =
        EngineCore::build(w.two_identities(), w.h.edges(), World::feature_edges(), MemoryTap::new(), w.h.core_events())
            .expect("engine builds");
    engine.start(None).await.expect("start sequence");

    assert!(w.h.web.kinds().is_empty(), "no sign-in page");
    assert_eq!(exchange_clients(&w), both_client_ids());
    assert!(w.h.msa.exchanges().iter().all(|e| !e.is_interactive()), "every exchange is silent");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sign_out_forgets_both_identities_the_phone_and_its_data_then_signs_in_again() {
    let w = World::new(None).await;
    let data = tempfile::tempdir().unwrap();
    let mut features = World::feature_edges();
    features.stores = StoreLocation::Directory(data.path().to_path_buf());
    let engine = Engine::with_config(w.two_identities(), w.h.edges(), features, w.h.events.clone());

    engine.start().await.expect("start");
    assert!(engine.status().phone_id.is_some(), "serving the phone");
    assert!(data.path().join("phones").is_dir(), "the phone's feature stores exist");
    let keys = w.h.store.keys();
    assert!(keys.iter().any(|k| k.starts_with("pl/auth/")) && keys.iter().any(|k| k.starts_with("auth/prod/")));

    engine.sign_out().await.expect("sign-out");

    assert!(engine.current_core().is_none(), "the core is dropped");
    assert_eq!(engine.status().own_device_id, None);
    let kept: BTreeSet<String> = w.h.store.keys().into_iter().collect();
    let expected: BTreeSet<String> =
        [INSTALLATION_ID_KEY, LOGICAL_DEVICE_ID_KEY].into_iter().chain(DEVICE_KEYS).map(str::to_string).collect();
    assert_eq!(kept, expected, "only the machine's ids and its device registration stay");
    assert!(w.h.store.load(SELECTED_PHONE_KEY.into()).unwrap().is_none());
    assert!(!data.path().join("phones").exists(), "the phone data is deleted");
    assert!(w.h.web.kinds().is_empty(), "a sign-out shows no page");

    // The next start signs in through the web view, on the kept device registration, and asks for
    // a phone.
    engine.start().await.expect("start after the sign-out");
    assert_eq!(w.h.web.kinds(), [WebSignInKind::Login]);
    assert!(engine.status().needs_phone_selection, "the selected phone was forgotten");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sign_out_before_any_start_clears_the_stored_state() {
    let w = World::new(None).await;
    let engine = Engine::with_config(w.two_identities(), w.h.edges(), World::feature_edges(), w.h.events.clone());

    engine.sign_out().await.expect("sign-out without a core");

    let keys = w.h.store.keys();
    assert!(!keys.iter().any(|k| k == USER_KEY || k == SELECTED_PHONE_KEY), "{keys:?}");
    for key in DEVICE_KEYS {
        assert!(keys.iter().any(|k| k == key), "{key} is kept: {keys:?}");
    }
}
