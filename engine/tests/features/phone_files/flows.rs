//! Phone files: enablement from the phone's configuration, the full sync over the relay and the
//! incremental sync over the files-request channel, the tree (synthetic folders, children, changes),
//! downloads and uploads over the content channel, thumbnails and their cache, create folder, rename
//! and the delete batch, and turning the feature off.

use crate::assert_matches;
use crate::support::file_transfer_world as world;
use crate::support::platform_world;
use phonelink_engine::config::features::PhoneFilesConfig;

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use prost::Message;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use phonelink_engine::features::phone_files::{FilesChannel, FilesChannels, PhoneFiles, PhoneFilesEvent, ROOT};
use phonelink_engine::features::shared::nano_channels::{ChannelOpenError, TransferChannel};
use phonelink_engine::nano::channel::blob::BlobChannel;
use phonelink_engine::nano::channel::message::{InboundMessage, MessageChannel};
use phonelink_engine::nano::channel::testing::pair;
use phonelink_engine::nano::device::{PlatformChannel, PlatformChannelType};
use phonelink_engine::nano::named_streams::{NamedStreams, StreamReceiver, StreamSource, TransferStatus};
use phonelink_engine::platform::{ContextHandle, route};
use phonelink_engine::proto::phone_files::v1 as pf;
use phonelink_engine::proto::platform_sdk::v1 as pb;
use phonelink_engine::seams::PlatformMessenger;
use phonelink_engine::types::{TraceContext, TransportMessage};

use platform_world::{drm_answer, from_phone, phone_id, platform, settle};
use world::{ShareWorld, phone_item};

const FILES_METADATA: i32 = 1;
const GET: i32 = 1;
const UPDATE: i32 = 2;
const DELETE: i32 = 3;
const SYNC: i32 = 4;
const REQUEST_SCENARIO: &str = "filesRequestTransfer_3GAC175E-1576-6FA0-CC27-000534EEE79G";
const CONTENT_SCENARIO: &str = "fileTransfer_1EAA053C-0354-4DF8-AA05-888312CCC57E";

/// A channel the feature opened: its scenario and the phone's end, with the phone's listeners.
struct Opened {
    scenario: String,
    phone: Arc<NamedStreams>,
    requests: HashMap<String, mpsc::UnboundedReceiver<InboundMessage>>,
}

/// [`FilesChannels`] over in-memory channel pairs. The phone's end serves pulls from `source` and takes
/// pushes into `sink` from the moment it opens.
struct FakeChannels {
    opened: mpsc::UnboundedSender<Opened>,
    source: Mutex<Option<Arc<dyn StreamSource>>>,
    sink: Mutex<Option<Arc<dyn StreamReceiver>>>,
    /// Each open's scenario and the trace state of its operation's context.
    traces: Mutex<Vec<(String, TraceState)>>,
    /// Each open's trace id.
    trace_ids: Mutex<Vec<String>>,
    /// The next opens fail with this.
    refuse: Mutex<bool>,
}

async fn named_streams_pair(scenario: &str) -> (Arc<NamedStreams>, Arc<NamedStreams>) {
    let ((m_pc, _), (m_ph, _)) = pair(&format!("x,1,4,ScenarioId={scenario}"), 1200);
    let ((b_pc, _), (b_ph, _)) = pair(&format!("x,2,5,ScenarioId={scenario}"), 1200);
    let (m_pc, m_ph, b_pc, b_ph) = tokio::join!(
        MessageChannel::open(m_pc),
        MessageChannel::open(m_ph),
        BlobChannel::open(b_pc),
        BlobChannel::open(b_ph)
    );

    let channel = |message, blob| PlatformChannel {
        kind: PlatformChannelType::NamedStreams,
        scenario_id: scenario.to_owned(),
        channel_id: "c".into(),
        message,
        blob: Some(blob),
        video: None,
        input: None,
        audio: None,
    };

    let pc = NamedStreams::new(channel(m_pc.unwrap(), b_pc.unwrap()), Duration::from_secs(30)).unwrap();
    let phone = NamedStreams::new(channel(m_ph.unwrap(), b_ph.unwrap()), Duration::from_secs(30)).unwrap();
    (pc, phone)
}

