use super::*;
use crate::channel::blob::BlobChannel;
use crate::channel::message::MessageChannel;
use crate::channel::testing::pair;

const S: &str = "fileTransfer_1EAA053C-0354-4DF8-AA05-888312CCC57E";

/// The PC's `NamedStreams` over loopback pairs, and the phone's raw Message child.
async fn channel() -> (Arc<NamedStreams>, MessageChannel, BlobChannel) {
    let ((m_pc, _), (m_ph, _)) = pair(&format!("x,1,4,ScenarioId={S}"), 1200);
    let ((b_pc, _), (b_ph, _)) = pair(&format!("x,2,5,ScenarioId={S}"), 1200);
    let (m_pc, m_ph, b_pc, b_ph) = tokio::join!(
        MessageChannel::open(m_pc),
        MessageChannel::open(m_ph),
        BlobChannel::open(b_pc),
        BlobChannel::open(b_ph)
    );

    let pc = PlatformChannel {
        kind: PlatformChannelType::NamedStreams,
        scenario_id: S.to_owned(),
        channel_id: "c".into(),
        message: m_pc.unwrap(),
        blob: Some(b_pc.unwrap()),
        video: None,
        input: None,
        audio: None,
    };

    (NamedStreams::new(pc, Duration::from_secs(5)).unwrap(), m_ph.unwrap(), b_ph.unwrap())
}

struct Nothing;

#[async_trait]
impl StreamReceiver for Nothing {
    async fn destination(&self, _id: &str, _size: u64) -> Option<Box<dyn AsyncWrite + Send + Unpin>> {
        None
    }
}

#[tokio::test]
async fn a_manifest_carries_a_child_of_the_operations_context() {
    let (pc, phone, _blob) = channel().await;
    let mut requests = phone.register(format!("/{S}/ManifestRequest"));
    let operation = TraceContext::new_root(vec![("ypScenarioId".into(), "CDEH.PhoneFiles.FetchData".into())]);
    let entry = ProtoManifestEntry { id: "a".into(), size: 1, metadata: HashMap::new() };

    pc.request_streams(vec![entry], Arc::new(Nothing), &operation).unwrap();
    let m = tokio::time::timeout(Duration::from_secs(5), requests.recv()).await.unwrap().unwrap();

    let trace = ProtoManifest::decode(m.body.as_slice()).unwrap().trace_context.unwrap();
    assert_eq!(trace.trace_id, operation.trace_id);
    assert_eq!(trace.parent_id, operation.correlation_id);
    assert_ne!(trace.correlation_id, operation.correlation_id);
    assert_eq!(trace.trace_state.get("ypScenarioId").map(String::as_str), Some("CDEH.PhoneFiles.FetchData"));
}

#[tokio::test]
async fn the_ack_of_a_manifest_without_a_context_carries_a_fresh_one() {
    let (pc, phone, _blob) = channel().await;
    pc.set_receiver(Arc::new(Nothing));
    let mut acks = phone.register(format!("/{S}/ManifestAck"));
    let manifest = ProtoManifest { entries: HashMap::new(), trace_context: None };

    phone.send(&format!("/{S}/Manifest"), &manifest.encode_to_vec()).unwrap();
    let m = tokio::time::timeout(Duration::from_secs(5), acks.recv()).await.unwrap().unwrap();

    let ack = ProtoManifestAck::decode(m.body.as_slice()).unwrap();
    assert_eq!(ack.status, ProtoManifestStatus::Success as i32);
    let trace = ack.trace_context.unwrap();
    assert_eq!(trace.trace_id.len(), 32);
    assert!(trace.trace_state.is_empty());

    // A manifest that does not parse is acked without a context.
    phone.send(&format!("/{S}/Manifest"), &[0xff, 0xff]).unwrap();
    let m = tokio::time::timeout(Duration::from_secs(5), acks.recv()).await.unwrap().unwrap();

    let ack = ProtoManifestAck::decode(m.body.as_slice()).unwrap();
    assert_eq!(ack.status, ProtoManifestStatus::ParseError as i32);
    assert!(ack.trace_context.is_none());
}

#[tokio::test]
async fn without_a_receiver_every_manifest_is_unready() {
    let (_pc, phone, _blob) = channel().await;
    let mut acks = phone.register(format!("/{S}/ManifestAck"));
    let operation = TraceContext::new_root(Vec::new());
    let manifest = ProtoManifest { entries: HashMap::new(), trace_context: Some(proto_trace(&operation)) };

    phone.send(&format!("/{S}/Manifest"), &manifest.encode_to_vec()).unwrap();
    let m = tokio::time::timeout(Duration::from_secs(5), acks.recv()).await.unwrap().unwrap();

    let ack = ProtoManifestAck::decode(m.body.as_slice()).unwrap();
    assert_eq!(ack.status, ProtoManifestStatus::StreamUnreadyError as i32);
    assert_eq!(ack.trace_context.unwrap().trace_id, operation.trace_id);

    // Also when the manifest does not parse, and then without a context.
    phone.send(&format!("/{S}/Manifest"), &[0xff, 0xff]).unwrap();
    let m = tokio::time::timeout(Duration::from_secs(5), acks.recv()).await.unwrap().unwrap();

    let ack = ProtoManifestAck::decode(m.body.as_slice()).unwrap();
    assert_eq!(ack.status, ProtoManifestStatus::StreamUnreadyError as i32);
    assert!(ack.trace_context.is_none());
}
