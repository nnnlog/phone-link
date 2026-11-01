//! The shared feature scaffolding over the App layer: permission flags read with their feature
//! default until the phone reports them in its `permissions` message, the peer directory and the
//! per-phone switch of the `FeatureEnv` test world.

use crate::support::app_world;
use crate::support::feature_world;

use app_world::*;
use feature_world::feature_world;
use phonelink_engine::app::AppEvent;
use phonelink_engine::app::consts::permission;
use phonelink_engine::codec::valueset::ValueSet;

fn permissions_push(types: Vec<i32>, states: Vec<i32>) -> phonelink_engine::types::TransportMessage {
    phone_content(
        "permissions",
        ValueSet::new()
            .with("itemCount", types.len() as i32)
            .with("permissionTypes", types)
            .with("permissionStates", states),
    )
}

#[tokio::test(start_paused = true)]
async fn a_reported_denial_overrides_a_granted_default_and_is_announced() {
    let h = Harness::new();
    let record = h.app.phone_record().unwrap();
    assert!(record.permission_or(permission::MESSAGES, true), "unreported: the feature default");
    assert!(!record.permission_or(permission::NOTIFICATIONS_SYNC, false));

    let mut events = h.app.subscribe_events();
    // Messages denied (state 1): bit 0x2 stays 0, but it is now reported.
    h.app.handle_inbound(phone(), trace(), permissions_push(vec![0x2], vec![1])).await;
    let record = h.app.phone_record().unwrap();
    assert!(!record.permission_or(permission::MESSAGES, true));

    let mut announced = false;
    while let Ok(e) = events.try_recv() {
        announced |= matches!(e, AppEvent::PermissionsChanged { .. });
    }
    assert!(announced, "a report that only changes the reported set is announced");

    // NotificationsSync granted (state 0).
    h.app.handle_inbound(phone(), trace(), permissions_push(vec![0x40], vec![0])).await;
    assert!(h.app.phone_record().unwrap().permission_or(permission::NOTIFICATIONS_SYNC, false));
}

#[tokio::test(start_paused = true)]
async fn the_feature_world_knows_the_harness_phone() {
    let h = Harness::new();
    let w = feature_world(&h);

    assert!(w.env.peers.is_enrolled());
    let peer = w.env.peers.peer(&phone()).expect("the harness phone is a peer");
    assert_eq!(peer.info.os_name.as_deref(), Some("Android"));
    assert!(w.switches.is_enabled(&phone()).unwrap());
    assert!(w.notifier.calls().is_empty());
}
