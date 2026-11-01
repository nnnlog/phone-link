//! Test world of the photos feature: the App-layer harness (`support/app_world`), the feature
//! environment (`support/feature_world`) and an installed `Photos`, plus builders of the photos
//! messages and a scripted phone for `/media/v1` and `/permission` requests.
//! `tests/main.rs` declares it as `crate::support::photos_world`, next to the `crate::support::app_world` and
//! `crate::support::feature_world` it uses.
#![allow(dead_code)]

use crate::support::common::payload;
use phonelink_engine::config::features::PhotosConfig;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use tokio::sync::{broadcast, oneshot, watch};

use phonelink_engine::app::InboundDisposition;
use phonelink_engine::codec::valueset::{self, ValueSet};
use phonelink_engine::edge::Clock;
use phonelink_engine::features::StoreLocation;
use phonelink_engine::features::photos::{FileAction, Photos, PhotosEvent};
use phonelink_engine::seams::PlatformResponse;
use phonelink_engine::types::{DeviceInfoItem, TransportMessage};

use crate::support::app_world::{
    ASYNC_TRUST_CUSTOM_DATA, Harness, Reply, device_item, message, phone, response_with, result_response, trace,
};
use crate::support::feature_world::{FeatureWorld, feature_world_at};

pub const MEDIA: &str = "/media/v1";
pub const PERMISSION: &str = "/permission";

pub struct World {
    pub h: Harness,
    pub w: FeatureWorld,
    pub photos: Arc<Photos>,
}

impl World {
    /// The feature world's wall clock (2026-10-20T00:00:00Z at start), Unix ms.
    pub fn now_ms(&self) -> i64 {
        self.w.clock.now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
    }

    /// Delivers one inbound message and returns the answer sent.
    pub async fn inbound(&self, msg: TransportMessage) -> ValueSet {
        match self.h.app.handle_inbound(phone(), trace(), msg).await {
            InboundDisposition::Answered(v) => v,
            other => panic!("not answered: {other:?}"),
        }
    }

    pub fn events(&self) -> broadcast::Receiver<PhotosEvent> {
        self.photos.subscribe()
    }

    /// Payloads of the `/media/v1` requests sent so far.
    pub fn media_requests(&self) -> Vec<ValueSet> {
        self.h.messenger.requests_on(MEDIA).iter().map(payload).collect()
    }

    pub fn permission_requests(&self) -> Vec<ValueSet> {
        self.h.messenger.requests_on(PERMISSION).iter().map(payload).collect()
    }

    /// The `/media/v1` requests of one `contentType` (`None`: the delete, which has none).
    pub fn media_of(&self, content_type: Option<&str>) -> Vec<ValueSet> {
        self.media_requests().into_iter().filter(|v| v.str("contentType") == content_type).collect()
    }

    pub fn gallery_ids(&self) -> Vec<i64> {
        self.photos.gallery().unwrap().iter().map(|i| i.id).collect()
    }
}

/// A connected harness with the phone selected (`AsyncTrust`; `custom_data` adds device-list keys),
/// a feature world over in-memory stores, and `Photos` installed with `config`.
pub fn world_with(config: PhotosConfig, custom_data: Option<&str>) -> World {
    world_at(config, StoreLocation::Memory, custom_data)
}

pub fn world() -> World {
    world_with(PhotosConfig::default(), None)
}

pub fn world_at(config: PhotosConfig, stores: StoreLocation, custom_data: Option<&str>) -> World {
    let data = custom_data.unwrap_or(ASYNC_TRUST_CUSTOM_DATA);
    let h = Harness::with_custom_data(Some(data));
    let w = feature_world_at(&h, stores, device_item(Some(data)));
    let photos = Photos::install(&w.env, config).expect("photos installs");
    World { h, w, photos }
}

/// A device-list entry like the harness's, with other `customData`.
pub fn item_with(custom_data: &str) -> DeviceInfoItem {
    device_item(Some(custom_data))
}

fn with(mut vs: ValueSet, extra: ValueSet) -> ValueSet {
    for (k, v) in extra.iter() {
        vs.insert(k, v.clone());
    }
    vs
}

/// A `/legacy/phonecontent` `mediaInfo` request: `_requestId: req-1`, `contractVersion 3.99`,
/// `correlationVector: cv-1`, `syncID` when given, and `extra`.
pub fn media_info(sync_id: Option<&str>, extra: ValueSet) -> TransportMessage {
    let mut vs = ValueSet::new()
        .with("contentType", "mediaInfo")
        .with("contractVersion", 3.99f64)
        .with("correlationVector", "cv-1");
    if let Some(s) = sync_id {
        vs.insert("syncID", s);
    }
    message(&[("_requestId", "req-1"), ("_route", "/legacy/phonecontent")], valueset::encode(&with(vs, extra)))
}

