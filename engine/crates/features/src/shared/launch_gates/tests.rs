use super::*;
use phonelink_app::consts::platform;
use phonelink_core::types::{DcgClient, Environment};

fn record() -> PhoneRecord {
    let mut r = PhoneRecord::new(&DcgClient::new("0f8fad5b-d9cb-469f-a165-70867728950e", Environment::Prod));
    r.platform = platform::ANDROID;
    r.capability_bits = capability::ANDROID_PHONE_DEFAULT;
    r
}

#[test]
fn recent_apps_need_all_three_flags() {
    let mut r = record();
    assert!(!recent_apps_supported(&r), "no remoting capabilities reported: 0");

    r.set_remoting_flags(0x1 | 0x2);
    assert!(!recent_apps_supported(&r));

    r.set_remoting_flags(0x1 | 0x2 | 0x800);
    assert!(recent_apps_supported(&r));
    assert!(!app_handoff_enabled(&r));

    r.set_remoting_flags(0x1EFFF);
    assert!(recent_apps_supported(&r) && app_handoff_enabled(&r));
}

#[test]
fn phone_screen_apps_follow_the_client_model_and_contract() {
    let app = AppConfig::default();
    let mut r = record();

    assert!(!phone_screen_apps(&r, &app), "model 0");

    r.client_model_name = Some(client_model::SG);
    assert!(phone_screen_apps(&r, &app));

    r.reported_contract_version = Some(3.90);
    assert!(!phone_screen_apps(&r, &app));
    r.reported_contract_version = Some(3.91);
    assert!(phone_screen_apps(&r, &app));

    r.client_model_name = Some(client_model::MS1);
    r.client_model_revision = Some(1);
    assert!(!phone_screen_apps(&r, &app));
    r.client_model_revision = Some(2);
    assert!(phone_screen_apps(&r, &app));

    r.client_model_name = Some(client_model::GE);
    r.client_model_revision = Some(0);
    assert!(phone_screen_apps(&r, &app));
}

#[test]
fn remoting_needs_mirror_and_an_oem_integration_or_the_injector() {
    let app = AppConfig::default();
    let mut r = record();

    assert!(!remoting_supported(&r));

    r.input_injector_available = Some(true);
    assert!(remoting_supported(&r));
    assert!(!apps_view_visible(&r, &app), "no phone-screen apps on model 0");

    r.input_injector_available = None;
    r.client_model_revision = Some(1);
    assert!(remoting_supported(&r));

    r.client_model_name = Some(client_model::SG);
    assert!(apps_view_visible(&r, &app));

    r.capability_bits &= !capability::MIRROR;
    assert!(!remoting_supported(&r) && !apps_view_visible(&r, &app));
}

#[test]
fn hosted_apps_need_the_flag_the_contract_and_android_11() {
    let app = AppConfig::default();
    let mut r = record();
    r.os_version = Some("14".into());

    assert!(!hosted_apps(&r, &app, false), "no AppsOnWindowsSupported");

    r.set_remoting_flags(remoting::APPS_ON_WINDOWS_SUPPORTED);
    assert!(hosted_apps(&r, &app, false));
    assert!(!hosted_apps(&r, &app, true), "one window per app turned off");

    r.reported_contract_version = Some(3.93);
    assert!(!hosted_apps(&r, &app, false));

    r.reported_contract_version = Some(3.94);
    r.os_version = Some("10".into());
    assert!(!hosted_apps(&r, &app, false));

    r.os_version = None;
    assert!(!hosted_apps(&r, &app, false), "no OS version");
}
