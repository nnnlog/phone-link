//! File transfer: the capability, the `FILE_SHARE` probe, sending files to the phone (the gate, the
//! permission check and the grant-permission notification, the context handle from tag 3 and from the
//! `GetContextHandle` request, the channel, the phone's messages, cancel), and receiving files (the
//! receive request and its refusals, destinations and name collisions, the completion message, the
//! received notification, incomplete files deleted).

use crate::support::file_transfer_world as world;
use crate::support::platform_world;
use phonelink_engine::config::features::FileTransferConfig;

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use prost::Message;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

use phonelink_engine::features::file_transfer::{
    FileTransfer, FileTransferEvent, SendError, SendOrigin, with_file_transfer_capability,
};
use phonelink_engine::features::shared::nano_channels::ChannelOpenError;
use phonelink_engine::features::shared::notifier::NotificationResponse;
use phonelink_engine::nano::device::{AcceptanceReason, PlatformChannelError};
use phonelink_engine::nano::named_streams::{StreamReceiver, StreamSource, TransferStatus};
use phonelink_engine::proto::file_transfer::v1::{
    FileShareContentMessage, FileShareMessage, NanoDelegateInitRequest, NanoDelegateInitResponse,
};
use phonelink_engine::proto::platform_sdk::v1 as pb;
use phonelink_engine::seams::PlatformRequestStatus;

use platform_world::{phone_id, settle};
use world::{ShareWorld, permission_answer, phone_item};

const SEND: &str = "fileShare_FDF73A34-0C30-4CC0-8165-87FC30DB1E35";
const RECEIVE: &str = "fileShare_2B0F3A34-0C30-4CC0-8165-87FC30DB1E35";
const CONTROL: &str = "/fileTransfer/platform";

/// The `FILE_SHARE` probe's `/DeviceResourceManager` body.
const PROBE_BODY: [u8; 20] = [
    0x08, 0x09, 0x10, 0x01, 0x1a, 0x02, 0x08, 0x01, 0x22, 0x0a, 0x2f, 0x66, 0x69, 0x6c, 0x65, 0x73, 0x68, 0x61, 0x72,
    0x65,
];
/// The silent files permission check.
const SILENT_CHECK_BODY: [u8; 23] = [
    0x08, 0x03, 0x10, 0x01, 0x1a, 0x03, 0x0a, 0x01, 0x02, 0x22, 0x0c, 0x2f, 0x70, 0x65, 0x72, 0x6d, 0x69, 0x73, 0x73,
    0x69, 0x6f, 0x6e, 0x73,
];
const GET: i32 = 1;
const SYNC: i32 = 4;

fn content(id: &str, name: &str, size: i64, status: i32) -> FileShareContentMessage {
    FileShareContentMessage {
        file_id: id.into(),
        file_name: name.into(),
        file_size: size,
        file_hash: String::new(),
        file_status: status,
    }
}

fn control(transfer: &str, scenario: &str, content: Vec<FileShareContentMessage>) -> Vec<u8> {
    FileShareMessage { transfer_id: transfer.into(), scenario: scenario.into(), content, ..Default::default() }
        .encode_to_vec()
}

async fn next_control(
    rx: &mut mpsc::UnboundedReceiver<phonelink_engine::nano::channel::message::InboundMessage>,
) -> FileShareMessage {
    let m = tokio::time::timeout(Duration::from_secs(60), rx.recv()).await.unwrap().unwrap();
    FileShareMessage::decode(m.body.as_slice()).unwrap()
}

/// Writes into a shared buffer.
struct VecWriter(Arc<Mutex<Vec<u8>>>);

