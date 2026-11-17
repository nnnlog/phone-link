use super::*;

/// The phone's effect names follow the system's locale while the app shows the system's language.
#[test]
fn effect_names_follow_the_language_the_app_shows() {
    assert_eq!(effect_name_locale(Some("ko_KR.UTF-8")), "ko-KR");
    assert_eq!(effect_name_locale(Some("ko:en_US")), "ko");
    assert_eq!(effect_name_locale(Some("en_GB.UTF-8")), "en-GB");
    assert_eq!(effect_name_locale(Some("en_US.UTF-8@euro")), "en-US");
    // The app has no German: it shows English, and the names follow its localization.
    assert_eq!(effect_name_locale(Some("de_DE.UTF-8")), "en");
    assert_eq!(effect_name_locale(Some("C")), "en");
    assert_eq!(effect_name_locale(None), "en");
}

#[test]
fn effects_are_labeled_for_the_user() {
    assert_eq!(effect_label(&PhoneCameraEffect::None), "No Effect");
    assert_eq!(effect_label(&PhoneCameraEffect::Bokeh), "Blur");
    let known = PhoneCameraEffect::Oem { label: OemEffectLabel::Known { label: EffectLabel::AutoFraming }, icon: None };
    assert_eq!(effect_label(&known), "Auto-Framing");
    let text = PhoneCameraEffect::Oem { label: OemEffectLabel::Text { text: "Portrait".to_owned() }, icon: None };
    assert_eq!(effect_label(&text), "Portrait", "the phone's own name as it is");
}

#[test]
fn the_device_or_why_there_is_none() {
    assert_eq!(device_json(Ok("/dev/video10".into())), json!({ "path": "/dev/video10", "problem": null }));
    assert_eq!(device_json(Err(Unavailable::NoLoopback))["problem"], "noLoopback");
    assert_eq!(device_json(Err(Unavailable::Unlabeled))["problem"], "unlabeled");
}
