use super::*;

fn response(caps: &[ProtoPlatformCapabilities], nano: i32) -> Vec<u8> {
    ProtoPlatformSessionValidationResponse {
        platform_capabilities: caps.iter().map(|c| *c as i32).collect(),
        persistent_messaging_channel_version: 14,
        nano_transport_preference_version: nano,
    }
    .encode_to_vec()
}

#[test]
fn request_bytes() {
    let mut config = EngineConfig::default();
    assert_eq!(request_message(&config).payload, vec![0x0A, 0x01, 0x01]);

    config.platform.enable_nano_transport_preference = true;
    assert_eq!(request_message(&config).payload, vec![0x0A, 0x02, 0x01, 0x03]);
}

#[test]
fn acceptance_rules() {
    use ProtoPlatformCapabilities::*;
    let mut config = EngineConfig::default();
    let all = [SessionValidation, PersistentMessageChannel, NanoTransportPreference];

    assert_eq!(
        accept_response(&config, &response(&all, 3)),
        ValidationOutcome::Validated(AcceptedCapabilities {
            capabilities: vec![SessionValidation],
            nano_transport_preference_version: None
        })
    );
    assert_eq!(accept_response(&config, &response(&[PersistentMessageChannel], 3)), ValidationOutcome::NotSupported);
    assert_eq!(accept_response(&config, &[]), ValidationOutcome::NotSupported);
    assert_eq!(accept_response(&config, &[0xFF]), ValidationOutcome::NotSupported);

    // The nano version is not read while the preference is off.
    assert!(matches!(accept_response(&config, &response(&[SessionValidation], 0)), ValidationOutcome::Validated(_)));

    config.platform.enable_nano_transport_preference = true;
    assert_eq!(
        accept_response(&config, &response(&all, 3)),
        ValidationOutcome::Validated(AcceptedCapabilities {
            capabilities: vec![SessionValidation, NanoTransportPreference],
            nano_transport_preference_version: Some(3)
        })
    );
    assert_eq!(accept_response(&config, &response(&all, 2)), ValidationOutcome::NanoTransportPreferenceVersionTooLow);
    assert_eq!(accept_response(&config, &response(&[SessionValidation], 5)), ValidationOutcome::NotSupported);
}

#[test]
fn failure_mapping() {
    use phonelink_core::types::DcgSendStatus;
    let config = EngineConfig::default();
    let status = |s: PlatformRequestStatus| map_response(&config, PlatformResponse { status: s, message: None });

    assert_eq!(status(PlatformRequestStatus::NoResponse), ValidationOutcome::NotSupported);
    assert_eq!(status(PlatformRequestStatus::RemoteVersionTooLow), ValidationOutcome::NotSupported);
    assert_eq!(status(PlatformRequestStatus::RemoteNotSubscribedToRoute), ValidationOutcome::NotSupported);
    assert_eq!(status(PlatformRequestStatus::Success), ValidationOutcome::NotSupported, "null payload");

    for same in [
        PlatformRequestStatus::RemoteAllSyncDisabled,
        PlatformRequestStatus::RemoteMeteredSyncDisabled,
        PlatformRequestStatus::Canceled,
        PlatformRequestStatus::ResponseTimedOut,
        PlatformRequestStatus::RequestSendFailed(DcgSendStatus::PartnerNotConnected),
        PlatformRequestStatus::RequestSendFailed(DcgSendStatus::LostTrustWithPartner),
    ] {
        assert_eq!(status(same.clone()), ValidationOutcome::Failed(same));
    }
}
