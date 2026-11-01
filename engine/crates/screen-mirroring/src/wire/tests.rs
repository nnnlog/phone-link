use super::*;

#[test]
fn workflow_bodies_are_byte_exact() {
    let id = "5f0c0a9e-1b2c-4d3e-8f90-a1b2c3d4e5f6";
    let expected = format!("{{\r\n  \"workflow\": \"remote\",\r\n  \"workflowId\": \"{id}\"\r\n}}");
    assert_eq!(workflow_body("remote", id), expected.into_bytes());
    assert_eq!(parse_stop_workflow(&workflow_body("audio", "x")).unwrap(), ("audio".into(), Some("x".into())));
    assert_eq!(parse_stop_workflow(br#"{"workflow":"remote"}"#).unwrap(), ("remote".into(), None));
    assert!(parse_stop_workflow(br#"{"workflowId":"x"}"#).is_err());
}

#[test]
fn per_app_bodies_as_the_peer_writes_them() {
    assert_eq!(
        app_focusable_state(FocusableState::InFocus, "c-1"),
        br#"{"Scope":"/message/windowFocusableState","CorrelationId":"c-1","focusableState":"inFocus"}"#.to_vec()
    );
    assert!(app_focusable_state(FocusableState::Suspended, "c").ends_with(br#""focusableState":"suspended"}"#));

    let item = ClipItem {
        mime_type: "image/png".into(),
        filename: "café <1>.png".into(),
        file_size: 12,
        data_id: "d-1".into(),
    };

    // `\uXXXX`, built so the backslash is plain.
    let u = |hex: &str| format!("{}u{hex}", '\\');
    let filename = format!("caf{} {}1{}.png", u("00E9"), u("003C"), u("003E"));
    assert_eq!(
        String::from_utf8(drag_start_pc("c-2", &[item])).unwrap(),
        format!(
            r#"{{"Scope":"/drag/startPc","correlationId":"c-2","clipItems":[{{"mimeType":"image/png","filename":"{filename}","fileSize":12,"dataId":"d-1"}}]}}"#
        )
    );

    assert_eq!(drag_drop_pc("c"), br#"{"Scope":"/drag/dropPc","correlationId":"c"}"#);
    assert_eq!(drag_cancel("c", 5), br#"{"Scope":"/drag/cancel","correlationId":"c","reason":5}"#);
    assert_eq!(drag_request_ownership("c"), br#"{"Scope":"/drag/requestOwnership","correlationId":"c"}"#);
    assert_eq!(drag_request_item("i"), br#"{"itemId":"i"}"#);

    // Inbound: the strict typing of the drag bodies.
    assert_eq!(parse_drag_cancel(br#"{"correlationId":"c","reason":11}"#), Ok(11));
    assert_eq!(parse_drag_cancel(br#"{"correlationId":null}"#), Ok(0));
    assert!(parse_drag_cancel(br#"{"correlationId":"c","reason":"11"}"#).is_err());
    assert!(parse_drag_cancel(br#"{"reason":5}"#).is_err());
    assert_eq!(parse_drag_complete(br#"{"correlationId":"c","dropTarget":null}"#), Ok(None));
    assert!(parse_drag_complete(br#"{"correlationId":"c","droppedItemCount":1.5}"#).is_err());
    assert!(parse_drag_start_phone(br#"{"correlationId":"c","mimeTypes":null,"sessionId":""}"#).is_ok());
    assert!(parse_drag_start_phone(br#"{"correlationId":"c","mimeTypes":[1]}"#).is_err());
    assert_eq!(parse_clip_data(br#"{"correlationId":"c","clipDataItems":null}"#), Ok(None));
    assert!(
        parse_clip_data(
            br#"{"correlationId":"c","clipDataItems":{"dataIds":[],"mimeTypes":[],"fileSizes":[2.5],"filenames":[]}}"#
        )
        .is_err()
    );
    assert!(parse_drag_request_item(br#"{"itemId":3}"#).is_err());

    assert_eq!(disconnecting_result("t"), br#"{"transactionId":"t"}"#);
    assert_eq!(request_move_to_vd_result(true, "t"), br#"{"result":true,"transactionId":"t"}"#);
    assert_eq!(request_screen_layout(2, "t", None), br#"{"targetScreenLayout":2,"transactionId":"t"}"#);

    // An astral character is two UTF-16 escapes; `"` and `+` are escaped too.
    let mut s = String::new();
    write_string(&mut s, "a\"+\u{1F600}", Escaping::Strict);
    assert_eq!(s, format!("\"a{}{}{}{}\"", u("0022"), u("002B"), u("D83D"), u("DE00")));
}

#[test]
fn orchestrator_and_permission_bodies() {
    let c = ConfigValues {
        max_main_display_fps: 0,
        apps_feature_type: 2,
        width: 2880,
        height: 1700,
        dpi: 192,
        host_display_width: 2880,
        host_display_height: 1800,
    };

    assert_eq!(
            config(&c),
            br#"{"maxMainDisplayFps":0,"appsFeatureType":2,"width":2880,"height":1700,"dpi":192,"hostDisplayWidth":2880,"hostDisplayHeight":1800}"#
        );
    assert_eq!(
        launch_app(PHONE_SCREEN_PACKAGE, "s", 0, None, 3),
        br#"{"packageName":"yourphone.phonescreen","sessionId":"s","targetScreenLayout":0,"flags":3}"#
    );
    assert_eq!(
        launch_app("p", "s", 2, Some(77), 4),
        br#"{"packageName":"p","sessionId":"s","targetScreenLayout":2,"transferTaskId":77,"flags":4}"#
    );
    assert_eq!(request_permission(), br#"{"permission":"screenScrape"}"#);
    assert_eq!(permission_transaction("oemAudio", "t"), br#"{"permission":"oemAudio","transactionId":"t"}"#);
    assert_eq!(
        request_session_result("s", "t", true, 0),
        br#"{"sessionId":"s","transactionId":"t","sessionAlreadyExists":true,"targetScreenLayout":0}"#
    );
    assert_eq!(launch_notification_v2("s", "k", -1), br#"{"transactionId":"s","key":"k","actionIndex":-1}"#);
    assert_eq!(launch_notification_v1("k", 2), br#"{"key":"k","actionIndex":2}"#);
    assert_eq!(launch_custom_intent(KEYBOARD_SETTINGS_INTENT), br#"{"intentKey":"hardware_keyboard_settings"}"#);
    assert_eq!(
        launch_phone_screen_app(PHONE_SCREEN_PACKAGE, "g", 3),
        br#"{"packageName":"yourphone.phonescreen","sessionId":"g","launchType":3}"#
    );
    assert_eq!(black_screen(true), br#"{"blackScreenToggleState":true}"#);
    assert_eq!(always_expanded(false), br#"{"isAlwaysInExpandLayout":false}"#);
    assert_eq!(app_resumable("t"), br#"{"transactionId":"t"}"#);
    assert!(empty_orchestrator().is_empty());
    assert_eq!(empty_nearby(), b"{}");
    assert_eq!(nearby_request_setting(true, "t"), br#"{"nearbyState":true,"transactionId":"t"}"#);
    assert_eq!(audio_set_volume("p", 50), br#"{"packageName":"p","volume":50}"#);
    assert_eq!(audio_pc_streaming_allowed(true), br#"{"isAudioStreamingAllowed":true}"#);
}

#[test]
fn inbound_parsing_defaults_and_unknowns() {
    let l = parse_locked_state(br#"{"isLocked":true,"isFolded":false,"canDraw":true}"#).unwrap();
    assert!(l.is_locked && l.is_screen_on && !l.is_secure_locked && !l.is_always_unlocked_display);
    assert_eq!(parse_locked_state(br#"{"isLocked":true}"#), Err(ParseError::Missing("isFolded")));
    assert_eq!(parse_locked_state(b"not json"), Err(ParseError::NotAnObject));

    assert_eq!(
        parse_disconnecting(br#"{"reason":"maxapps","transactionId":"t"}"#).unwrap().0,
        DisconnectingReason::MaxApps
    );
    assert_eq!(parse_disconnecting(br#"{"reason":"Sideways"}"#).unwrap().0, DisconnectingReason::Unknown);
    assert_eq!(parse_disconnecting(br#"{"reason":8}"#).unwrap().0, DisconnectingReason::Heat);
    assert_eq!(parse_disconnecting(br#"{"reason":"Heat"}"#).unwrap().1, "", "no transactionId: acknowledged with \"\"");
    assert_eq!(parse_disconnecting(br#"{"reason":"Heat","transactionId":7}"#).unwrap().1, "7");

    let r = parse_notification_result(br#"{"launchResult":1.0,"key":"k"}"#).unwrap();
    assert_eq!((r.launch_result, r.key.as_deref(), r.transaction_id), (1, Some("k"), None));
}
