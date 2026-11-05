use super::*;

#[test]
fn capability_list() {
    let base = vec!["DEVICE_STATUS".to_owned(), "UNLINKABLE_V1".to_owned()];
    let on = RemoteControlConfig::default();
    assert_eq!(
        with_remote_control_capabilities(&base, &on),
        ["DEVICE_STATUS", "UNLINKABLE_V1", "REMOTE_CONTROL", "REMOTE_CONTROL_V2"]
    );

    let v1 = RemoteControlConfig { enabled: true, v2_enabled: false };
    assert_eq!(with_remote_control_capabilities(&base, &v1), ["DEVICE_STATUS", "UNLINKABLE_V1", "REMOTE_CONTROL"]);

    let off = RemoteControlConfig { enabled: false, v2_enabled: true };
    assert_eq!(with_remote_control_capabilities(&base, &off), base);

    let with = with_remote_control_capabilities(&base, &on);
    assert_eq!(with_remote_control_capabilities(&with, &on), with, "never twice");
    assert_eq!(with_remote_control_capabilities(&with, &off), base);
}

#[test]
fn the_settings_line_names_the_v2_commands_only_while_the_phone_and_the_mac_advertise_them() {
    const V1: &[&str] = &["REMOTE_CONTROL"];
    const V2: &[&str] = &["REMOTE_CONTROL", "REMOTE_CONTROL_V2"];
    const NEITHER: &[&str] = &["DEVICE_STATUS"];
    const WRONG_CASE: &[&str] = &["remote_control", "remote_control_v2"];
    let both = RemoteControlConfig { enabled: true, v2_enabled: true };
    let v1_only = RemoteControlConfig { enabled: true, v2_enabled: false };
    let off = RemoteControlConfig { enabled: false, v2_enabled: true };

    // (the phone's capabilities, the Mac's options) → (applicable, capable, v2_capable)
    let cases = [
        (V2, &both, (true, true, true)),
        (V2, &v1_only, (true, true, false)),
        (V1, &both, (true, true, false)),
        (V2, &off, (false, true, true)),
        (NEITHER, &both, (false, false, false)),
        (WRONG_CASE, &both, (false, false, false)),
    ];

    for (capabilities, config, expected) in cases {
        let info = DeviceInfoItem {
            capabilities: Some(capabilities.iter().map(|c| (*c).to_owned()).collect()),
            ..DeviceInfoItem::default()
        };
        let phone = RemoteControlPhone::new(&info, false, config);
        assert_eq!((phone.applicable, phone.capable, phone.v2_capable), expected, "{capabilities:?} {config:?}");
        assert!(!phone.toggle, "the toggle is reported as stored");
    }
}
