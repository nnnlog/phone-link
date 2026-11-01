//! A `FeatureEnv` over the App-layer test harness (`support/app_world`): in-memory feature stores,
//! a recording system notifier, a static peer list holding the harness phone, and a manual clock.
//! `tests/main.rs` declares it as `crate::support::feature_world`, next to the `crate::support::app_world` it uses.
#![allow(dead_code)]

use std::sync::Arc;

use phonelink_engine::auth::PeerDevice;
use phonelink_engine::config::EngineConfig;
use phonelink_engine::edge::Edges;
use phonelink_engine::edge::testing::{ManualClock, memory_edges};
use phonelink_engine::features::shared::notifier::testing::RecordingNotifier;
use phonelink_engine::features::shared::peers::testing::StaticPeers;
use phonelink_engine::features::{FeatureEnv, NotificationRouter, PhoneSwitches, StoreLocation};
use phonelink_engine::tap::MemoryTap;
use phonelink_engine::types::DeviceInfoItem;

use crate::support::app_world::{ASYNC_TRUST_CUSTOM_DATA, Harness, device_item, phone};

pub struct FeatureWorld {
    pub env: FeatureEnv,
    pub notifier: Arc<RecordingNotifier>,
    pub peers: Arc<StaticPeers>,
    pub responses: Arc<NotificationRouter>,
    pub switches: Arc<PhoneSwitches>,
    pub clock: Arc<ManualClock>,
}

/// The harness phone's device-list entry as a peer.
pub fn harness_peer(item: DeviceInfoItem) -> PeerDevice {
    PeerDevice { client: phone(), info: item }
}

/// A `FeatureEnv` over `h` with in-memory stores; the peer list holds the harness phone (with
/// `AsyncTrust`, as `Harness::new` selects it).
pub fn feature_world(h: &Harness) -> FeatureWorld {
    feature_world_at(h, StoreLocation::Memory, device_item(Some(ASYNC_TRUST_CUSTOM_DATA)))
}

/// As [`feature_world`], with the stores at `stores` and the phone's device-list entry `item`.
pub fn feature_world_at(h: &Harness, stores: StoreLocation, item: DeviceInfoItem) -> FeatureWorld {
    let clock = Arc::new(ManualClock::fixed());
    let edges = Edges {
        store: h.store.clone(),
        lock: h.lock.clone(),
        reachability: h.reachability.clone(),
        clock: clock.clone(),
        ..memory_edges()
    };

    let notifier = RecordingNotifier::new();
    let peers = StaticPeers::new(vec![harness_peer(item)]);
    let responses = NotificationRouter::new();
    let switches = PhoneSwitches::open(&stores).expect("phone switches open");

    let env = FeatureEnv {
        config: Arc::new(EngineConfig::default()),
        edges,
        app: h.app.clone(),
        sessions: h.sessions.clone(),
        peers: peers.clone(),
        switches: switches.clone(),
        notifier: notifier.clone(),
        responses: responses.clone(),
        stores,
        tap: MemoryTap::new(),
    };

    FeatureWorld { env, notifier, peers, responses, switches, clock }
}
