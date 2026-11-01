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