impl AsyncWrite for VecWriter {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// The phone's side of a send: collects each stream and reports the end of the transfer.
struct PhoneSink {
    files: Mutex<HashMap<String, Arc<Mutex<Vec<u8>>>>>,
    done: mpsc::UnboundedSender<TransferStatus>,
}

#[async_trait]
impl StreamReceiver for PhoneSink {
    async fn destination(&self, id: &str, _size: u64) -> Option<Box<dyn AsyncWrite + Send + Unpin>> {
        let buf = Arc::new(Mutex::new(Vec::new()));
        self.files.lock().unwrap().insert(id.to_owned(), buf.clone());
        Some(Box::new(VecWriter(buf)))
    }
    fn transfer_done(&self, status: TransferStatus) {
        let _ = self.done.send(status);
    }
}

/// The phone's side of a receive: serves each stream from memory.
struct PhoneSource(HashMap<String, Vec<u8>>);

#[async_trait]
impl StreamSource for PhoneSource {
    async fn open(&self, entry: &pb::ProtoManifestEntry) -> Option<Box<dyn AsyncRead + Send + Unpin>> {
        Some(Box::new(io::Cursor::new(self.0.get(&entry.id)?.clone())))
    }
}

/// Serves five bytes of every stream, then nothing more.
struct StallingSource;

struct Stalling(bool);

impl AsyncRead for Stalling {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.0 {
            return Poll::Pending;
        }

        self.0 = true;
        buf.put_slice(b"12345");
        Poll::Ready(Ok(()))
    }
}

#[async_trait]
impl StreamSource for StallingSource {
    async fn open(&self, _: &pb::ProtoManifestEntry) -> Option<Box<dyn AsyncRead + Send + Unpin>> {
        Some(Box::new(Stalling(false)))
    }
}

fn temp_file(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn spawn_send(
    feature: &Arc<FileTransfer>,
    files: Vec<PathBuf>,
    origin: SendOrigin,
) -> tokio::task::JoinHandle<Result<u32, SendError>> {
    let feature = feature.clone();
    tokio::spawn(async move { feature.send(&phone_id(), files, origin).await })
}

#[test]
fn the_capability_follows_the_receive_option() {
    let base = vec!["DEVICE_STATUS".to_owned(), "FILE_TRANSFER".to_owned()];
    assert_eq!(with_file_transfer_capability(&base, &FileTransferConfig::default()), base);

    let off = FileTransferConfig { phone_to_pc_enabled: false, ..Default::default() };
    assert_eq!(with_file_transfer_capability(&base, &off), vec!["DEVICE_STATUS".to_owned()]);
}

#[tokio::test(start_paused = true)]
async fn an_enabled_android_phone_is_probed_at_start() {
    let world = ShareWorld::new(phone_item()).await;
    let _feature = world.install();
    settle().await;

    let requests = world.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0.encode_to_vec(), PROBE_BODY);
    assert_eq!(
        requests[0].1,
        vec![
            ("ypScenarioId".to_owned(), "CDEH.FileShare.SendDrmRequestForPermission".to_owned()),
            ("ypTriggerId".to_owned(), "ShareServiceStarted".to_owned())
        ]
    );

    // A disabled phone is not probed.
    let world = ShareWorld::new(phone_item()).await;
    world.switches.set_enabled(&phone_id(), false).unwrap();
    let _feature = world.install();
    settle().await;
    assert!(world.requests().is_empty());
}

