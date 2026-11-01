//! The wallpaper feature on one session: the device-status session world
//! (`support/device_status_world` — the platform `World`, an `AppLayer` over it with the World's
//! phone selected, a phone-side App responder) with the wallpaper feature installed over it and a
//! desktop picture the test sets. `tests/main.rs` declares it as `crate::support::wallpaper_world`, next to the
//! `crate::support::platform_world` and `crate::support::device_status_world` it uses.
#![allow(dead_code)]

use phonelink_engine::config::features::WallpaperConfig;
use std::sync::Arc;

use prost::Message;
use tokio::sync::broadcast;

use phonelink_engine::app::{ConnectKind, ConnectSendResult};
use phonelink_engine::codec::valueset::{Value, ValueSet};
use phonelink_engine::features::wallpaper::edge::standins::FixedPcWallpaper;
use phonelink_engine::features::wallpaper::edge::testing::TINY_JPEG;
use phonelink_engine::features::wallpaper::{
    PcWallpaper, PhoneLinkState, ShownWallpaper, StoredWallpaper, Wallpaper, WallpaperCopy, WallpaperEvent,
};
use phonelink_engine::platform::route;
use phonelink_engine::proto::device_info::v1::{
    WallpaperRequestMessage, WallpaperResponseMessage, WallpaperResponseMetadata, WallpaperType,
};
use phonelink_engine::proto::platform_sdk::v1 as pb;
use phonelink_engine::seams::PlatformMessenger;
use phonelink_engine::types::{DeviceInfoItem, HeaderMap, MessageType, TransportMessage};

use crate::support::device_status_world::{StatusWorld, phone_item};
use crate::support::platform_world::phone_id;
use crate::support::platform_world::{drm_answer, from_phone};

pub struct WallpaperWorld {
    pub sw: StatusWorld,
    pub pc: Arc<FixedPcWallpaper>,
}

impl WallpaperWorld {
    /// The World phone (`item`), selected and not connected; the Mac's desktop picture is a JPEG.
    pub fn new(item: DeviceInfoItem) -> Self {
        Self {
            sw: StatusWorld::new(item),
            pc: FixedPcWallpaper::new(PcWallpaper::Picture { jpeg: TINY_JPEG.to_vec() }),
        }
    }

    /// The default Android phone.
    pub fn android() -> Self {
        Self::new(phone_item())
    }

    pub fn install(&self) -> Arc<Wallpaper> {
        self.install_with(WallpaperConfig::default())
    }

    pub fn install_with(&self, config: WallpaperConfig) -> Arc<Wallpaper> {
        Wallpaper::install(
            &self.sw.env,
            config,
            self.sw.w.routes.msaep.clone(),
            self.sw.w.routes.drm.clone(),
            self.pc.clone(),
        )
        .expect("wallpaper installs")
    }

    /// The phone answers every PC `/DeviceResourceManager` request with `reply` (`SUCCESS`).
    pub fn phone_answers_pulls(&self, reply: &WallpaperResponseMessage) {
        self.sw.phone_answers_pulls(reply.encode_to_vec());
    }

    /// The pulls the phone received.
    pub fn pulls(&self) -> Vec<pb::DeviceResourceManagerMessage> {
        self.sw.pulls()
    }

    /// The payload of the `n`th pull.
    pub fn pull_payload(&self, n: usize) -> WallpaperRequestMessage {
        WallpaperRequestMessage::decode(self.pulls()[n].payload.as_slice()).unwrap()
    }

    /// The phone answers the connect with `reply` (and every other App request with `{result: 0}`),
    /// then the PC sends one background connect; returns the connect's request.
    pub async fn connect(&self, reply: ValueSet) -> ValueSet {
        self.sw.phone_app.set_answer(move |route, _| match route {
            "/connect/v1" => Some(reply.clone()),
            _ => Some(ValueSet::new().with("result", 0i32)),
        });

        let before = self.sw.phone_app.received_on("/connect/v1").len();
        match self.sw.app.send_connect(ConnectKind::Background, "Test").await {
            ConnectSendResult::Sent(_) => {}
            other => panic!("connect not sent: {other:?}"),
        }

        crate::support::platform_world::settle().await;
        self.sw.phone_app.received_on("/connect/v1")[before].clone()
    }

    /// A `wallpaper` message from the phone with `keys` (plus `contentType`, `contractVersion`,
    /// `correlationVector`); returns the PC's answer.
    pub async fn phone_sends(&self, keys: ValueSet) -> ValueSet {
        self.sw.phone_sends("wallpaper", keys).await
    }

    /// A `wallpaper` message from the phone with exactly `payload`, answered.
    pub async fn phone_sends_raw(&self, payload: ValueSet) -> ValueSet {
        let message = TransportMessage::new(
            MessageType::App,
            HeaderMap::from_pairs([("_route", "/legacy/phonecontent")]).unwrap(),
            phonelink_engine::codec::valueset::encode(&payload),
        );

        let response = self.sw.w.pair.phone.request(from_phone(message)).await;
        let message = response.message.expect("the PC answers phone content");
        phonelink_engine::codec::valueset::decode(&message.payload).unwrap()
    }

    /// The phone asks for the Mac's wallpaper; the decoded answer, or `None` when none came.
    pub async fn phone_asks(&self, resource_type: i32, path: &str) -> Option<WallpaperResponseMessage> {
        let response = self.sw.phone_drm_request(resource_type, 1, path, b"").await;
        let message = response.message?;
        let body = pb::DeviceResourceManagerResponseMessage::decode(message.payload.as_slice()).unwrap();
        assert_eq!(body.response_type, Some(1), "SUCCESS");
        Some(WallpaperResponseMessage::decode(body.payload.as_slice()).unwrap())
    }

    /// The phone publishes on MSAEP tag 5.
    pub async fn phone_publishes_tag_5(&self) {
        self.sw.phone_publishes(5, "m-5", b"\x08\x01".to_vec()).await;
    }
}

/// A full content item.
pub fn item(id: i64, wallpaper_type: i64, bytes: &[u8], sequence: i64) -> ValueSet {
    ValueSet::new()
        .with("wallpaperId", id)
        .with("wallpaperType", wallpaper_type)
        .with("wallpaper", Value::UInt8Array(bytes.to_vec()))
        .with("sequenceNumber", sequence)
}

pub fn entry(t: WallpaperType, id: i64, payload: Option<&[u8]>) -> WallpaperResponseMetadata {
    WallpaperResponseMetadata { id, wallpaper_type: t as i32, payload: payload.map(<[u8]>::to_vec) }
}

pub fn ok_reply(wallpapers: Vec<WallpaperResponseMetadata>) -> WallpaperResponseMessage {
    WallpaperResponseMessage { wallpapers, status: 1 }
}

pub fn events(rx: &mut broadcast::Receiver<WallpaperEvent>) -> Vec<WallpaperEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

pub fn shown(w: &Wallpaper) -> Option<ShownWallpaper> {
    w.shown(&phone_id()).unwrap()
}

pub fn stored(w: &Wallpaper, copy: WallpaperCopy, t: i64) -> Option<StoredWallpaper> {
    w.stored(&phone_id(), copy, t).unwrap()
}

pub fn state(w: &Wallpaper) -> PhoneLinkState {
    w.phone_link_state(&phone_id()).unwrap()
}

/// The phone's answer of a PC request on `/DeviceResourceManager` with `payload`.
pub fn drm(payload: &[u8]) -> TransportMessage {
    drm_answer(payload, Some(1))
}

pub const CONTEXT_PUBLISH: &str = route::CONTEXT_PUBLISH;