#[async_trait]
impl FilesChannels for FakeChannels {
    async fn open(
        &self,
        _handle: &ContextHandle,
        scenario: &str,
        targets: &[&str],
        trace: &TraceContext,
    ) -> Result<FilesChannel, ChannelOpenError> {
        self.traces.lock().unwrap().push((scenario.to_owned(), trace.trace_state.clone()));
        self.trace_ids.lock().unwrap().push(trace.trace_id.clone());

        if *self.refuse.lock().unwrap() {
            return Err(ChannelOpenError::Channel(phonelink_engine::nano::device::PlatformChannelError::Timeout));
        }

        let (pc, phone) = named_streams_pair(scenario).await;
        if let Some(source) = self.source.lock().unwrap().clone() {
            phone.set_sender(source);
        }
        if let Some(sink) = self.sink.lock().unwrap().clone() {
            phone.set_receiver(sink);
        }

        let listeners = targets.iter().map(|t| (t.to_string(), pc.messages(t))).collect();
        let requests = ["/filesRequest", "/getThumbnailRequest", "/prefetchThumbnailsRequest"]
            .into_iter()
            .map(|t| (t.to_owned(), phone.messages(t)))
            .collect();

        let _ = self.opened.send(Opened { scenario: scenario.to_owned(), phone, requests });
        Ok(FilesChannel { channel: TransferChannel::new(pc, None), listeners })
    }
}

struct FilesWorld {
    s: ShareWorld,
    opened: tokio::sync::Mutex<mpsc::UnboundedReceiver<Opened>>,
    channels: Arc<FakeChannels>,
    dir: tempfile::TempDir,
}

fn meta(id: i64, path: &str, dir: bool) -> pf::FileMetadata {
    pf::FileMetadata {
        id,
        uri: format!("content://media/external/file/{id}"),
        name: path.rsplit('/').next().unwrap().to_owned(),
        path: path.to_owned(),
        file_type: if dir { pf::FileType::None as i32 } else { pf::FileType::Image as i32 },
        size: if dir { 0 } else { 5 },
        created_time: Some(prost_types::Timestamp { seconds: 100, nanos: 0 }),
        modified_time: Some(prost_types::Timestamp { seconds: 200, nanos: 0 }),
        checksum: id * 11,
        is_directory: dir,
    }
}

fn full_reply(items: Vec<pf::FileMetadata>, ending: i64) -> pf::FilesGetResponse {
    pf::FilesGetResponse {
        status: pf::FilesResponseStatus::Success as i32,
        response: Some(pf::files_get_response::Response::FilesMetadataSyncResponse(pf::GetFilesMetadataSyncResponse {
            payload: Some(pf::get_files_metadata_sync_response::Payload::FullSync(pf::FullFileSyncPayload {
                metadata: items,
                ending_sequence_number: ending,
                ..Default::default()
            })),
        })),
        request_id: String::new(),
    }
}

fn status_reply<M: Message>(m: M) -> Option<TransportMessage> {
    Some(drm_answer(&m.encode_to_vec(), Some(1)))
}

fn config(nano: bool) -> pf::FilesConfigurationPayload {
    pf::FilesConfigurationPayload {
        capability: true,
        permission: true,
        thumbnail_supported_file_types: vec![pf::FileType::Image as i32],
        gigabyte_to_byte: 1_000_000_000,
        recycle_bin_path: "Recycle bin - Connected device".into(),
        batch_delete_capability: true,
        is_get_thumbnail_over_nano_mobile_enabled: nano,
        is_files_request_over_nano_capable: nano,
        ..Default::default()
    }
}

/// A request's trace state: `(key, value)` pairs.
type TraceState = Vec<(String, String)>;

fn trace(scenario: &str, trigger: &str) -> TraceState {
    vec![("ypScenarioId".to_owned(), scenario.to_owned()), ("ypTriggerId".to_owned(), trigger.to_owned())]
}

fn tree() -> Vec<pf::FileMetadata> {
    vec![
        meta(1, "/storage/emulated/0/DCIM", true),
        meta(2, "/storage/emulated/0/DCIM/Camera/a.jpg", false),
        meta(3, "/storage/emulated/0/Download", true),
        meta(4, "/storage/emulated/0/Download/longer-name.jpg", false),
        meta(5, "/storage/emulated/0/Download/Sub", true),
        meta(6, "/storage/emulated/0/Download/Sub/b.jpg", false),
    ]
}

