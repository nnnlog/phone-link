//! Remote control — the Cross Device contract over the platform layer: the `/remotecontrol`
//! receiver's actions and answers (a lock or a sleep answered before the Mac acts, the others after),
//! its gates (option, phone capabilities, toggle, enabled switch) and the device-list refresh before a
//! capability failure, the addresses that reach it, and what the UI reads. The Mac's actions only
//! reach a recording edge.

use crate::support::platform_world;
use crate::support::remote_control_world as rc;
use phonelink_engine::config::features::RemoteControlConfig;

use phonelink_engine::features::remote_control::{PowerAction, RemoteControlEvent, RemoteControlPhone};
use phonelink_engine::proto::remote_control::v1::{
    RemoteControlResponseMessage, RemoteControlResult, RemoteControlType,
};
use phonelink_engine::seams::PlatformRequestStatus;
use phonelink_engine::types::DcgSendStatus;
use platform_world::{Behavior, phone_id, settle};
use rc::{RcWorld, V2, item_with};

const SUCCESS: i32 = RemoteControlResult::Success as i32;
const FAILED: i32 = RemoteControlResult::Failed as i32;
const DISABLED: i32 = RemoteControlResult::Disabled as i32;

fn result(r: &RemoteControlResponseMessage) -> (i32, Option<bool>) {
    (r.control_result, r.pc_update_available)
}

#[tokio::test(start_paused = true)]
async fn every_control_type_acts_and_answers() {
    let w = RcWorld::connected().await;
    use RemoteControlType as T;
    for (t, action) in [
        (T::Lock, PowerAction::LockScreen),
        (T::Sleep, PowerAction::Sleep),
        (T::Shutdown, PowerAction::ShutDown),
        (T::Restart, PowerAction::Restart),
        // No pending-update install on macOS: the plain action.
        (T::UpdateAndShutdown, PowerAction::ShutDown),
        (T::UpdateAndRestart, PowerAction::Restart),
    ] {
        let before = w.power.actions().len();
        assert_eq!(result(&w.drm(t as i32).await), (SUCCESS, None), "{t:?}");
        settle().await;
        assert_eq!(w.power.actions()[before..], [action], "{t:?}");
    }

    // The query: SUCCESS, never an update pending, nothing done.
    let before = w.power.actions().len();
    assert_eq!(result(&w.drm(T::QueryUpdateAvailability as i32).await), (SUCCESS, Some(false)));

    // TRIGGER_TOGGLE_ON and unknown numbers: FAILED, nothing done.
    for t in [T::TriggerToggleOn as i32, 9, 1000] {
        assert_eq!(result(&w.drm(t).await), (FAILED, None), "{t}");
    }
    assert_eq!(w.power.actions().len(), before);
    assert_eq!(w.refresh.count(), 0, "a capable, enabled phone needs no refresh");
}

#[tokio::test(start_paused = true)]
async fn a_failing_action_answers_failed_unless_it_was_answered_first() {
    let w = RcWorld::connected().await;
    w.power.set_failing(true);

    // A lock or a sleep is answered SUCCESS before the Mac acts: the failure is only logged.
    for t in [RemoteControlType::Lock, RemoteControlType::Sleep] {
        assert_eq!(result(&w.drm(t as i32).await), (SUCCESS, None), "{t:?}");
    }
    for t in [RemoteControlType::Shutdown, RemoteControlType::Restart] {
        assert_eq!(result(&w.drm(t as i32).await), (FAILED, None), "{t:?}");
    }

    settle().await;
    assert_eq!(
        w.power.actions(),
        [PowerAction::LockScreen, PowerAction::Sleep, PowerAction::ShutDown, PowerAction::Restart],
        "each was tried"
    );
}