#[tokio::test(start_paused = true)]
async fn settings_follow_the_capability_and_the_toggle() {
    let world = ShareWorld::new(phone_item()).await;
    let feature = world.install();

    let s = feature.settings(&phone_id()).unwrap();
    assert!(s.share_capable && s.share_toggle_shown && s.share_enabled && s.offered_as_send_target);
    assert!(!s.folder_setting_shown, "shown once a receive was accepted");
    assert_eq!(s.received_files_folder, Some(world.folder.path().join("Mobile Devices")));

    feature.set_share_enabled(&phone_id(), false).unwrap();
    let s = feature.settings(&phone_id()).unwrap();
    assert!(!s.share_enabled && !s.offered_as_send_target);

    // Without the phone's capability nothing is offered.
    let world = ShareWorld::new(phonelink_engine::types::DeviceInfoItem {
        capabilities: Some(vec!["DEVICE_STATUS".into()]),
        ..phone_item()
    })
    .await;
    let feature = world.install();
    let s = feature.settings(&phone_id()).unwrap();
    assert!(!s.share_capable && !s.share_toggle_shown && !s.offered_as_send_target);

    // Not offered while the option is off.
    let world = ShareWorld::new(phone_item()).await;
    let feature = world.install_with(FileTransferConfig { share_enabled: false, ..Default::default() });
    let s = feature.settings(&phone_id()).unwrap();
    assert!(s.share_capable && !s.share_toggle_shown && !s.offered_as_send_target);
}

#[tokio::test(start_paused = true)]
async fn a_send_delivers_the_file_list_then_the_files_the_phone_asks_for() {
    let world = ShareWorld::new(phone_item()).await;
    let feature = world.install();
    world.publish_permission().await;
    settle().await;

    let dir = tempfile::tempdir().unwrap();
    let a: Vec<u8> = (0..250_000u32).map(|i| (i % 253) as u8).collect();
    let files = vec![temp_file(&dir, "a.bin", &a), temp_file(&dir, "b.txt", b"hello"), dir.path().to_path_buf()];

    let send = spawn_send(&feature, files, SendOrigin::Window);

    let opened = world.next_channel().await;
    assert_eq!(opened.scenario, SEND);
    let (phone, mut inbound) = (opened.phone, opened.control);
    let start = next_control(&mut inbound).await;
    assert_eq!(start.scenario, "FileTransferToPhone");
    assert_eq!(start.sender_name, "test-mac");
    assert!(start.is_concurrent_mode);
    assert_eq!(start.session_try_count, 1);
    assert_eq!(start.received_file_count, 0);
    assert!(!start.transfer_id.is_empty());
    let listed: Vec<(&str, i64, i32)> =
        start.content.iter().map(|c| (c.file_name.as_str(), c.file_size, c.file_status)).collect();
    assert_eq!(listed, vec![("a.bin", 250_000, 1), ("b.txt", 5, 1)], "the folder is left out");
    let transfers = feature.transfers(&phone_id());
    assert_eq!(transfers.send.unwrap().bytes_total, 250_005);

    // The phone takes every file at once.
    let (done_tx, mut done) = mpsc::unbounded_channel();
    let sink = Arc::new(PhoneSink { files: Mutex::default(), done: done_tx });
    phone.set_receiver(sink.clone());
    let wanted = start.content.iter().map(|c| content(&c.file_id, &c.file_name, c.file_size, 2)).collect();
    phone.send_message(CONTROL, &control(&start.transfer_id, "FileTransferToPhoneFile", wanted)).unwrap();

    assert_eq!(
        tokio::time::timeout(Duration::from_secs(60), done.recv()).await.unwrap(),
        Some(TransferStatus::Success)
    );

    let got = sink.files.lock().unwrap().clone();
    assert_eq!(*got[&start.content[0].file_id].lock().unwrap(), a);
    assert_eq!(*got[&start.content[1].file_id].lock().unwrap(), b"hello");

    let completed = start.content.iter().map(|c| content(&c.file_id, &c.file_name, c.file_size, 3)).collect();
    phone.send_message(CONTROL, &control(&start.transfer_id, "FileTransferToPhoneCompleted", completed)).unwrap();
    assert_eq!(send.await.unwrap(), Ok(2));
    assert!(feature.transfers(&phone_id()).send.is_none());

    // The silent check went first; the handle came from the earlier publication.
    let requests = world.requests();
    let permission = requests.iter().find(|(m, _)| m.resource_type == 3).unwrap();
    assert_eq!(permission.0.encode_to_vec(), SILENT_CHECK_BODY);
    assert_eq!(permission.1[1], ("ypTriggerId".to_owned(), "CheckFilePermission".to_owned()));
    assert_eq!(world.permission_requests(), vec![(GET, "CheckFilePermission".to_owned())]);
    assert!(world.notifier.posted().is_empty(), "a send from the window posts nothing");
}

