use super::*;

#[test]
fn action_json_writes_every_field_in_order_with_nulls() {
    let body = action(Action::Pause, None, None);
    let text = String::from_utf8(body).unwrap();

    let at = |k: &str| text.find(&format!("\"{k}\":")).unwrap();
    let keys = ["scope", "correlationId", "action", "cameraError", "cameraConfigPayload"];
    assert!(keys.windows(2).all(|w| at(w[0]) < at(w[1])), "{text}");
    assert!(text.contains("\"action\":2,\"cameraError\":null,\"cameraConfigPayload\":null"));
    assert!(text.starts_with("{\"scope\":\"/streaming/action\",\"correlationId\":\""));
}

#[test]
fn face_update_matches_the_constructed_example() {
    let body = action(Action::Update, None, Some(face_config(CameraFace::Back)));
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        v["cameraConfigPayload"],
        serde_json::json!({"cameraFace":1,"oemExtensionEnabled":false,"supportedDefaultExtensions":0,
                "selectedDefaultExtension":0,"selectedOemExtension":null,"supportedOemExtensions":null})
    );

    let text = String::from_utf8(network(true)).unwrap();
    assert!(text.ends_with("\"isLowQualityNetwork\":true}"));
}

#[test]
fn inbound_dispatch_by_channel_scope() {
    let start = br#"{"scope":"x","correlationId":"c","action":1,"cameraError":null,
            "cameraConfigPayload":{"cameraFace":0,"oemExtensionEnabled":false,"supportedDefaultExtensions":3,
            "selectedDefaultExtension":0,"selectedOemExtension":null,"supportedOemExtensions":null}}"#;
    match inbound(scope::ACTION, start).unwrap() {
        Inbound::Action(a) => {
            assert_eq!(a.action, 1);
            assert_eq!(a.camera_config_payload.unwrap().supported_default_extensions, 3);
        }
        other => panic!("{other:?}"),
    }

    assert_eq!(inbound(scope::ACTION, b""), Err(InboundError::Empty));
    assert!(matches!(inbound("/streaming/other", b"{}"), Err(InboundError::UnknownScope(_))));
    assert!(matches!(inbound(scope::BATTERY, b"not json"), Err(InboundError::Parse(_))));

    assert_eq!(
        inbound(scope::BATTERY, br#"{"isCharging":true,"batteryLevel":40}"#).unwrap(),
        Inbound::Battery(BatteryMessage { is_charging: true, battery_level: 40, ..Default::default() })
    );
}

#[test]
fn publications_and_replies() {
    let p = StreamingConfigurationMessage {
        is_capable: 1,
        permissions_granted: 0,
        core_permissions_granted: 1,
        permissions_permanently_denied: false,
    };
    assert_eq!(
        camera_publication(&p.encode_to_vec()).unwrap(),
        CameraPublication { capable: true, camera_permission: false, notification_permission: true }
    );

    assert!(auto_prompt_permanently_denied(&[0x20, 0x01]));
    assert!(!auto_prompt_permanently_denied(b""));
    assert!(!auto_prompt_permanently_denied(b"CannotBeEmpty"));

    let accepted = |core: Vec<i32>, feature: Vec<i32>, qr: Option<&str>| {
        PermissionsResponseMessage {
            request_status: PermissionsRequestStatus::Accepted as i32,
            core_permissions: core,
            feature_permissions: feature,
            qr_code_uri: qr.map(str::to_owned),
            ..Default::default()
        }
        .encode_to_vec()
    };

    assert_eq!(permission_publication(&accepted(vec![1], vec![1], None)).unwrap(), Grant::Camera);
    assert_eq!(permission_publication(&accepted(vec![1], vec![1], Some("q"))).unwrap(), Grant::Core);
    assert_eq!(permission_publication(&accepted(vec![], vec![2], None)).unwrap(), Grant::Nothing);

    let reply = permission_reply(&accepted(vec![], vec![], Some("https://qr"))).unwrap();
    assert_eq!((reply.notifications, reply.camera, reply.qr_code_uri.as_deref()), (false, false, Some("https://qr")));
    assert_eq!(permission_reply(b"\x08\x03"), None, "another request status");

    assert_eq!(permission_request(), vec![0x0a, 0x01, 0x01]);
}

#[test]
fn errors_round_trip() {
    for v in [0, 1, 10, 18, 20, 24] {
        assert_eq!(CameraError::from_wire(v).wire(), v);
    }

    assert_eq!(CameraError::from_wire(17), CameraError::Other(17));
    assert!(CameraError::CameraPermissionDenied.is_permission());
    assert!(!CameraError::CameraPermissionRevoked.is_permission());
}
