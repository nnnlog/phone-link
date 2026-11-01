use super::*;

#[test]
fn placeholders_by_state_and_error() {
    use CameraPlaceholder as P;
    let p = |s, e| placeholder_for(s, e, true);
    assert_eq!(p(CameraState::Streaming, CameraError::None), None);
    assert_eq!(p(CameraState::Registered, CameraError::None), Some(P::Black));
    assert_eq!(p(CameraState::Connecting, CameraError::None), Some(P::Connecting));
    assert_eq!(p(CameraState::TerminalError, CameraError::CallInProgress), Some(P::Paused));
    assert_eq!(p(CameraState::TerminalError, CameraError::ScreenLocked), Some(P::PhoneLocked));
    assert_eq!(p(CameraState::Error, CameraError::LtwDisabled), Some(P::Disconnected));
    assert_eq!(p(CameraState::Error, CameraError::CameraPermissionDenied), Some(P::StartStreamNotification));
    assert_eq!(placeholder_for(CameraState::Error, CameraError::CameraPermissionNeeded, false), Some(P::PhoneLocked));
    assert_eq!(p(CameraState::Error, CameraError::CameraPermissionRevoked), Some(P::Error));
    assert_eq!(p(CameraState::StartStreamNotification, CameraError::None), Some(P::StartStreamNotification));
}

#[test]
fn network_quality_from_metrics() {
    let m = |loss: &str, received: &str| {
        format!(
            r#"{{"rttavg":"0","bandwidthavg":"0","packetlossrate":"{loss}","packetsreceivedintimewindow":"{received}","metricsVersion":"1"}}"#
        )
    };
    assert_eq!(low_network(&m("0.5", "1"), 0.5), Some(true));
    assert_eq!(low_network(&m("0.499", "1"), 0.5), Some(false));
    assert_eq!(low_network(&m("0", "0"), 0.5), Some(true));
    assert_eq!(low_network(&m("nan", "1"), 0.5), Some(false));
    assert_eq!(low_network("{}", 0.5), None);
}

#[test]
fn error_actions() {
    assert_eq!(ErrorAction::for_error(CameraError::ChannelDisconnected), ErrorAction::Refresh);
    assert_eq!(ErrorAction::for_error(CameraError::PostNotificationPermissionNeeded), ErrorAction::Refresh);
    assert_eq!(ErrorAction::for_error(CameraError::CameraPermissionNeededAutoprompt), ErrorAction::RequestPermissions);
    assert_eq!(ErrorAction::for_error(CameraError::CameraServiceFatal), ErrorAction::Close);
    assert_eq!(ErrorAction::for_error(CameraError::DeviceHeat), ErrorAction::None);
}

fn oem(resource: &str, fallbacks: &[(&str, &str)]) -> Effect {
    Effect::Oem(OemExtension {
        id: 3,
        name: wire::ExtensionName {
            resource: resource.into(),
            fallbacks: fallbacks
                .iter()
                .map(|(locale, value)| wire::Fallback { locale: (*locale).into(), value: (*value).into() })
                .collect(),
        },
        icon: None,
    })
}

#[test]
fn oem_labels_take_the_ui_string_for_their_resource_first() {
    let texts = [("en-US", "Portrait")];
    assert_eq!(
        oem("camera_effect_auto_framing", &texts).oem_label("en-US"),
        Some(OemLabel::Known(EffectLabel::AutoFraming))
    );
    assert_eq!(oem("camera_effect_soft_focus", &[]).oem_label("en-US"), Some(OemLabel::Known(EffectLabel::SoftFocus)));
    assert_eq!(
        oem("camera_effect_stabilization", &[]).oem_label(""),
        Some(OemLabel::Known(EffectLabel::Stabilization))
    );
    assert_eq!(oem("camera_effect_no_filter", &[]).oem_label("en-US"), Some(OemLabel::Known(EffectLabel::NoFilter)));
    assert_eq!(Effect::None.oem_label("en-US"), None);
}

#[test]
fn oem_labels_fall_back_to_the_locale_text_then_the_resource() {
    let texts = [("ko-KR", "인물"), ("en-US", "Portrait")];
    let text = |s: &str| Some(OemLabel::Text(s.into()));
    assert_eq!(oem("portrait", &texts).oem_label("en-US"), text("Portrait"));
    assert_eq!(oem("portrait", &texts).oem_label(""), text("인물"));
    assert_eq!(oem("portrait", &texts).oem_label("en"), text("portrait"));
    assert_eq!(oem("portrait", &[]).oem_label(""), text("portrait"));
    assert_eq!(oem("", &[]).oem_label("en-US"), text(""));

    // An empty part makes no key.
    assert_eq!(oem("camera__effect_blur", &texts).oem_label("en-US"), text("Portrait"));
    assert_eq!(oem("camera_effect_blur_", &[]).oem_label("en-US"), text("camera_effect_blur_"));
}
