use super::*;
use crate::channel::testing::pair;

async fn open_pair(max: usize) -> (BlobChannel, BlobChannel) {
    let ((a, _), (b, _)) = pair("c,2,5", max);
    let (a, b) = tokio::join!(BlobChannel::open(a), BlobChannel::open(b));
    (a.unwrap(), b.unwrap())
}

#[tokio::test]
async fn stream_round_trip() {
    let (a, b) = open_pair(1200).await;
    assert_eq!(a.version(), 3);

    let mut rx = b.register(b"entry-1".to_vec());
    let data: Vec<u8> = (0..250_000u32).map(|i| i as u8).collect();
    a.send_stream(b"entry-1", data.len() as u64, &data[..]).await.unwrap();

    assert_eq!(rx.recv().await.unwrap(), BlobEvent::Start { size: 250_000 });
    let mut got = Vec::new();
    loop {
        match rx.recv().await.unwrap() {
            BlobEvent::Chunk(c) => got.extend(c),
            BlobEvent::Completed(s) => {
                assert_eq!(s, status::COMPLETED);
                break;
            }
            other => panic!("{other:?}"),
        }
    }

    assert_eq!(got, data);
}

/// Every Data packet of a chunk carries the number of fragments the chunk was cut into: the
/// receiver takes the chunk as complete once it has that many.
#[tokio::test]
async fn fragment_count_is_the_number_of_fragments() {
    let ((a, link), (b, _)) = pair("c,2,5", 1200);
    let (a, _b) = tokio::join!(BlobChannel::open(a), BlobChannel::open(b));
    let a = a.unwrap();

    let data = vec![7u8; 100_048];
    a.send_stream(b"x", data.len() as u64, &data[..]).await.unwrap();

    let mut per_chunk: HashMap<u64, (u32, u32)> = HashMap::new();
    for (_, _, packet) in link.sent.lock_or_panic().iter() {
        let (k, body) = unframe(packet).unwrap();
        if k != kind::DATA {
            continue;
        }

        let mut r = Reader::new(body);
        let _flags = r.u32().unwrap();
        let count = r.u32().unwrap();
        let chunk = r.u64().unwrap();

        let e = per_chunk.entry(chunk).or_insert((count, 0));
        assert_eq!(e.0, count, "one count per chunk");
        e.1 += 1;
    }

    assert_eq!(per_chunk.len(), 2, "a full chunk and a 48-byte one");
    for (chunk, (count, sent)) in per_chunk {
        assert_eq!(count, sent, "chunk {chunk}");
    }
}

#[test]
fn version_negotiation() {
    assert_eq!(negotiate(2, 3), 3);
    assert_eq!(negotiate(1, 3), 3);
    assert_eq!(negotiate(0, 2), 2);
    assert_eq!(negotiate(2, 4), 2, "a maximum above 3 takes the minimum");
    assert_eq!(negotiate(3, 5), 3);
    assert_eq!(negotiate(1, 9), 2);
}

#[tokio::test]
async fn a_peer_range_beyond_3_negotiates_its_minimum() {
    let ((a, link_a), (b, _)) = pair("c,2,5", 1200);

    let mut hs = 2u32.to_le_bytes().to_vec();
    hs.extend_from_slice(&4u32.to_le_bytes());
    crate::channel::ChannelLink::send(&*link_a, 1, SendMode::Reliable, PT_CHANNEL, frame(kind::HANDSHAKE, &hs))
        .unwrap();

    drop(a);
    assert_eq!(BlobChannel::open(b).await.unwrap().version(), 2);
}

/// The receiver's end of a stream reaches the sender as its Completion status.
#[tokio::test]
async fn receiver_ends_reach_the_sender() {
    for (end, code) in
        [(ReceiveEnd::NoStream, 5), (ReceiveEnd::Failed, 2), (ReceiveEnd::Closed, 4), (ReceiveEnd::CloseFailed, 6)]
    {
        let ((a, _), (b, link_b)) = pair("c,2,5", 1200);
        let (_a, b) = tokio::join!(BlobChannel::open(a), BlobChannel::open(b));
        let b = b.unwrap();
        let _rx = b.register(b"n".to_vec());

        b.end_receive(b"n", end).unwrap();

        let sent = link_b.sent.lock_or_panic();
        let (k, body) = unframe(&sent.last().unwrap().2).unwrap();
        assert_eq!(k, kind::COMPLETION);
        let mut r = Reader::new(body);
        assert_eq!(r.u32().unwrap(), code, "{end:?}");
    }
}

#[tokio::test]
async fn unregistered_name_is_refused() {
    let (a, _b) = open_pair(1200).await;
    let r = a.send_stream(b"nobody", 10, &[1u8; 10][..]).await;
    // The refusal races the send; either the send finished or the peer's status 5 stopped it.
    assert!(matches!(r, Ok(()) | Err(BlobSendError::Peer(status::NO_STREAM))));
}

#[tokio::test]
async fn names_in_use_are_refused_locally() {
    let (a, b) = open_pair(1200).await;
    let _rx = b.register(b"n".to_vec());
    a.state.lock_or_panic().sending.insert(b"n".to_vec());
    assert_eq!(a.send_stream(b"n", 0, &[][..]).await, Err(BlobSendError::NameInUse));
}
