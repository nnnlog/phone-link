use super::*;

fn samsung() -> PhoneValues {
    PhoneValues {
        mirror_capability: true,
        remoting: 0x1EFFF,
        client_model: 1,
        client_revision: 1,
        input_injector: true,
        contract_version: 3.99,
        os_major: Some(14),
    }
}

#[test]
fn gates_of_a_samsung_phone_and_of_a_base_mode_phone() {
    let p = samsung();
    assert!(p.remoting_supported() && p.hosted_apps() && p.phone_screen_apps());
    assert_eq!(p.apps_feature_type(), AppsFeatureType::MultipleApps);
    assert!(p.ice_support() && p.audio() && p.notification_fallback() && p.adaptive_layout() && p.nearby());

    let old_android = PhoneValues { os_major: Some(10), ..p };
    assert_eq!(old_android.apps_feature_type(), AppsFeatureType::PhoneScreenOnly);

    let base = PhoneValues {
        remoting: 0x8 | 0x20 | 0x80 | 0x100 | 0x200,
        client_model: 0,
        client_revision: 0,
        input_injector: false,
        ..p
    };
    assert!(!base.remoting_supported() && !base.accepts_connection());
    assert_eq!(base.apps_feature_type(), AppsFeatureType::Disabled);

    let ms1 = PhoneValues { client_model: 2, client_revision: 1, remoting: 0, ..p };
    assert!(!ms1.phone_screen_apps());
    assert!(PhoneValues { client_revision: 2, ..ms1 }.phone_screen_apps());

    assert!(!PhoneValues { contract_version: 3.98, ..p }.ice_support());
}