#[tokio::test(start_paused = true)]
async fn the_gate_refuses_before_anything_is_sent() {
    let world = ShareWorld::new(phone_item()).await;
    let feature = world.install();
    settle().await;

    let dir = tempfile::tempdir().unwrap();
    let file = temp_file(&dir, "a", b"x");

    assert_eq!(feature.send(&phone_id(), vec![], SendOrigin::Window).await, Err(SendError::FileNotSupported));
    assert_eq!(
        feature.send(&phone_id(), vec![dir.path().join("missing")], SendOrigin::Window).await,
        Err(SendError::FileNotSupported)
    );

    feature.set_share_enabled(&phone_id(), false).unwrap();
    assert_eq!(feature.send(&phone_id(), vec![file.clone()], SendOrigin::Window).await, Err(SendError::NotSupportedPc));
    feature.set_share_enabled(&phone_id(), true).unwrap();
    world.switches.set_enabled(&phone_id(), false).unwrap();
    assert_eq!(feature.send(&phone_id(), vec![file.clone()], SendOrigin::Window).await, Err(SendError::NotSupportedPc));
    settle().await;
    assert_eq!(world.permission_requests(), vec![], "no request went out");

    // A phone without the capability.
    let world = ShareWorld::new(phonelink_engine::types::DeviceInfoItem { capabilities: None, ..phone_item() }).await;
    let feature = world.install();
    assert_eq!(feature.send(&phone_id(), vec![file], SendOrigin::Window).await, Err(SendError::PhoneNotSupported));
}

#[tokio::test(start_paused = true)]
async fn a_phone_without_the_permission_gets_the_grant_notification() {
    let world = ShareWorld::new(phone_item()).await;
    world.answer(|m| (m.resource_type == 3).then(|| permission_answer(false)));
    let feature = world.install();
    settle().await;

    let dir = tempfile::tempdir().unwrap();
    let file = temp_file(&dir, "a", b"x");

    let result = feature.send(&phone_id(), vec![file], SendOrigin::ShareMenu).await;
    assert_eq!(result, Err(SendError::PermissionRequired));

    let posted = world.notifier.posted();
    let grant = posted.iter().find(|n| n.id.starts_with("file_transfer/permission/")).expect("grant notification");
    assert_eq!(grant.actions[0].id, "grant");

    world.responses.route(&grant.id, &grant.user_info, NotificationResponse::Action { action_id: "grant".into() });
    settle().await;
    assert_eq!(
        world.permission_requests(),
        vec![(GET, "CheckFilePermission".to_owned()), (SYNC, "UserGrantFilesPermission".to_owned())]
    );
}