/// The same without any `correlationVector` (payload or header).
pub fn media_info_without_cv(extra: ValueSet) -> TransportMessage {
    let vs = ValueSet::new().with("contentType", "mediaInfo").with("contractVersion", 3.99f64);
    message(&[("_requestId", "req-1"), ("_route", "/legacy/phonecontent")], valueset::encode(&with(vs, extra)))
}

/// The keys of a metadata message.
pub fn metadata(ids: &[i64], times: &[i64], checksums: Option<&[i64]>, sequence: Option<i64>) -> ValueSet {
    let mut vs =
        ValueSet::new().with("metadata", true).with("mediaIds", ids.to_vec()).with("mediaTimes", times.to_vec());
    if let Some(c) = checksums {
        vs.insert("checksums", c.to_vec());
    }
    if let Some(s) = sequence {
        vs.insert("sequenceNumber", s);
    }
    vs
}

/// One item of an item message or thumbnail reply.
#[derive(Debug, Clone)]
pub struct It {
    pub id: i64,
    pub name: String,
    pub modified: i64,
    pub taken: i64,
    pub checksum: i64,
    pub uri: String,
    pub thumbnail: Option<Vec<u8>>,
    pub action: i32,
}

/// An added item with a 3-byte thumbnail, taken and modified at `at`.
pub fn it(id: i64, at: i64) -> It {
    It {
        id,
        name: format!("IMG_{id}.jpg"),
        modified: at,
        taken: at,
        checksum: id * 10,
        uri: format!("content://media/{id}"),
        thumbnail: Some(vec![id as u8, 1, 2]),
        action: 0,
    }
}

impl It {
    pub fn no_thumb(mut self) -> Self {
        self.thumbnail = None;
        self
    }
    pub fn removed(mut self) -> Self {
        self.action = 2;
        self
    }
    pub fn checksum(mut self, c: i64) -> Self {
        self.checksum = c;
        self
    }
    pub fn thumb(mut self, t: &[u8]) -> Self {
        self.thumbnail = Some(t.to_vec());
        self
    }
    pub fn times(mut self, taken: i64, modified: i64) -> Self {
        self.taken = taken;
        self.modified = modified;
        self
    }
}

/// The parallel item arrays of `items`.
pub fn items(items: &[It]) -> ValueSet {
    let mut blob = Vec::new();
    let mut sizes = Vec::new();
    for i in items {
        match &i.thumbnail {
            Some(t) if i.action != 2 => {
                blob.extend_from_slice(t);
                sizes.push(t.len() as i64);
            }
            _ => sizes.push(0),
        }
    }

    ValueSet::new()
        .with("mediaIds", items.iter().map(|i| i.id).collect::<Vec<_>>())
        .with("mediaNames", items.iter().map(|i| i.name.clone()).collect::<Vec<_>>())
        .with("mediaTimes", items.iter().map(|i| i.modified).collect::<Vec<_>>())
        .with("mediaTakenTimes", items.iter().map(|i| i.taken).collect::<Vec<_>>())
        .with("mediaOrientations", items.iter().map(|_| "0").collect::<Vec<_>>())
        .with("mediaMimeTypes", items.iter().map(|_| "image/jpeg").collect::<Vec<_>>())
        .with("mediaHeights", items.iter().map(|_| 4032i64).collect::<Vec<_>>())
        .with("mediaWidths", items.iter().map(|_| 3024i64).collect::<Vec<_>>())
        .with("mediaSizes", items.iter().map(|_| 2_000_000i64).collect::<Vec<_>>())
        .with("mediaActions", items.iter().map(|i| i.action).collect::<Vec<_>>())
        .with("mediaURIs", items.iter().map(|i| i.uri.clone()).collect::<Vec<_>>())
        .with("checksums", items.iter().map(|i| i.checksum).collect::<Vec<_>>())
        .with("mediaThumbnails", blob)
        .with("mediaThumbnailSizes", sizes)
}

/// A successful thumbnail reply carrying `items`.
pub fn thumbnail_reply(list: &[It]) -> PlatformResponse {
    response_with(items(list).with("result", 0i32))
}

