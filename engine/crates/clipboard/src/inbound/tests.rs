use super::*;
use phonelink_core::types::{DeviceInfoItem, Environment};

#[test]
fn the_pull_forces_a_session_start_and_only_shutdown_cancels_it() {
    let peer = PeerDevice {
        client: DcgClient::new("phone-1", Environment::Prod),
        info: DeviceInfoItem { id: "phone-1".into(), client_version: Some("1.24072.1.0".into()), ..Default::default() },
    };
    let shutdown = CancellationToken::new();
    let request = content_pull(&peer, "cid-1", true, Duration::from_secs(120), shutdown.child_token());

    assert!(request.force_session_start);
    assert!(request.phone_enabled);
    assert_eq!(request.address, DrmAddress::Path("/clipboard".into()));
    assert_eq!(request.request_type, ProtoDeviceResourceRequestType::Get);
    assert_eq!(request.payload, wire::content_request("cid-1"));
    assert_eq!(request.client_version.as_deref(), Some("1.24072.1.0"));
    assert_eq!(request.timeout, Duration::from_secs(120));
    assert!(!request.cancel.is_cancelled(), "no cancellation of its own");

    shutdown.cancel();
    assert!(request.cancel.is_cancelled(), "the feature's shutdown abandons it");

    let without_id = content_pull(&peer, "", false, Duration::from_secs(1), CancellationToken::new());
    assert_eq!(without_id.payload, vec![0x08, 0x01]);
}
