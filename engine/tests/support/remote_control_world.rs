//! Remote control installed over the platform `World` with an App layer on top
//! (`support/device_status_world`): the phone's DRM requests reach the `/remotecontrol`
//! receiver and its App content the `remoteControl` processor over one loopback. The Mac's actions
//! go to a recording edge — nothing here can lock, sleep, shut down or restart the machine.
//! `tests/main.rs` declares it as `crate::support::remote_control_world`, next to the `crate::support::platform_world` and
//! `crate::support::device_status_world` it uses.
#![allow(dead_code)]

use phonelink_engine::config::features::RemoteControlConfig;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use prost::Message;

use phonelink_engine::auth::PeerDevice;
use phonelink_engine::codec::valueset::ValueSet;
use phonelink_engine::features::remote_control::edge::standins::RecordingPowerControl;
use phonelink_engine::features::remote_control::{DeviceListRefresh, RemoteControl, RemoteControlEvent};
use phonelink_engine::proto::platform_sdk::v1 as pb;
use phonelink_engine::proto::remote_control::v1::{RemoteControlRequestMessage, RemoteControlResponseMessage};
use phonelink_engine::seams::{PlatformRequestStatus, PlatformResponse};
use phonelink_engine::types::DeviceInfoItem;

use crate::support::device_status_world::{StatusWorld, phone_item};
use crate::support::platform_world::phone_id;

type OnRefresh = Box<dyn Fn() + Send + Sync>;

/// The device-list refresh the receiver asks for: counted, and running what the test sets (for
/// example adding the capability the phone lacked).
#[derive(Default)]
pub struct FakeRefresh {
    pub count: AtomicUsize,
    on_refresh: Mutex<Option<OnRefresh>>,
}

impl FakeRefresh {
    pub fn on_refresh(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.on_refresh.lock().unwrap() = Some(Box::new(f));
    }
    pub fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl DeviceListRefresh for FakeRefresh {
    async fn refresh_device_list(&self) -> bool {
        self.count.fetch_add(1, Ordering::SeqCst);
        if let Some(f) = self.on_refresh.lock().unwrap().as_ref() {
            f();
        }
        true
    }
}

/// The World phone's entry with `capabilities`.
pub fn item_with(capabilities: &[&str]) -> DeviceInfoItem {
    DeviceInfoItem { capabilities: Some(capabilities.iter().map(|s| s.to_string()).collect()), ..phone_item() }
}

pub const V2: [&str; 2] = ["REMOTE_CONTROL", "REMOTE_CONTROL_V2"];

pub struct RcWorld {
    pub sw: StatusWorld,
    pub power: Arc<RecordingPowerControl>,
    pub refresh: Arc<FakeRefresh>,
    pub rc: Arc<RemoteControl>,
}

impl RcWorld {
    /// The phone (`item`) selected but not connected; remote control installed with `config`.
    pub fn new(item: DeviceInfoItem, config: RemoteControlConfig) -> Self {
        let sw = StatusWorld::new(item);
        let power = RecordingPowerControl::new();
        let refresh = Arc::new(FakeRefresh::default());
        let rc =
            RemoteControl::install(&sw.env, &sw.env, config, sw.w.routes.drm.clone(), power.clone(), refresh.clone())
                .expect("remote control installs");
        RcWorld { sw, power, refresh, rc }
    }

    /// A phone with both capabilities, default options, connected.
    pub async fn connected() -> Self {
        let world = Self::new(item_with(&V2), RemoteControlConfig::default());
        world.sw.w.connect_phone().await;
        world
    }

    /// Replaces the phone's device-list entry.
    pub fn set_item(&self, item: DeviceInfoItem) {
        self.sw.peers.upsert(PeerDevice { client: phone_id(), info: item });
    }

    /// The phone sends `control_type` on `/remotecontrol` (path-addressed, `UPDATE`) and reads the answer.
    pub async fn drm(&self, control_type: i32) -> RemoteControlResponseMessage {
        let payload = RemoteControlRequestMessage { control_type }.encode_to_vec();
        self.drm_raw(4, 2, "/remotecontrol", &payload).await
    }

    /// The phone sends a raw `/DeviceResourceManager` request and reads the answer.
    pub async fn drm_raw(
        &self,
        resource_type: i32,
        request_type: i32,
        path: &str,
        payload: &[u8],
    ) -> RemoteControlResponseMessage {
        RemoteControlResponseMessage::decode(
            answer_bytes(&self.sw.phone_drm_request(resource_type, request_type, path, payload).await).as_slice(),
        )
        .expect("a RemoteControlResponseMessage")
    }

    /// The phone sends `remoteControl` content with `keys`; returns the PC's answer.
    pub async fn content(&self, keys: ValueSet) -> ValueSet {
        self.sw.phone_sends("remoteControl", keys).await
    }

    pub fn events(&self, rx: &mut tokio::sync::broadcast::Receiver<RemoteControlEvent>) -> Vec<RemoteControlEvent> {
        let mut out = Vec::new();
        while let Ok(e) = rx.try_recv() {
            out.push(e);
        }
        out
    }
}

/// The payload of a `SUCCESS` `/DeviceResourceManager` answer.
pub fn answer_bytes(response: &PlatformResponse) -> Vec<u8> {
    assert_eq!(response.status, PlatformRequestStatus::Success);
    let body = pb::DeviceResourceManagerResponseMessage::decode(
        response.message.as_ref().expect("an answer").payload.as_slice(),
    )
    .unwrap();
    assert_eq!(body.response_type, Some(1), "always SUCCESS");
    body.payload
}
