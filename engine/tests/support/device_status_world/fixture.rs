//! The device-status feature installed over the App test harness (`support/app_world`, through
//! `support/feature_world`), with a platform `World` (`support/platform_world`) supplying the
//! MSAEP and DRM handles the install takes. `tests/main.rs` declares it as
//! `crate::support::device_status_world::fixture`, next to the `crate::support::app_world`, `crate::support::feature_world` and `crate::support::platform_world`
//! it uses.
#![allow(dead_code)]

use crate::support::common::payload;
use phonelink_engine::config::features::DeviceStatusConfig;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;

use phonelink_engine::app::{InboundDisposition, PhoneRecord};
use phonelink_engine::codec::valueset::ValueSet;
use phonelink_engine::config::EngineConfig;
use phonelink_engine::features::StoreLocation;
use phonelink_engine::features::device_status::edge::standins::{FixedPcStatus, PcState};
use phonelink_engine::features::device_status::{DeviceStatus, DeviceStatusEvent, StatusView};
use phonelink_engine::types::{DeviceInfoItem, TransportMessage};

use crate::support::app_world::{ASYNC_TRUST_CUSTOM_DATA, Harness, device_item, phone, phone_content, trace};
use crate::support::feature_world::{FeatureWorld, feature_world_at};
use crate::support::platform_world::World;

pub struct Fixture {
    pub h: Harness,
    pub fw: FeatureWorld,
    /// Supplies the MSAEP and DRM handles; its phone is not the harness phone.
    pub world: World,
    pub pc: Arc<FixedPcStatus>,
    pub ds: Arc<DeviceStatus>,
    pub events: broadcast::Receiver<DeviceStatusEvent>,
}

impl Fixture {
    /// The harness phone (Android 14, `AsyncTrust`, device-list status capabilities `0x17FF`),
    /// connected, in-memory stores, default configuration.
    pub fn new() -> Self {
        Self::build(Harness::new(), StoreLocation::Memory, DeviceStatusConfig::default())
    }

    pub fn with_config(config: DeviceStatusConfig) -> Self {
        Self::build(Harness::new(), StoreLocation::Memory, config)
    }

    pub fn build(h: Harness, stores: StoreLocation, config: DeviceStatusConfig) -> Self {
        Self::build_with_item(h, stores, config, device_item(Some(ASYNC_TRUST_CUSTOM_DATA)))
    }

    pub fn build_with_item(
        h: Harness,
        stores: StoreLocation,
        config: DeviceStatusConfig,
        item: DeviceInfoItem,
    ) -> Self {
        let world = World::new(EngineConfig::default());
        let fw = feature_world_at(&h, stores, item);
        let pc = FixedPcStatus::new(PcState::default());
        let ds =
            DeviceStatus::install(&fw.env, config, world.routes.msaep.clone(), world.routes.drm.clone(), pc.clone())
                .expect("device status installs");
        let events = ds.subscribe();
        Fixture { h, fw, world, pc, ds, events }
    }

    /// Hands `message` to the App layer from the harness phone and returns its answer.
    pub async fn inbound(&self, message: TransportMessage) -> ValueSet {
        match self.h.app.handle_inbound(phone(), trace(), message).await {
            InboundDisposition::Answered(v) => v,
            other => panic!("not answered: {other:?}"),
        }
    }

    /// A `deviceStatus` item from the harness phone, answered.
    pub async fn status_item(&self, keys: ValueSet) -> ValueSet {
        self.inbound(phone_content("deviceStatus", keys)).await
    }

    pub fn record(&self) -> PhoneRecord {
        self.h.app.phone_record().expect("the harness phone is selected")
    }

    pub fn status(&self) -> StatusView {
        self.ds.status(&phone())
    }

    /// The events received so far.
    pub fn take_events(&mut self) -> Vec<DeviceStatusEvent> {
        let mut out = Vec::new();
        while let Ok(e) = self.events.try_recv() {
            out.push(e);
        }
        out
    }

    /// The App requests sent on `route`, decoded.
    pub fn sent_on(&self, route: &str) -> Vec<ValueSet> {
        self.h.messenger.requests_on(route).iter().map(payload).collect()
    }
}

/// Lets spawned tasks and listeners run (paused time advances 1 ms per round).
pub async fn settle() {
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}
