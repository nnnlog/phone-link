use phonelink_engine::api::screen_mirroring::MirrorPhoneState;

use super::*;

fn all_off() -> MirrorSettings {
    MirrorSettings {
        allow_apps: false,
        black_screen: false,
        audio_streaming: false,
        always_expanded: false,
        do_not_ask_restart: false,
        do_not_ask_task_transfer: false,
        do_not_show_true_touch: false,
        do_not_ask_skip_unlock: false,
        do_not_ask_skip_verification: false,
        do_not_ask_switch_layout: false,
    }
}

/// Every setting the pane shows round-trips through its name.
#[test]
fn each_setting_changes_by_its_name() {
    let names = settings(&all_off());
    let names = names.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()).unwrap_or_default();
    assert_eq!(names.len(), 10);
    for name in names {
        let changed = with_setting(all_off(), &name, true).map(|s| settings(&s));
        let on: Vec<_> = changed
            .iter()
            .flat_map(|v| v.as_object().into_iter().flatten())
            .filter(|(_, v)| **v == Value::Bool(true))
            .map(|(k, _)| k.clone())
            .collect();
        assert_eq!(on, std::slice::from_ref(&name), "only {name} turned on");
    }
    assert!(with_setting(all_off(), "volume", true).is_err());
}

fn status_of(apps: MirrorAppsFeatureType, remoting: bool) -> MirroringStatusView {
    MirroringStatusView {
        remoting_supported: remoting,
        phone_screen_apps: true,
        apps_feature_type: apps,
        input_supported: true,
        audio_supported: true,
        adaptive_layout: false,
        phone_state: MirrorPhoneState::Connected,
        phone_error: None,
        nearby: None,
        settings: MirrorSettings { allow_apps: true, ..all_off() },
        permission_accepted: true,
    }
}

/// Mirroring is offered when the phone supports it, shows its screen and its apps open here.
#[test]
fn mirroring_is_offered_by_the_phone() {
    assert_eq!(status_json(status_of(MirrorAppsFeatureType::MultipleApps, true))["offered"], true);
    assert_eq!(status_json(status_of(MirrorAppsFeatureType::PhoneScreenOnly, true))["offered"], true);
    assert_eq!(status_json(status_of(MirrorAppsFeatureType::Disabled, true))["offered"], false);
    assert_eq!(status_json(status_of(MirrorAppsFeatureType::MultipleApps, false))["offered"], false);
    let status = status_json(status_of(MirrorAppsFeatureType::MultipleApps, true));
    assert_eq!(status["settings"]["allowApps"], true);
    assert_eq!(status["allowApps"], true);
}