/// A successful full-resolution reply.
pub fn full_reply(images: &[(i64, &[u8])]) -> PlatformResponse {
    let mut content = Vec::new();
    for (_, b) in images {
        content.extend_from_slice(b);
    }

    response_with(
        ValueSet::new()
            .with("result", 0i32)
            .with("mediaIds", images.iter().map(|(i, _)| *i).collect::<Vec<_>>())
            .with("mediaSizes", images.iter().map(|(_, b)| b.len() as i64).collect::<Vec<_>>())
            .with("content", content),
    )
}

/// Stores items in the cache through an item message processed on its own.
pub async fn seed(world: &World, list: &[It]) {
    let answer = world.inbound(media_info(Some("seed"), items(list))).await;
    assert_eq!(answer, ValueSet::new().with("result", 0i32), "seed items accepted");
}

/// Gives cached item `id` its full bytes through the fetch of a Copy, which `phone` answers with
/// `bytes`; no other `/media/v1` request may be held meanwhile.
pub async fn store_full_image(world: &World, phone: &Phone, id: i64, bytes: &[u8]) {
    let photos = world.photos.clone();
    let fetch = tokio::spawn(async move { photos.full_image_for_action(id, FileAction::Copy).await });
    wait_held(phone, 1).await;
    let held = phone.take();
    assert_eq!((held.content_type(), held.ids()), (Some("photos"), vec![id]));
    held.answer(full_reply(&[(id, bytes)]));
    assert_eq!(fetch.await.unwrap().unwrap().as_deref(), Some(bytes), "the full image of {id} is stored");
}

/// One `/media/v1` request held by the scripted phone.
pub struct Held {
    pub payload: ValueSet,
    pub reply: oneshot::Sender<PlatformResponse>,
}

/// A phone that holds every `/media/v1` request until the test answers it; other routes are
/// answered `{ result: 0 }` unless a code is queued for `/permission`.
pub struct Phone {
    pub held: Mutex<VecDeque<Held>>,
    pub permission_codes: Mutex<VecDeque<i32>>,
    /// The number of held requests.
    count: watch::Sender<usize>,
}

impl Phone {
    pub fn install(world: &World) -> Arc<Self> {
        let phone = Arc::new(Self {
            held: Mutex::new(VecDeque::new()),
            permission_codes: Mutex::new(VecDeque::new()),
            count: watch::channel(0).0,
        });

        let p = phone.clone();
        world.h.messenger.set_responder(move |req| {
            let route = req.message.route().unwrap_or_default().to_string();

            if route == MEDIA {
                let (tx, rx) = oneshot::channel();
                let n = {
                    let mut held = p.held.lock().unwrap();
                    held.push_back(Held { payload: payload(req), reply: tx });
                    held.len()
                };
                p.count.send_replace(n);
                return Reply::Later(rx);
            }

            if route == PERMISSION {
                if let Some(code) = p.permission_codes.lock().unwrap().pop_front() {
                    return Reply::Now(result_response(code));
                }
            }
            Reply::Now(result_response(0))
        });

        phone
    }

    /// Takes the oldest held request.
    pub fn take(&self) -> Held {
        let (held, n) = {
            let mut q = self.held.lock().unwrap();
            (q.pop_front().expect("a /media/v1 request is held"), q.len())
        };
        self.count.send_replace(n);
        held
    }

    pub fn held_count(&self) -> usize {
        self.held.lock().unwrap().len()
    }

    pub fn queue_permission(&self, code: i32) {
        self.permission_codes.lock().unwrap().push_back(code);
    }
}

impl Held {
    pub fn ids(&self) -> Vec<i64> {
        self.payload.i64_array("mediaIds").unwrap().to_vec()
    }
    pub fn content_type(&self) -> Option<&str> {
        self.payload.str("contentType")
    }
    pub fn answer(self, response: PlatformResponse) {
        let _ = self.reply.send(response);
    }
}

/// Lets spawned tasks run (paused time: advances only when everything idles).
pub async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

/// Waits until `n` `/media/v1` requests are held (paused time advances while everything idles).
pub async fn wait_held(phone: &Phone, n: usize) {
    let mut rx = phone.count.subscribe();
    if tokio::time::timeout(Duration::from_secs(600), rx.wait_for(|c| *c >= n)).await.is_err() {
        panic!("{n} /media/v1 requests were not sent (held {})", phone.held_count());
    }
}

/// Drains the events received so far.
pub fn drain(rx: &mut broadcast::Receiver<PhotosEvent>) -> Vec<PhotosEvent> {
    let mut v = Vec::new();
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}