impl FilesWorld {
    async fn new() -> Self {
        let s = ShareWorld::new(phone_item()).await;
        let (tx, rx) = mpsc::unbounded_channel();
        FilesWorld {
            s,
            opened: tokio::sync::Mutex::new(rx),
            channels: Arc::new(FakeChannels {
                opened: tx,
                source: Mutex::default(),
                sink: Mutex::default(),
                traces: Mutex::default(),
                trace_ids: Mutex::default(),
                refuse: Mutex::default(),
            }),
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn install(&self) -> Arc<PhoneFiles> {
        let config = PhoneFilesConfig { content_dir: self.dir.path().join("content"), ..Default::default() };
        PhoneFiles::install(&self.s.env, config, &self.s.w.routes, self.channels.clone()).expect("phone files installs")
    }

    /// The phone publishes `additional` under `tag`.
    async fn publish(&self, tag: i32, additional: Vec<u8>) {
        let payload = pb::PubSubPayload { data: Vec::new(), additional }.encode_to_vec();
        let body = pb::MsaepMessage {
            message_tag: tag,
            dcg_client_id: "self-declared".into(),
            message_id: phonelink_engine::types::new_guid_lower(),
            payload,
            platform_major_version: 1,
            platform_minor_version: 1,
        }
        .encode_to_vec();

        let mut request = from_phone(platform(
            &[("_route", route::CONTEXT_PUBLISH), ("ms-content-type", "application/x-binary")],
            &body,
        ));

        request.options.one_way = true;
        self.s.w.pair.phone.request(request).await;
        settle().await;
    }

    async fn publish_files(&self, payload: pf::files_response_payload::Payload) {
        self.publish(4, pf::FilesResponsePayload { payload: Some(payload) }.encode_to_vec()).await;
    }

    async fn next_channel(&self) -> Opened {
        tokio::time::timeout(Duration::from_secs(300), self.opened.lock().await.recv()).await.unwrap().unwrap()
    }

    /// The `FILES_METADATA` requests: `(request_type, payload)`.
    fn files_requests(&self) -> Vec<(i32, Vec<u8>)> {
        self.s
            .requests()
            .into_iter()
            .filter(|(m, _)| m.resource_type == FILES_METADATA)
            .map(|(m, _)| (m.request_type, m.payload))
            .collect()
    }

    /// The `FILES_METADATA` `GET`s: their payload and trace state.
    fn gets(&self) -> Vec<(Vec<u8>, TraceState)> {
        self.s
            .requests()
            .into_iter()
            .filter(|(m, _)| m.resource_type == FILES_METADATA && m.request_type == GET)
            .map(|(m, t)| (m.payload, t))
            .collect()
    }

    /// Installs the feature with the phone answering the config sync, publishes `config` and lets the
    /// full sync over the relay answer `tree()`.
    async fn running(&self, cfg: pf::FilesConfigurationPayload) -> Arc<PhoneFiles> {
        self.s.answer(|m| {
            if m.resource_type != FILES_METADATA {
                return None;
            }
            match m.request_type {
                SYNC => status_reply(pf::FilesSyncResponse { status: 1 }),
                GET => match pf::FilesGetRequest::decode(m.payload.as_slice()).ok()?.request? {
                    pf::files_get_request::Request::FilesMetadataSyncRequest(_) => status_reply(full_reply(tree(), 10)),
                    _ => None,
                },
                _ => None,
            }
        });

        let feature = self.install();
        settle().await;
        self.publish_files(pf::files_response_payload::Payload::FileConfiguration(cfg)).await;
        settle().await;
        feature
    }
}

#[tokio::test(start_paused = true)]
async fn a_phone_without_a_configuration_is_asked_for_one() {
    let world = FilesWorld::new().await;
    world.s.answer(|m| {
        (m.resource_type == FILES_METADATA && m.request_type == SYNC)
            .then(|| status_reply(pf::FilesSyncResponse { status: 1 }))
            .flatten()
    });

    let feature = world.install();
    settle().await;

    assert_eq!(world.files_requests(), vec![(SYNC, vec![0x0a, 0x00])]);
    let (_, trace) = world.s.requests().into_iter().find(|(m, _)| m.resource_type == FILES_METADATA).unwrap();
    assert_eq!(
        trace,
        vec![
            ("ypScenarioId".to_owned(), "CDEH.PhoneFiles.DeviceConfigRequest".to_owned()),
            ("ypTriggerId".to_owned(), "DeviceInitialization".to_owned())
        ]
    );
    assert!(!feature.status(&phone_id()).unwrap().running);
}

#[tokio::test(start_paused = true)]
async fn the_configuration_enables_the_feature_and_the_full_sync_fills_the_tree() {
    let world = FilesWorld::new().await;
    let feature = world.running(config(false)).await;
    let mut events = feature.subscribe();

    let status = feature.status(&phone_id()).unwrap();
    assert!(status.capable && status.enabled && status.running && status.permission_granted);
    assert!(status.initial_sync_done);

    let root = feature.children(&phone_id(), ROOT).unwrap().unwrap();
    assert_eq!(root.len(), 1);
    assert_eq!(
        (root[0].identifier.as_str(), root[0].name.as_str(), root[0].synthetic),
        ("dir:storage", "storage", true)
    );

    let storage = feature.children(&phone_id(), "dir:storage").unwrap().unwrap();
    let mut names: Vec<_> = storage.iter().map(|i| (i.identifier.clone(), i.parent.clone())).collect();
    names.sort();
    assert_eq!(names, vec![("1".into(), "dir:storage".into()), ("3".into(), "dir:storage".into())]);

    let dcim = feature.children(&phone_id(), "1").unwrap().unwrap();
    assert_eq!(dcim[0].identifier, "dir:storage\\DCIM\\Camera");
    let camera = feature.children(&phone_id(), "dir:storage\\DCIM\\Camera").unwrap().unwrap();
    assert_eq!(camera[0].name, "a.jpg");
    assert!(camera[0].has_thumbnail);

    // The full request: FULL_SYNC from -1, not user-initiated.
    let full = world
        .files_requests()
        .into_iter()
        .filter(|(t, _)| *t == GET)
        .map(|(_, p)| pf::FilesGetRequest::decode(p.as_slice()).unwrap())
        .next()
        .unwrap();
    let Some(pf::files_get_request::Request::FilesMetadataSyncRequest(r)) = full.request else { panic!() };
    assert_eq!((r.sync_type, r.seq_num, r.is_user_initiated), (1, Some(-1), Some(false)));
    assert_eq!(world.gets()[0].1, trace("CDEH.PhoneFiles.FullSyncToLocalDB", "FilesServiceStarted"));

    // Storage is kept for the status.
    world
        .publish_files(pf::files_response_payload::Payload::StorageInfo(pf::StorageInfoPayload {
            available_size_bytes: 10,
            total_size_bytes: 20,
        }))
        .await;

    let status = feature.status(&phone_id()).unwrap();
    assert_eq!((status.storage_free, status.storage_total), (10, 20));
    assert_eq!(status.recycle_bin.as_deref(), Some("storage\\Recycle bin - Connected device"));

    while let Ok(e) = events.try_recv() {
        assert!(!matches!(e, PhoneFilesEvent::Running { running: false, .. }));
    }
}

#[tokio::test(start_paused = true)]
async fn a_sync_update_runs_an_incremental_sync_over_the_channel() {
    let world = FilesWorld::new().await;
    let feature = world.running(config(false)).await;
    let generation = feature.generation(&phone_id()).unwrap();

    // The phone now takes metadata requests over the channel.
    world.publish_files(pf::files_response_payload::Payload::FileConfiguration(config(true))).await;
    settle().await;

    let before = world.files_requests().len();
    world
        .publish_files(pf::files_response_payload::Payload::FileSyncUpdate(pf::FilesSyncUpdateMessage {
            sync_type: pf::FilesSyncType::IncrementalSync as i32,
            seq_num: 12,
        }))
        .await;

    // Starting services after the new configuration may have run its own incremental first.
    let mut opened = world.next_channel().await;
    assert_eq!(opened.scenario, REQUEST_SCENARIO);
    let mut requests = opened.requests.remove("/filesRequest").unwrap();
    let m = tokio::time::timeout(Duration::from_secs(60), requests.recv()).await.unwrap().unwrap();
    let request = pf::FilesGetRequest::decode(m.body.as_slice()).unwrap();
    let Some(pf::files_get_request::Request::FilesMetadataSyncRequest(r)) = &request.request else { panic!() };
    assert_eq!((r.sync_type, r.seq_num, r.is_no_engagement_sync), (2, Some(10), Some(false)));

    let mut renamed = meta(2, "/storage/emulated/0/DCIM/Camera/renamed.jpg", false);
    renamed.checksum = 999;
    let reply = pf::FilesGetResponse {
        status: 1,
        response: Some(pf::files_get_response::Response::FilesMetadataSyncResponse(pf::GetFilesMetadataSyncResponse {
            payload: Some(pf::get_files_metadata_sync_response::Payload::IncrementalSync(
                pf::IncrementalFileSyncPayload {
                    sync_operation: vec![
                        pf::IncrementalFileSync {
                            operation: Some(pf::incremental_file_sync::Operation::Update(pf::FileSyncUpdate {
                                metadata: Some(renamed),
                            })),
                        },
                        pf::IncrementalFileSync {
                            operation: Some(pf::incremental_file_sync::Operation::Delete(pf::FileSyncDelete {
                                metadata: Some(pf::FileMetadata { id: 6, ..Default::default() }),
                            })),
                        },
                    ],
                    starting_sequence_number: 10,
                    ending_sequence_number: 12,
                },
            )),
        })),
        request_id: request.request_id.clone(),
    };

    opened.phone.send_message("/filesRequestResponse", &reply.encode_to_vec()).unwrap();
    settle().await;

    // Nothing went over the relay.
    assert!(world.files_requests()[before..].iter().all(|(t, _)| *t != GET));

    let changes = feature.changes(&phone_id(), generation).unwrap().unwrap();
    assert!(changes.updated.iter().any(|i| i.identifier == "2" && i.name == "renamed.jpg"));
    assert_eq!(changes.removed, vec!["6".to_owned()]);

    // An update not past the stored sequence is not sent.
    world
        .publish_files(pf::files_response_payload::Payload::FileSyncUpdate(pf::FilesSyncUpdateMessage {
            sync_type: pf::FilesSyncType::IncrementalSync as i32,
            seq_num: 12,
        }))
        .await;

    assert!(tokio::time::timeout(Duration::from_secs(5), world.next_channel()).await.is_err());
}

/// The phone's side of a download: serves each stream from memory.
struct PhoneSource(Vec<u8>);

#[async_trait]
impl StreamSource for PhoneSource {
    async fn open(&self, _entry: &pb::ProtoManifestEntry) -> Option<Box<dyn AsyncRead + Send + Unpin>> {
        Some(Box::new(io::Cursor::new(self.0.clone())))
    }
}

#[tokio::test(start_paused = true)]
async fn a_download_pulls_the_file_into_a_temporary_file() {
    let world = FilesWorld::new().await;
    let feature = world.running(config(false)).await;
    *world.channels.source.lock().unwrap() = Some(Arc::new(PhoneSource(b"hello".to_vec())));

    let progress = Arc::new(Mutex::new(Vec::new()));
    let p = progress.clone();
    let f = feature.clone();
    let task = tokio::spawn(async move {
        f.download(
            &phone_id(),
            "2",
            true,
            Arc::new(move |d, t| p.lock().unwrap().push((d, t))),
            CancellationToken::new(),
        )
        .await
    });

    let opened = world.next_channel().await;
    assert_eq!(opened.scenario, CONTENT_SCENARIO);

    let (path, item) = tokio::time::timeout(Duration::from_secs(60), task).await.unwrap().unwrap().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"hello");
    assert_eq!(item.identifier, "2");
    assert_eq!(progress.lock().unwrap().last(), Some(&(5, 5)));
    assert_eq!(
        world.channels.traces.lock().unwrap().last().unwrap(),
        &(CONTENT_SCENARIO.to_owned(), trace("CDEH.PhoneFiles.FetchData", "OSCallback"))
    );
}

/// The phone's side of an upload: collects the bytes and the manifest entry size.
struct PhoneSink {
    bytes: Arc<Mutex<Vec<u8>>>,
    done: mpsc::UnboundedSender<TransferStatus>,
}

struct VecWriter(Arc<Mutex<Vec<u8>>>);

impl AsyncWrite for VecWriter {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        self.0.lock().unwrap().extend_from_slice(buf);
        std::task::Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(self: std::pin::Pin<&mut Self>, _: &mut std::task::Context<'_>) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

#[async_trait]
impl StreamReceiver for PhoneSink {
    async fn destination(&self, _id: &str, _size: u64) -> Option<Box<dyn AsyncWrite + Send + Unpin>> {
        Some(Box::new(VecWriter(self.bytes.clone())))
    }
    fn transfer_done(&self, status: TransferStatus) {
        let _ = self.done.send(status);
    }
}

#[tokio::test(start_paused = true)]
async fn an_upload_pushes_the_file_and_records_the_phone_metadata() {
    let world = FilesWorld::new().await;
    let feature = world.running(config(false)).await;
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let (done_tx, mut done) = mpsc::unbounded_channel();
    *world.channels.sink.lock().unwrap() = Some(Arc::new(PhoneSink { bytes: bytes.clone(), done: done_tx }));

    let source = world.dir.path().join("report.pdf");
    std::fs::write(&source, b"12345").unwrap();
    let f = feature.clone();
    let times = phonelink_engine::features::phone_files::FileTimes {
        created_ms: 100_000,
        modified_ms: 200_500,
        accessed_ms: 200_500,
    };
    let task = tokio::spawn(async move {
        f.upload(&phone_id(), "3", "report.pdf", source, Some(times), CancellationToken::new()).await
    });

    let opened = world.next_channel().await;
    assert_eq!(opened.scenario, CONTENT_SCENARIO);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(60), done.recv()).await.unwrap(),
        Some(TransferStatus::Success)
    );
    assert_eq!(*bytes.lock().unwrap(), b"12345");