#[tokio::test(start_paused = true)]
async fn without_a_handle_the_send_asks_for_one_and_waits_for_the_publication() {
    let world = Arc::new(ShareWorld::new(phone_item()).await);
    let feature = world.install();
    settle().await;

    let dir = tempfile::tempdir().unwrap();
    let file = temp_file(&dir, "a", b"x");
    let send = spawn_send(&feature, vec![file.clone()], SendOrigin::Window);

    // The phone publishes once it sees the GetContextHandle request.
    loop {
        if world.permission_requests().iter().any(|(t, trigger)| *t == SYNC && trigger == "GetContextHandle") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    tokio::time::sleep(Duration::from_secs(5)).await;
    world.publish_permission().await;

    let opened = world.next_channel().await;
    assert_eq!(opened.scenario, SEND);
    opened.phone.send_message(CONTROL, &control("t", "FileTransferToPhoneRejected", vec![])).unwrap();
    assert_eq!(send.await.unwrap(), Err(SendError::AnotherTransferInProgress));

    // No publication: the send ends after the 120 s wait.
    let world = ShareWorld::new(phone_item()).await;
    let feature = world.install();
    settle().await;

    let started = tokio::time::Instant::now();
    assert_eq!(feature.send(&phone_id(), vec![file], SendOrigin::Window).await, Err(SendError::HandleTimedOut));
    assert!(started.elapsed() >= Duration::from_secs(120));
}

#[tokio::test(start_paused = true)]
async fn channel_refusals_and_a_second_send() {
    let world = ShareWorld::new(phone_item()).await;
    let feature = world.install();
    world.publish_permission().await;
    settle().await;

    let dir = tempfile::tempdir().unwrap();
    let file = temp_file(&dir, "a", b"x");
    world.channels.refusals.lock().unwrap().extend([
        ChannelOpenError::Channel(PlatformChannelError::RemoteScenarioRejectedChannel(AcceptanceReason::NotSupported)),
        ChannelOpenError::Channel(PlatformChannelError::RemoteScenarioRejectedChannel(
            AcceptanceReason::PermissionRevoked,
        )),
        ChannelOpenError::Channel(PlatformChannelError::Timeout),
    ]);

    let phone = phone_id();
    let send = |f: &Arc<FileTransfer>| {
        let (f, files) = (f.clone(), vec![file.clone()]);
        let phone = phone.clone();
        async move { f.send(&phone, files, SendOrigin::Window).await }
    };

    assert_eq!(send(&feature).await, Err(SendError::AnotherTransferInProgress));
    assert_eq!(
        send(&feature).await,
        Err(SendError::ChannelRejected(ChannelOpenError::Channel(
            PlatformChannelError::RemoteScenarioRejectedChannel(AcceptanceReason::PermissionRevoked)
        )))
    );
    assert_eq!(
        send(&feature).await,
        Err(SendError::ChannelFailed(ChannelOpenError::Channel(PlatformChannelError::Timeout)))
    );

    // A locked Mac opens no channel.
    world.w.lock.set(phonelink_engine::edge::LockState::Locked);
    assert_eq!(send(&feature).await, Err(SendError::Permission("ResponseStatus(Locked)".into())));
    world.w.lock.set(phonelink_engine::edge::LockState::Unlocked);

    // A second send while one runs.
    let first = spawn_send(&feature, vec![file.clone()], SendOrigin::Window);
    let opened = world.next_channel().await;
    assert_eq!(send(&feature).await, Err(SendError::AnotherTransferInProgress));

    // Cancel ends the first.
    feature.cancel_send(&phone_id());
    assert_eq!(first.await.unwrap(), Err(SendError::Cancelled));
    settle().await;
    assert!(opened.phone.is_closed(), "the channel is closed at the end");
}

#[tokio::test(start_paused = true)]
async fn a_share_menu_send_shows_its_states_and_outcome() {
    let world = ShareWorld::new(phone_item()).await;
    let feature = world.install();
    world.publish_permission().await;
    settle().await;

    let dir = tempfile::tempdir().unwrap();
    let send = spawn_send(&feature, vec![temp_file(&dir, "a", b"xyz")], SendOrigin::ShareMenu);

    let mut opened = world.next_channel().await;
    let mut inbound = std::mem::replace(&mut opened.control, mpsc::unbounded_channel().1);
    let start = next_control(&mut inbound).await;

    let titles: Vec<String> = world.notifier.posted().iter().map(|n| n.title.clone()).collect();
    assert_eq!(titles.first().map(String::as_str), Some("Connecting"));
    assert!(titles.iter().any(|t| t == "Preparing"), "{titles:?}");
    assert!(world.notifier.posted().iter().all(|n| n.actions.iter().any(|a| a.id == "cancel")));

    // The cancel action ends the send.
    let state = world.notifier.posted().last().unwrap().clone();
    world.responses.route(&state.id, &state.user_info, NotificationResponse::Action { action_id: "cancel".into() });
    assert_eq!(send.await.unwrap(), Err(SendError::Cancelled));
    assert_eq!(world.notifier.posted().last().unwrap().title, "Cancelled");
    assert_eq!(start.content.len(), 1);
}

/// A receive request with session `session`; its answer.
async fn receive_request(world: &ShareWorld, session: &str) -> Option<NanoDelegateInitResponse> {
    let payload = NanoDelegateInitRequest { file_transfer_session_id: session.into() }.encode_to_vec();
    let response = world.phone_drm(GET, "/fileshare_android2cdeh_phase1", &payload, Duration::from_secs(5)).await;
    if response.status != PlatformRequestStatus::Success {
        return None;
    }

    let body = pb::DeviceResourceManagerResponseMessage::decode(response.message.unwrap().payload.as_slice()).unwrap();
    Some(NanoDelegateInitResponse::decode(body.payload.as_slice()).unwrap())
}

#[tokio::test(start_paused = true)]
async fn receive_requests_that_are_refused() {
    let world = ShareWorld::new(phone_item()).await;
    let _feature = world.install();
    settle().await;

    let answer = |r: phonelink_engine::seams::PlatformResponse| {
        let body = pb::DeviceResourceManagerResponseMessage::decode(r.message.unwrap().payload.as_slice()).unwrap();
        NanoDelegateInitResponse::decode(body.payload.as_slice()).unwrap()
    };
    let ok = NanoDelegateInitRequest { file_transfer_session_id: "s".into() }.encode_to_vec();
    let path = "/fileshare_android2cdeh_phase1";

    let r = world.phone_drm(SYNC, path, &ok, Duration::from_secs(5)).await;
    assert_eq!(answer(r).status, 4, "not GET");
    let r = world.phone_drm(GET, path, &[], Duration::from_secs(5)).await;
    assert_eq!(answer(r).status, 3, "empty");
    let r = answer(world.phone_drm(GET, path, &[0xff, 0xff], Duration::from_secs(5)).await);
    assert_eq!(r.status, 2, "does not parse");
    assert!(!r.message.is_empty(), "the parser's text");
    assert!(world.channels.handles.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_receive_pulls_the_files_into_the_folder() {
    let world = ShareWorld::new(phone_item()).await;
    let feature = world.install();
    world.publish_permission().await;
    let mut events = feature.subscribe();
    settle().await;

    let folder = world.folder.path().join("Mobile Devices");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("photo.jpg"), b"old").unwrap();

    let reply = receive_request(&world, "session-1").await.unwrap();
    assert_eq!((reply.file_transfer_session_id.as_str(), reply.status), ("session-1", 1));

    let mut expected = vec![0x0a, 0x09];
    expected.extend_from_slice(b"session-1");
    expected.extend_from_slice(&[0x10, 0x01]);
    assert_eq!(reply.encode_to_vec(), expected);

    // A second request while it runs gets no reply.
    assert!(receive_request(&world, "session-2").await.is_none());

    let opened = world.next_channel().await;
    assert_eq!(opened.scenario, RECEIVE);
    let (phone, mut inbound) = (opened.phone, opened.control);
    let big: Vec<u8> = (0..180_000u32).map(|i| (i % 241) as u8).collect();
    phone.set_sender(Arc::new(PhoneSource(HashMap::from([
        ("f1".to_owned(), big.clone()),
        ("f2".to_owned(), b"note".to_vec()),
        ("f3".to_owned(), Vec::new()),
    ]))));
    let list =
        vec![content("f1", "photo.jpg", 180_000, 1), content("f2", "a.tar.gz", 4, 1), content("f3", "empty", 0, 1)];
    phone.send_message(CONTROL, &control("t-1", "FileTransferToPc", list)).unwrap();

    let done = next_control(&mut inbound).await;
    assert_eq!(done.scenario, "FileTransferToPcCompleted");
    assert_eq!(done.transfer_id, "t-1");
    assert_eq!(done.sender_name, "test-mac");
    assert_eq!(done.received_file_count, 3);
    assert!(!done.is_concurrent_mode);
    assert_eq!(
        done.content.iter().map(|c| (c.file_id.as_str(), c.file_status)).collect::<Vec<_>>(),
        vec![("f1", 3), ("f2", 3), ("f3", 3)]
    );

    settle().await;
    assert_eq!(std::fs::read(folder.join("photo (1).jpg")).unwrap(), big);
    assert_eq!(std::fs::read(folder.join("photo.jpg")).unwrap(), b"old");
    assert_eq!(std::fs::read(folder.join("a.tar.gz")).unwrap(), b"note");
    assert_eq!(std::fs::read(folder.join("empty")).unwrap(), b"");

    let received = world.notifier.posted().into_iter().find(|n| n.id.starts_with("file_transfer/received/")).unwrap();
    assert_eq!(received.body, "3 files received");
    assert!(feature.settings(&phone_id()).unwrap().folder_setting_shown);

    // Clicking it shows the folder.
    ShareWorld::events(&mut events);
    world.responses.route(&received.id, &received.user_info, NotificationResponse::Activated);
    assert_eq!(ShareWorld::events(&mut events), vec![FileTransferEvent::Reveal { path: folder.clone() }]);

    // The receive has ended: a new request is answered.
    assert_eq!(receive_request(&world, "session-3").await.unwrap().status, 1);
}

#[tokio::test(start_paused = true)]
async fn a_cancelled_receive_deletes_what_arrived_and_a_bad_name_ends_it() {
    let world = ShareWorld::new(phone_item()).await;
    let feature = world.install();
    world.publish_permission().await;
    settle().await;
    let folder = world.folder.path().join("Mobile Devices");

    // A name that is not a leaf name.
    assert_eq!(receive_request(&world, "s1").await.unwrap().status, 1);
    let opened = world.next_channel().await;
    opened.phone.send_message(CONTROL, &control("t", "FileTransferToPc", vec![content("f", "../x", 1, 1)])).unwrap();

    // The session ends; a new request is answered.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(feature.transfers(&phone_id()).receive.is_none());

    // A cancel after a file was created.
    assert_eq!(receive_request(&world, "s2").await.unwrap().status, 1);
    let opened = world.next_channel().await;
    let phone = opened.phone;
    phone.set_sender(Arc::new(StallingSource));
    phone.send_message(CONTROL, &control("t", "FileTransferToPc", vec![content("f", "part.bin", 10, 1)])).unwrap();

    // The file is created and its first bytes arrive; the rest never does.
    for _ in 0..100 {
        if folder.join("part.bin").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert!(folder.join("part.bin").exists());
    phone.send_message(CONTROL, &control("t", "FileTransferToPcCancel", vec![])).unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(!folder.join("part.bin").exists(), "the incomplete file is deleted");
    assert!(world.notifier.posted().iter().all(|n| !n.id.starts_with("file_transfer/received/")), "nothing completed");

    // A receive request from a phone that is not in the account gets no reply.
    world.peers.remove(&phone_id());
    settle().await;
    assert!(receive_request(&world, "s3").await.is_none());
}

#[tokio::test(start_paused = true)]
async fn a_receive_without_a_handle_asks_for_one() {
    let world = ShareWorld::new(phone_item()).await;
    let _feature = world.install();
    settle().await;

    assert_eq!(receive_request(&world, "s").await.unwrap().status, 1);
    settle().await;
    assert_eq!(world.permission_requests(), vec![(SYNC, "GetContextHandle".to_owned())]);

    world.publish_permission().await;
    let opened = world.next_channel().await;
    assert_eq!(opened.scenario, RECEIVE);
}
