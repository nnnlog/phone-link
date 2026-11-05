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

#[test]
fn peer_results_map_to_creation_failures() {
    use AcceptanceReason as R;
    use ChannelAcceptanceStatus as Why;
    use PlatformChannelError as E;
    use ProtoCreatePlatformChannelStatus as Code;

    let cases = [
        (Code::Success as i32, Why::ContextSourceDropped as i32, None),
        (
            Code::NoRemoteScenarioAvailable as i32,
            Why::NotSupported as i32,
            Some(E::NoRemoteScenarioAvailable(R::NotSupported)),
        ),
        (
            Code::RemoteScenarioRejectedChannel as i32,
            Why::MobileDataDisabled as i32,
            Some(E::RemoteScenarioRejectedChannel(R::MobileDataDisabled)),
        ),
        (Code::Failure as i32, Why::AppDisabled as i32, Some(E::PartnerChannelCreationFailure(R::AppDisabled))),
        (Code::Failure as i32, 99, Some(E::PartnerChannelCreationFailure(R::Unspecified))),
        (Code::ContextSourceDropped as i32, Why::Unspecified as i32, Some(E::ContextSourceDropped(R::Unspecified))),
        // A failure whose reason is a dropped context source is that, whatever its code.
        (
            Code::Failure as i32,
            Why::ContextSourceDropped as i32,
            Some(E::ContextSourceDropped(R::ContextSourceDropped)),
        ),
        (
            Code::RemoteScenarioRejectedChannel as i32,
            Why::ContextSourceDropped as i32,
            Some(E::ContextSourceDropped(R::ContextSourceDropped)),
        ),
        (
            Code::Unspecified as i32,
            Why::ContextSourceDropped as i32,
            Some(E::ContextSourceDropped(R::ContextSourceDropped)),
        ),
    ];

    for (code, reason, expected) in cases {
        let result =
            ProtoPeerPlatformChannelCreationResult { result_code: code, result_reason: reason, ..Default::default() };
        assert_eq!(creation_failure(&result), expected, "code {code}, reason {reason}");
    }
}

fn partner() -> DcgClient {
    DcgClient::new("phone", phonelink_core::types::Environment::Prod)
}

/// A device connection over a connection without sockets, the phone's end of its message channel, and
/// whether the connection was asked to close.
async fn offline_device() -> (DeviceConnection, MessageChannel, watch::Receiver<bool>) {
    let announcement =
        format!("Microsoft::Basix::Dct::Channel::Class::Messaging,1,4,ScenarioId={}", property::DEVICE_CONNECTION);
    let ((pc, _), (phone, _)) = pair(&announcement, 1000);
    let (connection, close_asked) = crate::connection::testing::offline(vec![pc]);

    let (device, phone) = tokio::join!(
        DeviceConnection::complete(partner(), connection, NanoConfig::default()),
        MessageChannel::open(phone),
    );
    (device.unwrap(), phone.unwrap(), close_asked)
}

/// The partner's context source, and a handle on it.
fn context() -> (Arc<phonelink_transport::platform::ContextHub>, ContextHandle) {
    let hub = phonelink_transport::platform::ContextHub::new(
        DcgClient::new("pc", phonelink_core::types::Environment::Prod),
        Weak::<phonelink_transport::platform::Router>::new(),
        Duration::from_secs(1),
    );
    hub.created(&partner());
    let handle = hub.handle(&partner()).unwrap();
    (hub, handle)
}

#[tokio::test]
async fn an_open_connection_closes_when_its_context_handle_drops() {
    let (device, _phone, mut close_asked) = offline_device().await;
    let (hub, handle) = context();
    let device = device.bound_to(&handle);
    tokio::task::yield_now().await;
    assert!(!*close_asked.borrow(), "open while the handle lives");

    hub.dropped(&partner());

    let wait = Duration::from_secs(5);
    tokio::time::timeout(wait, close_asked.wait_for(|c| *c)).await.unwrap().unwrap();
    tokio::time::timeout(wait, device.closed().wait_for(|c| *c)).await.unwrap().unwrap();
}

#[tokio::test]
async fn a_platform_channel_requested_when_the_handle_drops_fails_context_source_dropped() {
    let (device, phone, _) = offline_device().await;
    let (hub, handle) = context();
    let device = device.bound_to(&handle);
    let mut requests = phone.register(CREATE_REQUEST_TARGET);

    let trace = TraceContext::new_root(Vec::new());
    let create = device.create_platform_channel(
        PlatformChannelType::VideoStreaming,
        scenario::STREAMING,
        HashMap::new(),
        &trace,
    );
    let drop_once_requested = async {
        requests.recv().await.unwrap();
        hub.dropped(&partner());
    };

    let (created, ()) = tokio::join!(create, drop_once_requested);
    assert_eq!(created.unwrap_err(), PlatformChannelError::ContextSourceDropped(AcceptanceReason::Unspecified));
}

#[tokio::test]
async fn a_platform_channel_over_a_dropped_handle_is_not_requested() {
    let (device, phone, _) = offline_device().await;
    let (hub, handle) = context();
    hub.dropped(&partner());
    let device = device.bound_to(&handle);
    let mut requests = phone.register(CREATE_REQUEST_TARGET);

    let trace = TraceContext::new_root(Vec::new());
    let created = device
        .create_platform_channel(PlatformChannelType::VideoStreaming, scenario::STREAMING, HashMap::new(), &trace)
        .await;

    assert_eq!(created.unwrap_err(), PlatformChannelError::ContextSourceDropped(AcceptanceReason::Unspecified));
    assert!(requests.try_recv().is_err(), "nothing sent");
}