    let mut created = meta(40, "/storage/emulated/0/Download/report.pdf", false);
    created.modified_time = Some(prost_types::Timestamp { seconds: 200, nanos: 0 });
    let reply = pf::FilesUpdateResponse { status: 1, target_metadata: Some(created) };
    opened
        .phone
        .send_message("/com.microsoft.mmx.agents.files.transport.create_file_response", &reply.encode_to_vec())
        .unwrap();

    let item = tokio::time::timeout(Duration::from_secs(60), task).await.unwrap().unwrap().unwrap();
    assert_eq!((item.identifier.as_str(), item.parent.as_str(), item.name.as_str()), ("40", "3", "report.pdf"));
    assert!(feature.children(&phone_id(), "3").unwrap().unwrap().iter().any(|i| i.identifier == "40"));

    // A name that stays on the Mac is not sent.
    let other = world.dir.path().join(".DS_Store");
    std::fs::write(&other, b"x").unwrap();
    let r = feature.upload(&phone_id(), "3", ".DS_Store", other, None, CancellationToken::new()).await;
    assert_matches!(r, Err(phonelink_engine::features::phone_files::UploadError::Ignored));
}

#[tokio::test(start_paused = true)]
async fn a_thumbnail_over_the_relay_is_cached() {
    let world = FilesWorld::new().await;
    let feature = world.running(config(false)).await;
    let thumbnails = Arc::new(Mutex::new(0));
    let count = thumbnails.clone();
    world.s.answer(move |m| {
        if m.resource_type != FILES_METADATA || m.request_type != GET {
            return None;
        }

        let r = pf::FilesGetRequest::decode(m.payload.as_slice()).ok()?;
        let Some(pf::files_get_request::Request::ThumbnailRequest(t)) = r.request else { return None };
        assert_eq!(
            (t.target_uri.as_str(), t.target_checksum, t.width, t.height),
            ("content://media/external/file/2", 22, 256, 256)
        );
        *count.lock().unwrap() += 1;
        status_reply(pf::FilesGetResponse {
            status: 1,
            response: Some(pf::files_get_response::Response::ThumbnailResponse(pf::GetThumbnailResponse {
                thumbnail: vec![0xff, 0xd8],
                ..Default::default()
            })),
            request_id: String::new(),
        })
    });

    assert_eq!(feature.thumbnail(&phone_id(), "2").await.unwrap(), Some(vec![0xff, 0xd8]));
    assert_eq!(feature.thumbnail(&phone_id(), "2").await.unwrap(), Some(vec![0xff, 0xd8]));
    assert_eq!(*thumbnails.lock().unwrap(), 1);

    let thumbnail_get = world.gets().into_iter().last().unwrap();
    assert_eq!(thumbnail_get.1, trace("CDEH.PhoneFiles.GetThumbnailScenarioId", "ThumbnailRequested"));

    // A folder has none.
    assert_eq!(feature.thumbnail(&phone_id(), "1").await.unwrap(), None);
}

#[tokio::test(start_paused = true)]
async fn create_folder_and_rename_go_over_the_relay() {
    let world = FilesWorld::new().await;
    let feature = world.running(config(false)).await;
    world.s.answer(|m| {
        if m.resource_type != FILES_METADATA || m.request_type != UPDATE {
            return None;
        }

        let r = pf::FilesUpdateRequest::decode(m.payload.as_slice()).ok()?;
        match r.request? {
            pf::files_update_request::Request::CreateFolderRequest(c) => {
                assert_eq!(
                    (c.folder_uri.as_str(), c.folder_checksum, c.folder_name.as_str()),
                    ("content://media/external/file/3", 33, "New")
                );
                status_reply(pf::FilesUpdateResponse {
                    status: 1,
                    target_metadata: Some(meta(50, "/storage/emulated/0/Download/New", true)),
                })
            }
            pf::files_update_request::Request::RenameRequest(r) => {
                assert_eq!(r.target_path, "\\storage\\DCIM\\moved.jpg");
                status_reply(pf::FilesUpdateResponse { status: 1, target_metadata: None })
            }
            _ => None,
        }
    });

    let folder = feature.create_folder(&phone_id(), "3", "New").await.unwrap();
    assert_eq!((folder.identifier.as_str(), folder.parent.as_str(), folder.is_directory), ("50", "3", true));
    let moved = feature.rename(&phone_id(), "2", "1", "moved.jpg").await.unwrap();
    assert_eq!((moved.parent.as_str(), moved.name.as_str()), ("1", "moved.jpg"));

    // A name taken in the target folder is not sent.
    let taken = feature.rename(&phone_id(), "6", "3", "longer-name.jpg").await;
    assert_matches!(taken, Err(phonelink_engine::features::phone_files::OpError::Exists));
}

#[tokio::test(start_paused = true)]
async fn a_folder_delete_is_one_batch_ordered_by_name_length() {
    let world = FilesWorld::new().await;
    let feature = world.running(config(false)).await;
    let mut events = feature.subscribe();
    let batches = Arc::new(Mutex::new(Vec::new()));
    let b = batches.clone();
    world.s.answer(move |m| {
        if m.resource_type != FILES_METADATA || m.request_type != DELETE {
            return None;
        }

        let r = pf::FilesDeleteRequest::decode(m.payload.as_slice()).ok()?;
        assert!(r.is_batch && r.target_uri.is_empty());
        let uris: Vec<String> = r.batch_requests.iter().map(|i| i.target_uri.clone()).collect();
        b.lock().unwrap().push(uris.clone());

        status_reply(pf::FilesDeleteResponse {
            batch_responses: uris.into_iter().map(|u| pf::BatchedDeleteResponse { target_uri: u, status: 1 }).collect(),
            ..Default::default()
        })
    });

    feature.delete(&phone_id(), "3").await.unwrap();

    let batches = batches.lock().unwrap().clone();
    assert_eq!(batches.len(), 1);
    // longer-name.jpg (15), Download (8), b.jpg (5), Sub (3).
    let ids: Vec<&str> = batches[0].iter().map(|u| u.rsplit('/').next().unwrap()).collect();
    assert_eq!(ids, vec!["4", "3", "6", "5"]);

    let storage = feature.children(&phone_id(), "dir:storage").unwrap().unwrap();
    assert_eq!(storage.iter().map(|i| i.identifier.as_str()).collect::<Vec<_>>(), vec!["1"]);
    settle().await;
    let mut notice = false;
    while let Ok(e) = events.try_recv() {
        notice |= matches!(e, PhoneFilesEvent::RecycleBinNotice { .. });
    }
    assert!(notice);
}

#[tokio::test(start_paused = true)]
async fn turning_the_feature_off_removes_the_tree() {
    let world = FilesWorld::new().await;
    let feature = world.running(config(false)).await;
    let mut events = feature.subscribe();

    feature.set_enabled(&phone_id(), false).unwrap();
    settle().await;

    assert!(!feature.status(&phone_id()).unwrap().running);
    assert!(feature.children(&phone_id(), ROOT).is_err());
    let mut stopped = false;
    while let Ok(e) = events.try_recv() {
        stopped |= matches!(e, PhoneFilesEvent::Running { running: false, .. });
    }
    assert!(stopped);

    // On again: the tree comes back with a new full sync.
    feature.set_enabled(&phone_id(), true).unwrap();
    settle().await;
    assert!(feature.status(&phone_id()).unwrap().running);
    assert_eq!(feature.children(&phone_id(), ROOT).unwrap().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn an_expired_sequence_is_asked_again_at_once() {
    let world = FilesWorld::new().await;
    let feature = world.running(config(false)).await;
    let answered = Arc::new(Mutex::new(0));
    let n = answered.clone();
    world.s.answer(move |m| {
        if m.resource_type != FILES_METADATA || m.request_type != GET {
            return None;
        }

        let mut n = n.lock().unwrap();
        *n += 1;

        // First the phone's changes start before the stored sequence (10), then from it.
        let starting = if *n == 1 { 4 } else { 10 };
        status_reply(pf::FilesGetResponse {
            status: 1,
            response: Some(pf::files_get_response::Response::FilesMetadataSyncResponse(
                pf::GetFilesMetadataSyncResponse {
                    payload: Some(pf::get_files_metadata_sync_response::Payload::IncrementalSync(
                        pf::IncrementalFileSyncPayload {
                            sync_operation: vec![pf::IncrementalFileSync {
                                operation: Some(pf::incremental_file_sync::Operation::Create(pf::FileSyncCreate {
                                    metadata: Some(meta(70, "/storage/emulated/0/Download/new.jpg", false)),
                                })),
                            }],
                            starting_sequence_number: starting,
                            ending_sequence_number: 13,
                        },
                    )),
                },
            )),
            request_id: String::new(),
        })
    });

    let before = world.gets().len();
    feature.refresh(&phone_id()).unwrap();
    settle().await;

    let gets = world.gets()[before..].to_vec();
    assert_eq!(gets.len(), 2);
    assert_eq!(gets[0].1, trace("CDEH.PhoneFiles.IncrementalSync", "RefreshClicked"));
    assert_eq!(gets[1].1, trace("CDEH.PhoneFiles.IncrementalSync", "SequenceNumberExpired"));
    let again = pf::FilesGetRequest::decode(gets[1].0.as_slice()).unwrap();
    let Some(pf::files_get_request::Request::FilesMetadataSyncRequest(r)) = again.request else { panic!() };
    assert_eq!((r.sync_type, r.seq_num, r.is_user_initiated), (2, Some(10), Some(false)));
    assert!(feature.children(&phone_id(), "3").unwrap().unwrap().iter().any(|i| i.identifier == "70"));
}

#[tokio::test(start_paused = true)]
async fn a_syncs_channel_attempt_and_relay_request_share_its_trace() {
    let world = FilesWorld::new().await;
    let feature = world.running(config(false)).await;
    world.publish_files(pf::files_response_payload::Payload::FileConfiguration(config(true))).await;
    settle().await;
    *world.channels.refuse.lock().unwrap() = true;
    let before = world.s.w.phone_drm.received().len();

    feature.refresh(&phone_id()).unwrap();
    settle().await;

    let channel_trace = world.channels.trace_ids.lock().unwrap().last().unwrap().clone();
    let get = world.s.w.phone_drm.received()[before..]
        .iter()
        .find(|m| {
            let r = pb::DeviceResourceManagerMessage::decode(m.message.payload.as_slice()).unwrap();
            r.resource_type == FILES_METADATA && r.request_type == GET
        })
        .map(|m| (m.trace.trace_id.clone(), m.trace.parent_id.clone(), m.trace.trace_state.clone()))
        .expect("the relay request after the channel failed");
    assert_eq!(get.0, channel_trace, "one trace for the sync");
    assert!(get.1.is_some(), "the request is a child of the sync's context");
    assert_eq!(get.2, trace("CDEH.PhoneFiles.IncrementalSync", "RefreshClicked"));
}
