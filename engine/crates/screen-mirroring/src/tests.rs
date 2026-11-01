//! The phone and window state machines.

use super::machine::{
    Details, PhoneCommand as P, PhoneState, SessionCommand as C, SessionMachine, WindowState as W, phone_next,
};

#[test]
fn phone_transitions_follow_the_table() {
    assert_eq!(phone_next(PhoneState::Disconnected, P::EnsureConnection), Some(PhoneState::Connecting));
    assert_eq!(phone_next(PhoneState::Connecting, P::OnConnected), Some(PhoneState::Connected));
    assert_eq!(phone_next(PhoneState::Connected, P::OnConnectionUnstable), Some(PhoneState::ConnectionUnstable));
    assert_eq!(phone_next(PhoneState::ConnectionUnstable, P::OnConnectionRecovered), Some(PhoneState::Connected));
    assert_eq!(phone_next(PhoneState::ConnectionUnstable, P::StateMachineTimeout), Some(PhoneState::Error));
    assert_eq!(phone_next(PhoneState::Connecting, P::StateMachineTimeout), Some(PhoneState::Error));
    assert_eq!(phone_next(PhoneState::SecureLocked, P::OnLocked), Some(PhoneState::SecureLocked));
    assert_eq!(phone_next(PhoneState::SecureLocked, P::NearbyConnectionCompleted), Some(PhoneState::Disconnected));
    assert_eq!(phone_next(PhoneState::Error, P::RetryConnection), Some(PhoneState::Disconnected));
    assert_eq!(phone_next(PhoneState::Connected, P::EnsureConnection), None, "not in the table");
    assert_eq!(phone_next(PhoneState::Disconnected, P::OnConnected), None);
}

fn connected() -> SessionMachine {
    let mut m = SessionMachine::new(false);
    m.apply(C::AppLaunchSuccess, None);
    m.apply(C::PeerConnectionVideoReady, None);
    assert_eq!(m.state, W::Connected);
    m
}

#[test]
fn connected_needs_the_launch_and_the_first_frame() {
    let mut m = SessionMachine::new(false);
    assert!(!m.apply(C::PhoneStateConnected, None), "Connected without launch or video stays Loading");
    assert_eq!(m.state, W::Loading);

    m.apply(C::PeerConnectionVideoReady, None);
    assert_eq!(m.state, W::Loading, "video ready but not launched");

    assert!(m.apply(C::AppLaunchSuccess, None));
    assert_eq!(m.state, W::Connected);
}

#[test]
fn lock_states_and_their_rewrites() {
    let mut m = connected();
    m.apply(C::PhoneStateLocked, Some(Details::Locked { secure: true, virtual_device_streaming: false }));
    assert_eq!((m.state, &m.details), (W::Locked, &Details::Locked { secure: true, virtual_device_streaming: false }));

    // Restart while locked stays Locked; the Locked details are kept.
    m.apply(C::MoveToVirtualDisplay, Some(Details::Restart { task_transfer: true }));
    assert_eq!(m.state, W::Locked);

    m.apply(C::PhoneStateUnlocked, None);
    assert_eq!(m.state, W::Restart, "the pending move to the Mac shows once unlocked");
    assert_eq!(m.details, Details::Restart { task_transfer: true }, "the last restart details");

    m.apply(C::AppLaunchSuccess, None);
    assert_eq!(m.state, W::Connected);

    m.apply(C::PhoneStateScreenOff, None);
    assert_eq!(m.state, W::ScreenOff);
}

#[test]
fn true_touch_warning_and_do_not_show() {
    let mut m = SessionMachine::new(false);
    m.apply(C::TrueTouchWarning, None);
    assert_eq!(m.state, W::TrueTouchWarning);
    let mut m = SessionMachine::new(true);
    m.apply(C::TrueTouchWarning, None);
    assert_eq!(m.state, W::Loading, "\"do not show\" skips the warning");
}

#[test]
fn errors_keep_their_code_and_closures_need_a_relaunch() {
    let mut m = connected();
    m.apply(C::PhoneStateError, Some(Details::Error { code: 81 }));
    assert_eq!((m.state, &m.details), (W::Error, &Details::Error { code: 81 }));
    assert!(!m.launched() && !m.video_ready(), "an error clears the launch");

    m.apply(C::StateMachineTimeout, None);
    assert_eq!(m.details, Details::Error { code: 81 }, "an error without its own code keeps the last");

    m.apply(C::RelaunchAll, None);
    assert_eq!(m.state, W::Loading);

    m.apply(C::StateMachineTimeout, Some(Details::Error { code: 37 }));
    assert_eq!((m.state, &m.details), (W::Error, &Details::Error { code: 37 }));

    let mut m = connected();
    m.apply(C::PeerConnectionClosedSuspended, Some(Details::Closed { reason: None }));
    assert_eq!(m.state, W::Suspended);

    m.apply(C::PhoneStateConnected, None);
    assert_eq!(m.state, W::Closed, "Suspended + phone connected → Closed");

    m.apply(C::RelaunchApp, None);
    assert_eq!(m.state, W::Loading);

    let mut m = connected();
    m.apply(C::PhoneScreenAppLaunchError, Some(Details::Error { code: 34 }));
    assert_eq!(m.state, W::DismissibleError);

    m.apply(C::DismissError, None);
    assert_eq!(m.state, W::Connected, "the phone screen shows again");
}

