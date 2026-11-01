//! The `CLIPBOARD` capability through the real engine, as the PC's capability and as enrollment
//! reports it: `EnrollDevice` carries it as the persisted global switch says, and a change of the
//! switch reaches `SetDeviceInfo` — also when the clipboard's event stream lagged and lost the
//! change event. `REMOTE_CONTROL` and `REMOTE_CONTROL_V2` follow `UNLINKABLE_V1` (remote control's
//! options are on by default), then `FILE_TRANSFER` (the Mac receives files by default).

use crate::support::auth_world;
use crate::support::fake_hub;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use auth_world::harness::{Harness, eventually};
use fake_hub::FakeHub;
use phonelink_engine::engine::EngineCore;
use phonelink_engine::features::clipboard::store::ClipboardStore;
use phonelink_engine::features::shared::notifier::testing::RecordingNotifier;
use phonelink_engine::features::{FeatureEdges, StoreLocation};
use phonelink_engine::tap::MemoryTap;

fn caps(r: &auth_world::fake_dcg::Recorded) -> Vec<String> {
    r.json()["metadata"]["capabilities"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

async fn engine(h: &Harness, stores: StoreLocation) -> (Arc<EngineCore>, FakeHub) {
    h.seed_session();
    let device = h.phone("Pixel");
    h.dcg.add_peer(device.item.clone());
    h.select_phone(&device.id);

    let hub = FakeHub::start().await;
    let mut config = h.config.clone();
    config.endpoints.hub_base = Some(hub.base_url());

    let features = FeatureEdges::stand_ins(stores, RecordingNotifier::new());
    let engine =
        EngineCore::build(config, h.edges(), features, MemoryTap::new(), h.core_events()).expect("engine builds");
    engine.start(None).await.expect("start sequence");

    (engine, hub)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switch_change_reaches_set_device_info() {
    let h = Harness::new().await;
    let dir = tempfile::tempdir().unwrap();
    let (engine, _hub) = engine(&h, StoreLocation::Directory(dir.path().to_path_buf())).await;

    let enroll = h.dcg.of("EnrollDevice");
    assert_eq!(enroll.len(), 1);

    // `/DeviceAuthProxy/EnrollDevice` and `SetDeviceInfo`: DEVICE_STATUS and UNLINKABLE_V1 always,
    // CLIPBOARD while the clipboard's switch is on (first), REMOTE_CONTROL and REMOTE_CONTROL_V2 while
    // remote control's options are on (last).
    assert_eq!(
        caps(&enroll[0]),
        ["CLIPBOARD", "DEVICE_STATUS", "UNLINKABLE_V1", "REMOTE_CONTROL", "REMOTE_CONTROL_V2", "FILE_TRANSFER"]
    );

    let installed = engine.features().unwrap();
    installed.clipboard.set_global_switch(false).unwrap();

    let off = eventually(Duration::from_secs(10), || {
        h.dcg.of("SetDeviceInfo").iter().any(|r| {
            caps(r) == ["DEVICE_STATUS", "UNLINKABLE_V1", "REMOTE_CONTROL", "REMOTE_CONTROL_V2", "FILE_TRANSFER"]
        })
    })
    .await;
    assert!(off, "a SetDeviceInfo without CLIPBOARD follows the switch going off");

    installed.clipboard.set_global_switch(true).unwrap();

    let on = eventually(Duration::from_secs(10), || {
        h.dcg.of("SetDeviceInfo").last().is_some_and(|r| {
            caps(r)
                == [
                    "CLIPBOARD",
                    "DEVICE_STATUS",
                    "UNLINKABLE_V1",
                    "REMOTE_CONTROL",
                    "REMOTE_CONTROL_V2",
                    "FILE_TRANSFER",
                ]
        })
    })
    .await;
    assert!(on, "a SetDeviceInfo with CLIPBOARD follows the switch going on");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn enrollment_follows_the_persisted_switch() {
    let h = Harness::new().await;
    let dir = tempfile::tempdir().unwrap();
    let stores = StoreLocation::Directory(dir.path().to_path_buf());

    {
        let store = ClipboardStore::open(&stores).unwrap();
        assert!(store.set_global_switch(false).unwrap());
    }

    let (_engine, _hub) = engine(&h, stores).await;

    let enroll = h.dcg.of("EnrollDevice");
    assert_eq!(enroll.len(), 1);
    assert_eq!(
        caps(&enroll[0]),
        ["DEVICE_STATUS", "UNLINKABLE_V1", "REMOTE_CONTROL", "REMOTE_CONTROL_V2", "FILE_TRANSFER"]
    );
}

/// A switch change whose event is lost to a lagged event stream (64 buffered clipboard events while
/// the bridge waits on a slow `SetDeviceInfo`) is still advertised: the bridge re-reads the switch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switch_change_lost_to_a_lagged_event_stream_is_still_advertised() {
    let h = Harness::new().await;
    let dir = tempfile::tempdir().unwrap();
    let (engine, _hub) = engine(&h, StoreLocation::Directory(dir.path().to_path_buf())).await;
    let installed = engine.features().unwrap();
    let phone = engine.phone().expect("a phone");

    // The first SetDeviceInfo (switch off) is held until the test releases it: the bridge waits on it.
    let entered = Arc::new(AtomicBool::new(false));
    let (release, mut held) = {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        (tx, Some(rx))
    };
    let in_handler = entered.clone();
    h.dcg.on_request("SetDeviceInfo", move |_| {
        if let Some(held) = held.take() {
            in_handler.store(true, Ordering::SeqCst);
            // An error is the test gone: the request goes on.
            let _ = held.recv();
        }
    });

    installed.clipboard.set_global_switch(false).unwrap();
    // (Do not touch `h.dcg` until the request is released: its handler holds the fake's state lock.)
    let waiting = eventually(Duration::from_secs(10), || entered.load(Ordering::SeqCst)).await;
    assert!(waiting, "the bridge sends a SetDeviceInfo for the switch going off");

    // While the bridge waits: the switch goes back on, then 70 ordinary clipboard events.
    installed.clipboard.set_global_switch(true).unwrap();
    for i in 0..70 {
        installed.clipboard.set_phone_toggle(&phone, i % 2 == 0).unwrap();
    }

    release.send(()).unwrap();
    // The bridge hears of the lost events once the request returns, and re-reads the switch.
    let advertised = eventually(Duration::from_secs(10), || {
        h.dcg.of("SetDeviceInfo").iter().any(|r| caps(r).contains(&"CLIPBOARD".to_owned()))
    })
    .await;

    assert!(installed.clipboard.global_switch().unwrap());
    let sent: Vec<Vec<String>> = h.dcg.of("SetDeviceInfo").iter().map(caps).collect();
    assert!(advertised, "the switch is on but CLIPBOARD was never re-advertised: {sent:?}");
}
