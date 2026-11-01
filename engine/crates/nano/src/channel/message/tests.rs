use super::*;
use crate::channel::testing::pair;

#[test]
fn fragmentation() {
    let f = fragments(0, 7, b"/a/b", &[9u8; 100], 36 + 40);
    assert_eq!(f.len(), 3, "104 bytes in 40-byte fragments");

    let first = Fragment::decode(unframe(&f[0]).unwrap().1).unwrap();
    assert_eq!((first.fragment_count, first.target_length, first.body_length, first.offset), (3, 4, 100, 0));

    let last = Fragment::decode(unframe(&f[2]).unwrap().1).unwrap();
    assert_eq!((last.offset, last.data.len()), (80, 24));

    assert_eq!(fragments(0, 1, b"", b"", 100).len(), 1, "an empty message is one fragment");
}

#[tokio::test]
async fn handshake_send_and_receive() {
    let ((a, link_a), (b, _)) = pair("c,1,4,ScenarioId=s", 80);
    let (a, b) = tokio::join!(MessageChannel::open(a), MessageChannel::open(b));
    let (a, b) = (a.unwrap(), b.unwrap());

    assert_eq!(link_a.sent.lock_or_panic()[0].2, frame(kind::HANDSHAKE, &[1, 0, 0, 0]));

    let mut rx = b.register("/s/NamedStreams/x");
    a.send("/s/NamedStreams/x", &[5u8; 300]).unwrap();

    let m = rx.recv().await.unwrap();
    assert_eq!((m.target.as_str(), m.body.len()), ("/s/NamedStreams/x", 300));
    assert!(link_a.sent.lock_or_panic().iter().all(|(mode, _, _)| *mode == SendMode::Reliable));
}

/// The fields of the last Data packet `link` sent, once it has sent `count` packets.
async fn sent_after(link: &crate::channel::testing::Loopback, count: usize) -> Vec<u8> {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Some((_, _, bytes)) = link.sent.lock_or_panic().get(count - 1) {
                return bytes.clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("sent in time")
}

#[test]
fn scope_matching() {
    let target = |t| target_segments(t).unwrap();
    let ab = Scope::of("/a/b");

    for t in ["/a/b", "/a/b/", "/a/b/c", "/a/b?x=/y", "/a/b#f", "//a//b"] {
        assert!(ab.matches(&target(t)), "{t}");
    }
    for t in ["/a/bc", "/a", "/a/c/b", "/A/b"] {
        assert!(!ab.matches(&target(t)), "{t}");
    }

    assert_eq!(target_segments("a/b"), None);
    assert_eq!(target_segments("?/a"), None);

    let dotted = Scope::of("/platform.message");
    assert!(dotted.matches(&target("/platform.message")));
    assert!(dotted.matches(&target("/platformXmessage/sub")));
    assert!(!dotted.matches(&target("/platform\nmessage")));
    assert!(!dotted.matches(&target("/platformmessage")));

    assert!(Scope::of("/").matches(&target("/anything")), "the root scope holds every path");

    for bad in ["", "a/b", "/a+", "/a/(b)", "/x?y"] {
        assert!(Scope::parse(bad).is_err(), "{bad}");
    }
}

#[tokio::test]
#[should_panic(expected = "does not start with")]
async fn registering_a_scope_outside_the_root_panics() {
    let ((a, _), (b, _)) = pair("c,1,4", 200);
    let (a, _b) = tokio::join!(MessageChannel::open(a), MessageChannel::open(b));
    a.unwrap().register("relative");
}

#[tokio::test]
async fn the_longest_segment_prefix_receives_the_full_target() {
    let ((a, _), (b, _)) = pair("c,1,4", 200);
    let (a, b) = tokio::join!(MessageChannel::open(a), MessageChannel::open(b));
    let (a, b) = (a.unwrap(), b.unwrap());

    let mut short = b.register("/s/Messaging");
    let mut long = b.register("/s/Messaging/drag");

    a.send("/s/Messaging/drag/startPhone?x", b"1").unwrap();
    a.send("/s/Messaging/other", b"2").unwrap();
    a.send("/s/Messagingx", b"3").unwrap();
    a.send("/s/Messaging/drag", b"4").unwrap();

    let m = long.recv().await.unwrap();
    assert_eq!((m.target.as_str(), m.body.as_slice()), ("/s/Messaging/drag/startPhone?x", &b"1"[..]));
    let m = short.recv().await.unwrap();
    assert_eq!((m.target.as_str(), m.body.as_slice()), ("/s/Messaging/other", &b"2"[..]));
    assert_eq!(long.recv().await.unwrap().body, b"4", "/s/Messagingx reaches no scope");

    // A handler whose receiver is gone leaves its messages to the next longest scope.
    drop(long);
    a.send("/s/Messaging/drag/cancel", b"5").unwrap();
    assert_eq!(short.recv().await.unwrap().target, "/s/Messaging/drag/cancel");

    b.unregister("/s/Messaging/");
    a.send("/s/Messaging/other", b"6").unwrap();
    assert!(short.recv().await.is_none(), "unregistered: its sender is dropped");
}

#[tokio::test]
async fn a_target_that_is_not_a_path_is_answered_with_an_error() {
    let ((a, _), (b, link_b)) = pair("c,1,4", 200);
    let (a, b) = tokio::join!(MessageChannel::open(a), MessageChannel::open(b));
    let (a, _b) = (a.unwrap(), b.unwrap());

    for f in fragments(flags::EXPECTS_COMPLETION, 9, b"nobody", b"x", 200) {
        a.sender.send(SendMode::Reliable, PT_CHANNEL, f).unwrap();
    }

    let reply = Fragment::decode(unframe(&sent_after(&link_b, 2).await).unwrap().1).unwrap();
    assert_eq!((reply.flags, reply.message_id, reply.target_length, reply.body_length), (10, 9, 0, 0));
}

#[tokio::test]
async fn no_handler_completion() {
    let ((a, _), (b, link_b)) = pair("c,1,4", 200);
    let (a, b) = tokio::join!(MessageChannel::open(a), MessageChannel::open(b));
    let (a, _b) = (a.unwrap(), b.unwrap());

    for f in fragments(flags::EXPECTS_COMPLETION, 42, b"/nobody", b"x", 200) {
        a.sender.send(SendMode::Reliable, PT_CHANNEL, f).unwrap();
    }

    let bytes = sent_after(&link_b, 2).await;
    let sent = link_b.sent.lock_or_panic();
    let (mode, pt, _) = sent.last().unwrap();
    assert_eq!((*mode, *pt), (SendMode::Reliable, PT_CHANNEL));
    #[rustfmt::skip]
        assert_eq!(bytes, &[
            2, 0, 0, 0, 28, 0, 0, 0, // Data, 28-byte body
            6, 0, 0, 0, 42, 0, 0, 0, 1, 0, 0, 0, // flags 6, id 42, one fragment
            0, 0, 0, 0, 0, 0, 0, 0, // empty target, empty body
            0, 0, 0, 0, 0, 0, 0, 0, // offset 0, length 0
        ]);
}

#[tokio::test]
async fn wrong_version_fails() {
    let ((a, link_a), (b, _)) = pair("c,1,4", 200);

    crate::channel::ChannelLink::send(
        &*link_a,
        1,
        SendMode::Reliable,
        PT_CHANNEL,
        frame(kind::HANDSHAKE, &2u32.to_le_bytes()),
    )
    .unwrap();

    drop(a);
    assert!(matches!(MessageChannel::open(b).await, Err(ChannelError::Handshake(_))));
}
