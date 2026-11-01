use super::*;

#[test]
fn a_feature_state_request_has_no_correlation_id_and_follows_the_trigger() {
    let manager = Manager {
        phone: DcgClient::new("phone-1", Environment::Prod),
        capabilities: vec!["CLIPBOARD".into()],
        client_version: Some("1.24072.1.0".into()),
        available: true,
        enabled: true,
    };
    assert!(manager.capable());

    let config = ClipboardConfig::default();
    let shutdown = CancellationToken::new();
    let forced =
        feature_state_send(&manager, ClipboardRequestType::FeatureOn, true, true, &config, shutdown.child_token());

    assert!(forced.force_session_start);
    assert_eq!(forced.payload, vec![0x08, 0x02]);
    assert_eq!(forced.address, DrmAddress::Path("/clipboard".into()));
    assert_eq!(forced.request_type, ProtoDeviceResourceRequestType::Get);
    assert_eq!(forced.client_version.as_deref(), Some("1.24072.1.0"));
    assert_eq!(forced.timeout, config.request_timeout);
    assert!(!forced.cancel.is_cancelled(), "no cancellation of its own");

    shutdown.cancel();
    assert!(forced.cancel.is_cancelled(), "the feature's shutdown abandons it");

    let unforced = feature_state_send(
        &manager,
        ClipboardRequestType::FeatureDisable,
        false,
        false,
        &config,
        CancellationToken::new(),
    );
    assert!(!unforced.force_session_start);
    assert!(!unforced.phone_enabled);
    assert_eq!(unforced.payload, vec![0x08, 0x04]);

    let without_list = Manager { capabilities: Vec::new(), ..manager.clone() };
    assert!(!without_list.capable());

    assert_eq!(key(&DcgClient::new("PHONE-1", Environment::Prod)), key(&manager.phone));
    assert_ne!(key(&DcgClient::new("phone-1", Environment::Beta)), key(&manager.phone));
}