#[test]
fn unstable_connection_restores_the_audio_prompt() {
    let mut m = connected();
    m.apply(C::AudioStreamingPermissionNeeded, None);
    assert_eq!(m.state, W::AudioStreamingPermissionNeeded);

    m.apply(C::PhoneStateConnectionUnstable, None);
    assert_eq!(m.state, W::ConnectionUnstable);

    m.apply(C::PhoneStateConnected, None);
    assert_eq!(m.state, W::AudioStreamingPermissionNeeded);
}

#[test]
fn layout_switch_confirms_then_returns_to_connected() {
    let mut m = connected();
    m.apply(C::SwitchScreenLayout, Some(Details::LayoutSwitch));
    assert_eq!((m.state, &m.details), (W::Restart, &Details::LayoutSwitch));
    m.apply(C::AppLaunchSuccess, None);
    assert_eq!(m.state, W::Connected, "no move to the Mac is pending");
}

#[test]
fn screen_off_details_are_kept_apart_from_lock_details() {
    let mut m = connected();
    let locked = Details::Locked { secure: true, virtual_device_streaming: false };
    let off = Details::Locked { secure: false, virtual_device_streaming: true };

    m.apply(C::PhoneStateLocked, Some(locked.clone()));
    m.apply(C::PhoneStateError, Some(Details::Error { code: 80 }));

    // A screen-off command the error state ignores still records its details as the screen-off ones.
    m.apply(C::PhoneStateScreenOff, Some(off.clone()));
    assert_eq!(m.state, W::Error);

    m.apply(C::RelaunchAll, None);
    m.apply(C::PhoneStateLocked, None);
    assert_eq!((m.state, &m.details), (W::Locked, &locked), "the lock details, not the screen-off ones");
}

#[test]
fn an_error_without_a_code_keeps_details_only_within_the_error_state() {
    let mut m = SessionMachine::new(false);
    m.apply(C::PhoneStateLocked, Some(Details::Locked { secure: false, virtual_device_streaming: false }));
    m.apply(C::PhoneStateError, None);
    assert_eq!((m.state, &m.details), (W::Error, &Details::None), "entering Error without a code shows none");
}

#[test]
fn staying_in_the_touch_warning_ends_the_need_for_it() {
    let mut m = SessionMachine::new(false);
    m.apply(C::TrueTouchWarning, None);
    assert_eq!(m.state, W::TrueTouchWarning);

    m.apply(C::PhoneStateUnlocked, None);
    assert_eq!(m.state, W::TrueTouchWarning);

    m.apply(C::AppLaunchSuccess, None);
    m.apply(C::PeerConnectionVideoReady, None);
    assert_eq!(m.state, W::Connected, "the warning is not shown again");
}

#[test]
fn a_preserving_relaunch_keeps_the_details() {
    let mut m = connected();
    m.apply(C::AppLaunchError, Some(Details::Error { code: 32 }));
    assert!(m.apply_preserving(C::RelaunchApp));
    assert_eq!((m.state, &m.details), (W::Loading, &Details::Error { code: 32 }));
    assert!(!m.apply_preserving(C::RelaunchApp), "Loading to Loading with the same details is no change");
}

#[test]
fn the_enable_audio_button_shows_only_while_connected_and_the_volume_button_always() {
    use super::AudioButton;
    use super::session::Session;

    let mut s = Session::new("s".into(), "com.example.app".into(), 0, true, false);
    s.audio_button = AudioButton::EnableAudio;
    assert_eq!(s.shown_audio_button(), AudioButton::None, "loading");

    s.machine = connected();
    assert_eq!(s.shown_audio_button(), AudioButton::EnableAudio);

    s.machine.apply(C::PhoneStateConnecting, None);
    assert_ne!(s.machine.state, W::Connected);
    assert_eq!(s.shown_audio_button(), AudioButton::None);

    s.audio_button = AudioButton::Volume;
    assert_eq!(s.shown_audio_button(), AudioButton::Volume, "the volume button in any state");
}