#[tokio::test(start_paused = true)]
async fn a_lock_or_a_sleep_whose_answer_was_not_sent_is_not_carried_out() {
    let w = RcWorld::connected().await;
    let lock = rc::request_bytes(RemoteControlType::Lock as i32);
    let sleep = rc::request_bytes(RemoteControlType::Sleep as i32);

    // An answer that was sent: the Mac acts.
    let response = w.sw.phone_drm_request(4, 2, "/remotecontrol", &lock).await;
    assert_eq!(response.status, PlatformRequestStatus::Success);
    settle().await;
    assert_eq!(w.power.actions(), [PowerAction::LockScreen]);

    // An answer the transport could not send: the Mac does not act.
    w.sw.w.pair.pc_link.script(Behavior::Fail(DcgSendStatus::TimedOut));
    let response = w.sw.phone_drm_request(4, 2, "/remotecontrol", &sleep).await;
    assert_eq!(response.status, PlatformRequestStatus::ResponseTimedOut, "no answer reached the phone");
    settle().await;
    assert_eq!(w.power.actions(), [PowerAction::LockScreen], "the sleep was not carried out");

    // A shutdown is answered after the Mac acted, whatever its answer's fate.
    w.sw.w.pair.pc_link.script(Behavior::Fail(DcgSendStatus::TimedOut));
    let shutdown = rc::request_bytes(RemoteControlType::Shutdown as i32);
    w.sw.phone_drm_request(4, 2, "/remotecontrol", &shutdown).await;
    settle().await;
    assert_eq!(w.power.actions(), [PowerAction::LockScreen, PowerAction::ShutDown]);
}

#[tokio::test(start_paused = true)]
async fn empty_and_undecodable_payloads_answer_failed() {
    let w = RcWorld::connected().await;
    // UNSPECIFIED encodes as zero bytes.
    assert_eq!(result(&w.drm(RemoteControlType::Unspecified as i32).await), (FAILED, None));
    assert_eq!(result(&w.drm_raw(4, 2, "/remotecontrol", &[0xFF, 0xFF, 0xFF]).await), (FAILED, None));
    assert!(w.power.actions().is_empty());
    assert_eq!(w.refresh.count(), 0);
}

#[tokio::test(start_paused = true)]
async fn the_option_off_answers_disabled_before_parsing_and_offers_no_toggle() {
    let w = RcWorld::new(item_with(&V2), RemoteControlConfig { enabled: false, v2_enabled: true });
    w.sw.w.connect_phone().await;

    assert_eq!(result(&w.drm(RemoteControlType::Lock as i32).await), (DISABLED, None));
    assert_eq!(result(&w.drm_raw(4, 2, "/remotecontrol", &[0xFF, 0xFF]).await), (DISABLED, None));

    // The empty payload is checked first.
    assert_eq!(result(&w.drm(RemoteControlType::Unspecified as i32).await), (FAILED, None));
    assert!(w.power.actions().is_empty());
    assert_eq!(
        w.rc.phone(&phone_id()).unwrap(),
        Some(RemoteControlPhone { toggle: true, applicable: false, capable: true, v2_capable: true })
    );
}

#[tokio::test(start_paused = true)]
async fn the_toggle_and_the_enabled_switch_answer_disabled_without_a_refresh() {
    let w = RcWorld::connected().await;
    w.rc.set_phone_toggle(&phone_id(), false).unwrap();
    assert_eq!(result(&w.drm(RemoteControlType::Shutdown as i32).await), (DISABLED, None));

    w.rc.set_phone_toggle(&phone_id(), true).unwrap();
    w.sw.switches.set_enabled(&phone_id(), false).unwrap();
    assert_eq!(result(&w.drm(RemoteControlType::Restart as i32).await), (DISABLED, None));
    assert!(w.power.actions().is_empty());
    assert_eq!(w.refresh.count(), 0);

    w.sw.switches.set_enabled(&phone_id(), true).unwrap();
    assert_eq!(result(&w.drm(RemoteControlType::Lock as i32).await), (SUCCESS, None));
}

