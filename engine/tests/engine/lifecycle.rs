//! The engine's lifecycle with the features installed: a start that follows `stop` brings the phone
//! session back (the app-start trigger sends a background connect); a feature install that fails
//! part-way leaves the core needing a rebuild (it cannot install the features again), and a new
//! core over the same state starts; a notification response the foreign side routes from its own
//! thread runs inside the engine's runtime.

use crate::support::auth_world;
use crate::support::fake_hub;

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use auth_world::harness::{Harness, eventually};
use fake_hub::{FakeHub, on_connected, on_partner_connected};
use phonelink_engine::engine::EngineCore;
use phonelink_engine::features::shared::notifier::testing::RecordingNotifier;
use phonelink_engine::features::{FeatureEdges, NotificationResponder, NotificationResponse, StoreLocation};
use phonelink_engine::tap::MemoryTap;

async fn world(h: &Harness) -> (phonelink_engine::config::EngineConfig, FakeHub, String) {
    h.seed_session();
    let device = h.phone("Pixel");
    let phone_id = device.id.clone();
    h.dcg.add_peer(device.item.clone());
    h.select_phone(&device.id);

    let hub = FakeHub::start().await;
    let mut config = h.config.clone();
    config.endpoints.hub_base = Some(hub.base_url());

    (config, hub, phone_id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_start_after_stop_brings_the_phone_back() {
    let h = Harness::new().await;
    let (mut config, hub, phone_id) = world(&h).await;

    // The scripted hub does not deliver the `/PlatformDisconnectRequest`; do not wait 30 s for it.
    config.platform.send_disconnect_request_timeout = Duration::from_secs(1);
    let features = FeatureEdges::stand_ins(StoreLocation::Memory, RecordingNotifier::new());
    let engine = EngineCore::build(config, h.edges(), features, MemoryTap::new(), h.core_events()).unwrap();

    engine.start(None).await.unwrap();
    let session = hub.session(0).await;
    session.send(on_connected(Some("westus"), &[]));
    assert!(eventually(Duration::from_secs(20), || h.dcg.count("Wake") > 0).await, "first wake");
    session.send(on_partner_connected(&phone_id, rmpv::Value::from("westus")));
    assert!(eventually(Duration::from_secs(20), || engine.phone_connected()).await, "connected");

    engine.stop().await;
    assert!(!engine.phone_connected());

    let (wakes, hubs) = (h.dcg.count("Wake"), hub.sessions().len());
    engine.start(None).await.expect("the second start");
    assert!(
        eventually(Duration::from_secs(20), || hub.sessions().len() > hubs || h.dcg.count("Wake") > wakes).await,
        "after stop + start the background connect reconnects"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_feature_install_needs_a_rebuild_and_a_new_core_starts() {
    let h = Harness::new().await;
    let (config, _hub, _phone_id) = world(&h).await;
    let dir = tempfile::tempdir().unwrap();
    let stores = StoreLocation::Directory(dir.path().to_path_buf());
    let engine = EngineCore::build(
        config.clone(),
        h.edges(),
        FeatureEdges::stand_ins(stores.clone(), RecordingNotifier::new()),
        MemoryTap::new(),
        h.core_events(),
    )
    .unwrap();

    // `build` created the clipboard's global store; make it unopenable, so the clipboard — installed
    // after five other features — fails.
    let clip = dir.path().join("clipboard.sqlite3");
    std::fs::set_permissions(&clip, std::fs::Permissions::from_mode(0o000)).unwrap();

    assert!(engine.start(None).await.is_err(), "the clipboard store cannot be opened");
    assert!(engine.features().is_none());
    assert!(engine.needs_rebuild());

    // The condition clears. This core refuses to install again (the earlier features stay
    // registered); a new core over the same state starts.
    std::fs::set_permissions(&clip, std::fs::Permissions::from_mode(0o600)).unwrap();
    let again = engine.start(None).await.expect_err("a half-installed core does not start again");
    assert!(again.to_string().contains("rebuilt"), "{again}");

    drop(engine);
    let rebuilt = EngineCore::build(
        config,
        h.edges(),
        FeatureEdges::stand_ins(stores, RecordingNotifier::new()),
        MemoryTap::new(),
        h.core_events(),
    )
    .unwrap();
    rebuilt.start(None).await.expect("a new core starts");
    assert!(rebuilt.features().is_some());
    assert!(!rebuilt.needs_rebuild());
}

/// Records whether a response arrived inside a tokio runtime.
#[derive(Default)]
struct Probe {
    in_runtime: AtomicBool,
    responses: Mutex<Vec<String>>,
}

impl NotificationResponder for Probe {
    fn on_response(&self, id: &str, _user_info: &HashMap<String, String>, _response: NotificationResponse) {
        self.in_runtime.store(tokio::runtime::Handle::try_current().is_ok(), Ordering::SeqCst);
        self.responses.lock().unwrap().push(id.to_string());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_notification_response_from_a_foreign_thread_runs_in_the_engine_runtime() {
    let h = Harness::new().await;
    let (config, _hub, _phone_id) = world(&h).await;
    let features = FeatureEdges::stand_ins(StoreLocation::Memory, RecordingNotifier::new());
    let engine = EngineCore::build(config, h.edges(), features, MemoryTap::new(), h.core_events()).unwrap();

    let probe = Arc::new(Probe::default());
    engine.responses.register("probe", probe.clone()).unwrap();

    // The foreign side (the `UNUserNotificationCenter` delegate) calls on a thread of its own.
    let core = engine.clone();
    std::thread::spawn(move || {
        core.notification_response("probe/x", &HashMap::new(), NotificationResponse::Activated);
    })
    .join()
    .unwrap();

    assert_eq!(*probe.responses.lock().unwrap(), ["probe/x"]);
    assert!(probe.in_runtime.load(Ordering::SeqCst), "the responder ran inside the engine's runtime");
}
