use super::*;
use crate::channel::testing::pair;

fn raw(announcement: &str) -> RawChannel {
    pair(announcement, 1000).0.0
}

#[test]
fn matching_by_scenario_consumer_and_type() {
    let mut d = Dispatch::default();
    let (tx, mut rx) = mpsc::unbounded_channel();
    d.claims.push(Claim {
        scenario: "s".into(),
        consumer: "c1".into(),
        expected: vec![ChannelType::Message, ChannelType::Blob],
        tx,
    });

    d.offer(raw("x,1,4,ScenarioId=s&ConsumerId=other"));
    assert_eq!(d.unclaimed.len(), 1, "another consumer");

    d.offer(raw("x,2,4,ScenarioId=s&ConsumerId=c1"));
    d.offer(raw("x,3,5,ScenarioId=s"));
    assert!(rx.try_recv().unwrap().is_ok());
    assert!(rx.try_recv().unwrap().is_ok());
    assert!(d.claims.is_empty(), "filled");

    d.offer(raw("x,4,1,ScenarioId=s"));
    d.offer(raw("x,5,7,ScenarioId=s"));
    assert_eq!(d.unclaimed.len(), 3, "a video target, and an audio source the PC does not wrap");
}

#[test]
fn scopes() {
    assert_eq!(PlatformChannelType::NamedStreams.scope_prefix("s"), "/s/NamedStreams");
    assert_eq!(PlatformChannelType::AppRemote.scope_prefix("s"), "/s/Messaging");
    assert_eq!(PlatformChannelType::InternalMessaging.scope_prefix("s"), "/InternalMessaging");
    assert_eq!(PlatformChannelType::AppRemote.expected().len(), 5);
}