#[tokio::test(start_paused = true)]
async fn a_missing_capability_refreshes_the_device_list_then_fails() {
    let w = RcWorld::new(item_with(&["REMOTE_CONTROL"]), RemoteControlConfig::default());
    w.sw.w.connect_phone().await;

    // LOCK needs only REMOTE_CONTROL.
    assert_eq!(result(&w.drm(RemoteControlType::Lock as i32).await), (SUCCESS, None));
    assert_eq!(w.refresh.count(), 0);

    // SLEEP needs REMOTE_CONTROL_V2: refresh, still missing → FAILED.
    assert_eq!(result(&w.drm(RemoteControlType::Sleep as i32).await), (FAILED, None));
    assert_eq!(w.refresh.count(), 1);

    // The toggle is not consulted before the capability: off, the answer is still FAILED.
    w.rc.set_phone_toggle(&phone_id(), false).unwrap();
    assert_eq!(result(&w.drm(RemoteControlType::Sleep as i32).await), (FAILED, None));
    assert_eq!(w.refresh.count(), 2);

    // TRIGGER_TOGGLE_ON is not a V2 type: it passes to the toggle.
    assert_eq!(result(&w.drm(RemoteControlType::TriggerToggleOn as i32).await), (DISABLED, None));
    w.rc.set_phone_toggle(&phone_id(), true).unwrap();

    // The refresh brings the capability: checked again, then acted on.
    let peers = w.sw.peers.clone();
    w.refresh.on_refresh(move || {
        peers.upsert(phonelink_engine::auth::PeerDevice { client: phone_id(), info: item_with(&V2) })
    });
    assert_eq!(result(&w.drm(RemoteControlType::Sleep as i32).await), (SUCCESS, None));
    assert_eq!(w.refresh.count(), 3);
    settle().await;
    assert_eq!(w.power.actions(), [PowerAction::LockScreen, PowerAction::Sleep]);
}

#[tokio::test(start_paused = true)]
async fn a_phone_without_remote_control_or_not_in_the_list_fails_after_a_refresh() {
    let w = RcWorld::new(item_with(&["DEVICE_STATUS"]), RemoteControlConfig::default());
    w.sw.w.connect_phone().await;

    assert_eq!(result(&w.drm(RemoteControlType::Lock as i32).await), (FAILED, None));
    assert_eq!(w.refresh.count(), 1);
    assert_eq!(
        w.rc.phone(&phone_id()).unwrap(),
        Some(RemoteControlPhone { toggle: true, applicable: false, capable: false, v2_capable: false })
    );

    // Not in the account's list at all.
    w.sw.peers.remove(&phone_id());
    assert_eq!(result(&w.drm(RemoteControlType::Lock as i32).await), (FAILED, None));
    assert_eq!(w.refresh.count(), 2);
    assert_eq!(w.rc.phone(&phone_id()).unwrap(), None);
    assert!(w.power.actions().is_empty());
}

#[tokio::test(start_paused = true)]
async fn every_address_and_request_type_reaches_the_receiver() {
    let w = RcWorld::connected().await;
    let lock = [0x08, 0x01];

    // Path-addressed with any request type and a query; type-addressed REMOTE_CONTROL without a path.
    for (resource_type, request_type, path) in
        [(4, 1, "/remotecontrol"), (4, 3, "/remotecontrol?from=phone"), (4, 4, "/remotecontrol"), (10, 2, "")]
    {
        let r = w.drm_raw(resource_type, request_type, path, &lock).await;
        assert_eq!(result(&r), (SUCCESS, None), "{resource_type} {request_type} {path:?}");
    }
    settle().await;
    assert_eq!(w.power.actions().len(), 4);
}

#[tokio::test(start_paused = true)]
async fn the_ui_hears_toggle_and_device_list_changes() {
    let w = RcWorld::connected().await;
    let mut rx = w.rc.subscribe();
    assert_eq!(
        w.rc.phone(&phone_id()).unwrap(),
        Some(RemoteControlPhone { toggle: true, applicable: true, capable: true, v2_capable: true })
    );

    w.rc.set_phone_toggle(&phone_id(), true).unwrap();
    assert!(w.events(&mut rx).is_empty(), "no change, no event");

    w.rc.set_phone_toggle(&phone_id(), false).unwrap();
    assert_eq!(w.events(&mut rx), [RemoteControlEvent::PhoneChanged { phone: phone_id() }]);
    assert!(!w.rc.phone(&phone_id()).unwrap().unwrap().toggle);

    w.set_item(item_with(&["REMOTE_CONTROL"]));
    settle().await;
    assert_eq!(w.events(&mut rx), [RemoteControlEvent::PhoneChanged { phone: phone_id() }]);
    assert!(!w.rc.phone(&phone_id()).unwrap().unwrap().v2_capable);
}
