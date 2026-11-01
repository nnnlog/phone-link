//! Device status — the DND and ringer controls, their commands, prompts and confirmation timers,
//! the status shown when data is missing, and the low-battery notification, with where the app
//! differs from Phone Link on Windows.

use crate::support::app_world;
use crate::support::common::payload;
use crate::support::device_status_world::fixture;
use crate::support::feature_world;
use crate::support::platform_world;
use phonelink_engine::config::features::DeviceStatusConfig;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use app_world::*;
use fixture::*;
use phonelink_engine::app::consts::permission;
use phonelink_engine::codec::valueset::{Value, ValueSet};
use phonelink_engine::config::EngineConfig;
use phonelink_engine::edge::testing::MemoryStore;
use phonelink_engine::features::NotificationResponse;
use phonelink_engine::features::StoreLocation;
use phonelink_engine::features::device_status::{
    ControlOutcome, DeviceStatusEvent, DndControl, DndShown, Prompt, RingerControl, RingerShown,
};
use phonelink_engine::features::shared::notifier::testing::NotifierCall;
use phonelink_engine::seams::SessionEvent;
use phonelink_engine::types::DeviceInfoItem;

const COMMAND: &str = "/command/devicestatus";

fn simple(code: i32) -> ValueSet {
    ValueSet::new().with("result", code)
}

fn grant_notifications_sync(f: &Fixture) {
    f.h.app.update_phone_record(|r| r.set_permission(permission::NOTIFICATIONS_SYNC, true));
}

fn dnd(f: &Fixture) -> DndControl {
    f.status().do_not_disturb.expect("the DND control exists")
}

fn ringer(f: &Fixture) -> Option<RingerControl> {
    f.status().ringer
}

fn known(status: i32) -> DndShown {
    DndShown::Status { status, on: status > 1 }
}

/// The commands sent so far as `(type, value)`, checking each one's keys and options.
fn commands(f: &Fixture) -> Vec<(i32, i32)> {
    f.h.messenger
        .requests_on(COMMAND)
        .iter()
        .map(|req| {
            assert_eq!(req.priority, 0, "user-initiated");
            let vs = payload(req);
            let keys: Vec<&str> = vs.keys().collect();
            assert_eq!(
                &keys[..4],
                ["correlationVector", "deviceStatusCommandType", "deviceStatusCommandValue", "contentType"]
            );
            assert_eq!(vs.str("correlationVector"), Some(req.trace.correlation_id.as_str()));
            assert_eq!(vs.str("contentType"), Some("deviceStatusCommand"));
            assert_eq!(vs.get("isuserinitiated"), Some(&Value::Bool(true)));
            assert_eq!(vs.get("transportType"), Some(&Value::Int32(0)));
            (vs.i32("deviceStatusCommandType").unwrap(), vs.i32("deviceStatusCommandValue").unwrap())
        })
        .collect()
}

/// A connected phone that reports DND `dnd` and ringer `ringer`, with notifications sync granted.
async fn ready(dnd_status: i32, ringer_mode: i32) -> Fixture {
    let f = Fixture::new();
    grant_notifications_sync(&f);
    let keys = ValueSet::new().with("doNotDisturbStatus", dnd_status).with("ringerMode", ringer_mode);
    assert_eq!(f.status_item(keys).await, simple(0));
    settle().await;
    f
}

// ------------------------------------------------------------------------------------------------
// DND
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn dnd_toggle_sends_command_2_and_reverts_after_5_s_without_a_change() {
    let mut f = ready(1, 2).await;
    assert_eq!(dnd(&f), DndControl { shown: known(1), enabled: true });
    f.take_events();

    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::Sent { command_type: 2, value: 2 });
    settle().await;
    assert_eq!(commands(&f), vec![(2, 2)]);
    assert_eq!(dnd(&f), DndControl { shown: DndShown::TurningOn, enabled: false });
    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::NotAvailable, "pending");
    assert!(f.take_events().contains(&DeviceStatusEvent::Changed { phone: phone() }));

    tokio::time::sleep(Duration::from_millis(4_900)).await;
    assert_eq!(dnd(&f).shown, DndShown::TurningOn);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(dnd(&f), DndControl { shown: known(1), enabled: true }, "turning on reverts to 1");
    assert!(!f.take_events().iter().any(|e| matches!(e, DeviceStatusEvent::PromptRequested { .. })));
    assert_eq!(commands(&f), vec![(2, 2)], "nothing more sent");
}

#[tokio::test(start_paused = true)]
async fn only_a_change_of_the_phone_link_dnd_confirms() {
    let f = ready(1, 2).await;
    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::Sent { command_type: 2, value: 2 });
    settle().await;

    // The stored value again: not a change.
    assert_eq!(f.status_item(ValueSet::new().with("doNotDisturbStatus", 1i32)).await, simple(0));
    settle().await;
    assert_eq!(dnd(&f).shown, DndShown::TurningOn);

    // A change (also through any phone-content payload): confirmed, shown, no revert later.
    let hb = phone_content("heartbeat", ValueSet::new().with("doNotDisturbStatus", 2i32));
    assert_eq!(f.inbound(hb).await, simple(0));
    settle().await;
    assert_eq!(dnd(&f), DndControl { shown: known(2), enabled: true });

    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(dnd(&f), DndControl { shown: known(2), enabled: true });
}

#[tokio::test(start_paused = true)]
async fn a_reverted_turn_off_offers_launch_dnd_settings() {
    let mut f = ready(2, 2).await;
    assert_eq!(dnd(&f).shown, known(2));
    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::Sent { command_type: 2, value: 1 });
    assert_eq!(dnd(&f).shown, DndShown::TurningOff);

    f.take_events();
    tokio::time::sleep(Duration::from_millis(5_100)).await;
    assert_eq!(dnd(&f), DndControl { shown: known(3), enabled: true }, "turning off reverts to 3");
    let events = f.take_events();
    assert!(events.contains(&DeviceStatusEvent::PromptRequested { phone: phone(), prompt: Prompt::DndTurnOffFailed }));

    f.ds.accept_prompt(&phone(), Prompt::DndTurnOffFailed);
    settle().await;
    assert_eq!(commands(&f), vec![(2, 1), (9, 0)]);

    // Again, and this time the phone reports DND off before the user answers: the prompt is withdrawn.
    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::Sent { command_type: 2, value: 1 });
    tokio::time::sleep(Duration::from_millis(5_100)).await;
    assert!(
        f.take_events()
            .contains(&DeviceStatusEvent::PromptRequested { phone: phone(), prompt: Prompt::DndTurnOffFailed })
    );

    assert_eq!(f.status_item(ValueSet::new().with("doNotDisturbStatus", 1i32)).await, simple(0));
    settle().await;
    assert!(
        f.take_events()
            .contains(&DeviceStatusEvent::PromptWithdrawn { phone: phone(), prompt: Prompt::DndTurnOffFailed })
    );
    assert_eq!(dnd(&f).shown, known(1));

    // Accepting a withdrawn prompt sends nothing.
    f.ds.accept_prompt(&phone(), Prompt::DndTurnOffFailed);
    settle().await;
    assert_eq!(commands(&f), vec![(2, 1), (9, 0), (2, 1)]);
}

#[tokio::test(start_paused = true)]
async fn the_pending_state_for_every_shown_value() {
    // (shown, command value, pending, reverted)
    for (shown, value, pending, reverted) in [
        (0, 1, DndShown::TurningOn, 1),
        (1, 2, DndShown::TurningOn, 1),
        (2, 1, DndShown::TurningOff, 3),
        (3, 1, DndShown::TurningOff, 3),
        (4, 1, DndShown::TurningOn, 1),
    ] {
        let f = ready(shown, 2).await;
        assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::Sent { command_type: 2, value }, "{shown}");
        assert_eq!(dnd(&f).shown, pending, "{shown}");
        tokio::time::sleep(Duration::from_millis(5_100)).await;
        assert_eq!(dnd(&f).shown, known(reverted), "{shown}");

        // A reverted value is shown until the selected value changes; the stored value again is
        // not a change.
        assert_eq!(f.status_item(ValueSet::new().with("doNotDisturbStatus", shown)).await, simple(0));
        settle().await;
        assert_eq!(dnd(&f).shown, known(reverted), "{shown}");
        let next = (shown + 1) % 5;
        assert_eq!(f.status_item(ValueSet::new().with("doNotDisturbStatus", next)).await, simple(0));
        settle().await;
        assert_eq!(dnd(&f).shown, known(next), "{shown}");
    }
}

#[tokio::test(start_paused = true)]
async fn dnd_prompts_and_the_permission_request() {
    let f = Fixture::new();
    assert_eq!(f.status_item(ValueSet::new().with("doNotDisturbStatus", 1i32)).await, simple(0));

    // The notifications-sync permission is unreported: missing.
    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::Prompt(Prompt::DndNotificationsPermission));
    settle().await;
    assert!(commands(&f).is_empty());
    assert_eq!(dnd(&f).shown, known(1), "no pending state");

    // Accepting sends /permission 64, not silent, user-initiated; Accepted grants the permission.
    f.h.messenger.set_responder(|_| Reply::Now(result_response(4)));
    f.ds.accept_prompt(&phone(), Prompt::DndNotificationsPermission);
    settle().await;

    let requests = f.h.messenger.requests_on("/permission");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].priority, 0);
    let vs = payload(&requests[0]);
    assert_eq!(vs.get("permissionType"), Some(&Value::Int32(64)));
    assert_eq!(vs.get("silent"), Some(&Value::Bool(false)));
    assert_eq!(vs.str("contentType"), Some("permission"));
    assert_eq!(vs.str("permissionSessionId"), Some(f.h.app.app_session_id()));
    assert!(f.record().permission_or(permission::NOTIFICATIONS_SYNC, false));

    // Denied clears it again; an unknown result changes nothing.
    f.h.messenger.set_responder(|_| Reply::Now(result_response(5)));
    f.ds.accept_prompt(&phone(), Prompt::RingerNotificationsPermission);
    settle().await;
    assert!(!f.record().permission_or(permission::NOTIFICATIONS_SYNC, true));

    f.h.messenger.set_responder(|_| Reply::Now(result_response(2)));
    f.ds.accept_prompt(&phone(), Prompt::RingerNotificationsPermission);
    settle().await;
    assert_eq!(f.h.messenger.requests_on("/permission").len(), 3);
    assert!(!f.record().permission_or(permission::NOTIFICATIONS_SYNC, true));

    f.h.messenger.set_responder(|_| Reply::Now(result_response(0)));
    f.ds.accept_prompt(&phone(), Prompt::DndNotificationsPermission);
    settle().await;
    assert!(f.record().permission_or(permission::NOTIFICATIONS_SYNC, false), "Success grants");

    // With the permission, a disabled Android 15 DND rule prompts for command 9.
    assert_eq!(f.status_item(ValueSet::new().with("doNotDisturbRuleEnabled", false)).await, simple(0));
    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::Prompt(Prompt::DndRuleDisabled));
    f.ds.accept_prompt(&phone(), Prompt::DndRuleDisabled);
    settle().await;
    assert_eq!(commands(&f), vec![(9, 0)]);

    // A DND tip accepted after the rule came back on sends nothing.
    assert_eq!(f.status_item(ValueSet::new().with("doNotDisturbRuleEnabled", true)).await, simple(0));
    f.ds.accept_prompt(&phone(), Prompt::DndRuleDisabled);
    settle().await;
    assert_eq!(commands(&f), vec![(9, 0)]);

    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::Sent { command_type: 2, value: 2 });
}

#[tokio::test(start_paused = true)]
async fn recompute_triggers_end_a_pending_dnd_without_a_revert() {
    let mut f = ready(2, 2).await;
    assert_eq!(f.status_item(ValueSet::new().with("doNotDisturbRuleEnabled", true)).await, simple(0));
    settle().await;

    // A change of the rule flag.
    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::Sent { command_type: 2, value: 1 });
    assert_eq!(dnd(&f).shown, DndShown::TurningOff);
    assert_eq!(f.status_item(ValueSet::new().with("doNotDisturbRuleEnabled", false)).await, simple(0));
    settle().await;
    assert_eq!(dnd(&f).shown, known(2));

    f.take_events();
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(dnd(&f).shown, known(2), "no revert");
    assert!(!f.take_events().iter().any(|e| matches!(e, DeviceStatusEvent::PromptRequested { .. })));

    // A change of the platform connection.
    assert_eq!(f.status_item(ValueSet::new().with("doNotDisturbRuleEnabled", true)).await, simple(0));
    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::Sent { command_type: 2, value: 1 });
    f.h.sessions.send_event(SessionEvent::Connected(phone()));
    settle().await;
    assert_eq!(dnd(&f).shown, known(2));

    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(dnd(&f).shown, known(2));
}

// ------------------------------------------------------------------------------------------------
// Ringer
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn ringer_cycles_and_reverts_after_20_s() {
    let f = ready(1, 2).await;
    assert_eq!(ringer(&f), Some(RingerControl { shown: RingerShown::Mode(2), enabled: true }));

    assert_eq!(f.ds.cycle_ringer_mode(&phone()), ControlOutcome::Sent { command_type: 3, value: 1 });
    assert_eq!(ringer(&f), Some(RingerControl { shown: RingerShown::TurningVibrate, enabled: false }));
    assert_eq!(f.ds.cycle_ringer_mode(&phone()), ControlOutcome::NotAvailable);

    tokio::time::sleep(Duration::from_millis(19_900)).await;
    assert_eq!(ringer(&f).unwrap().shown, RingerShown::TurningVibrate);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(ringer(&f), Some(RingerControl { shown: RingerShown::Mode(2), enabled: true }), "back to normal");

    // 1 → 0, confirmed by a change of ringerMode.
    assert_eq!(f.status_item(ValueSet::new().with("ringerMode", 1i32)).await, simple(0));
    settle().await;

    assert_eq!(f.ds.cycle_ringer_mode(&phone()), ControlOutcome::Sent { command_type: 3, value: 0 });
    assert_eq!(ringer(&f).unwrap().shown, RingerShown::TurningOff);
    assert_eq!(f.status_item(ValueSet::new().with("ringerMode", 0i32)).await, simple(0));
    settle().await;
    assert_eq!(ringer(&f).unwrap().shown, RingerShown::Mode(0));

    tokio::time::sleep(Duration::from_secs(21)).await;
    assert_eq!(ringer(&f).unwrap().shown, RingerShown::Mode(0));

    // 0 → 2 (turning on), reverts to silent.
    assert_eq!(f.ds.cycle_ringer_mode(&phone()), ControlOutcome::Sent { command_type: 3, value: 2 });
    assert_eq!(ringer(&f).unwrap().shown, RingerShown::TurningOn);
    tokio::time::sleep(Duration::from_secs(21)).await;
    assert_eq!(ringer(&f).unwrap().shown, RingerShown::Mode(0));

    settle().await;
    assert_eq!(commands(&f), vec![(3, 1), (3, 0), (3, 2)]);
}

/// Each control compares record changes with what its own activation saw. A ringer change already written
/// when the user activates DND — its record event not yet processed — still confirms the pending ringer, and
/// the mirror case for DND.
#[tokio::test(start_paused = true)]
async fn activating_one_control_does_not_swallow_the_others_confirmation() {
    let f = ready(1, 2).await;
    assert_eq!(f.ds.cycle_ringer_mode(&phone()), ControlOutcome::Sent { command_type: 3, value: 1 });
    settle().await;

    f.h.app.update_phone_record(|r| r.apply_device_status_item(&ValueSet::new().with("ringerMode", 1i32)));
    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::Sent { command_type: 2, value: 2 });
    settle().await;

    assert_eq!(ringer(&f), Some(RingerControl { shown: RingerShown::Mode(1), enabled: true }), "confirmed");
    assert_eq!(dnd(&f).shown, DndShown::TurningOn, "the DND command stays pending");

    tokio::time::sleep(Duration::from_secs(21)).await;
    assert_eq!(ringer(&f).unwrap().shown, RingerShown::Mode(1), "no revert over the confirmed mode");

    // DND pending, its change written, the ringer activated before the change is processed.
    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::Sent { command_type: 2, value: 2 });
    settle().await;
    f.h.app.update_phone_record(|r| r.apply_device_status_item(&ValueSet::new().with("doNotDisturbStatus", 2i32)));
    assert_eq!(f.ds.cycle_ringer_mode(&phone()), ControlOutcome::Sent { command_type: 3, value: 0 });
    settle().await;

    assert_eq!(dnd(&f), DndControl { shown: known(2), enabled: true }, "confirmed");
    assert_eq!(ringer(&f).unwrap().shown, RingerShown::TurningOff, "the ringer command stays pending");
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert_eq!(dnd(&f).shown, known(2), "no revert");
    settle().await;
    assert_eq!(commands(&f), vec![(3, 1), (2, 2), (2, 2), (3, 0)]);
}

#[tokio::test(start_paused = true)]
async fn ringer_not_supported_and_its_prompt() {
    let f = ready(1, -5).await;
    assert_eq!(ringer(&f), None, "hidden at -5");
    assert_eq!(f.ds.cycle_ringer_mode(&phone()), ControlOutcome::NotAvailable);

    let f = Fixture::new();
    assert_eq!(f.status_item(ValueSet::new().with("ringerMode", 1i32)).await, simple(0));
    assert_eq!(f.ds.cycle_ringer_mode(&phone()), ControlOutcome::Prompt(Prompt::RingerNotificationsPermission));

    f.ds.accept_prompt(&phone(), Prompt::RingerNotificationsPermission);
    settle().await;
    assert_eq!(f.h.messenger.requests_on("/permission").len(), 1);
    assert!(commands(&f).is_empty());
}

#[tokio::test(start_paused = true)]
async fn controls_exist_and_enable_by_the_rules() {
    let f = ready(1, 2).await;
    // Disconnected: unknown and disabled.
    f.h.sessions.set_connected(false);
    assert_eq!(dnd(&f), DndControl { shown: DndShown::Unknown, enabled: false });
    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::NotAvailable);
    assert_eq!(f.ds.cycle_ringer_mode(&phone()), ControlOutcome::NotAvailable);
    f.h.sessions.set_connected(true);

    // All features disabled.
    f.h.app.update_phone_record(|r| r.all_features_disabled = true);
    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::NotAvailable);
    f.h.app.update_phone_record(|r| r.all_features_disabled = false);

    // Android 14 with the post-notification permission denied (an unreported one counts as granted).
    f.h.app.update_phone_record(|r| r.set_permission(permission::POST_NOTIFICATIONS, false));
    assert!(!dnd(&f).enabled);
    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::NotAvailable);

    // A device-list refresh reports Android 12 (the latest write of the phone's osVersion).
    let android_12 = DeviceInfoItem { os_version: Some("12".into()), ..device_item(Some(ASYNC_TRUST_CUSTOM_DATA)) };
    f.h.app.set_phone(phone(), &android_12).unwrap();
    assert!(dnd(&f).enabled, "below Android 13 the permission does not gate");
    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::Sent { command_type: 2, value: 2 });

    // Never delivered: unknown, disabled.
    let f = Fixture::new();
    grant_notifications_sync(&f);
    assert_eq!(dnd(&f), DndControl { shown: DndShown::Unknown, enabled: false });
    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::NotAvailable);

    // Without the action-strip bit there are no controls.
    let h = Harness::with_custom_data(Some(r#"{"yppCapabilities":32,"statusCapabilities":2043}"#));
    let f = Fixture::build(h, StoreLocation::Memory, DeviceStatusConfig::default());
    grant_notifications_sync(&f);
    assert_eq!(
        f.status_item(ValueSet::new().with("doNotDisturbStatus", 1i32).with("ringerMode", 2i32)).await,
        simple(0)
    );
    assert_eq!((f.status().do_not_disturb, f.status().ringer), (None, None));
    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::NotAvailable);

    // Another phone than the selected one has no controls to act on.
    let other = phonelink_engine::types::DcgClient::new("11111111-2222-3333-4444-555555555555", phone().env);
    assert_eq!(f.ds.toggle_do_not_disturb(&other), ControlOutcome::NotAvailable);
}

/// The action strip — and with it the DND and ringer controls — exists only for an Android or iOS phone,
/// whatever its status capabilities.
#[tokio::test(start_paused = true)]
async fn the_controls_need_an_android_or_ios_phone() {
    // No `osName` in the device list (an `LTW` client): platform Unknown, status capabilities 0xFF.
    let item = DeviceInfoItem {
        os_name: None,
        client_type: Some("LTW".into()),
        custom_data: Some(r#"{"yppCapabilities":32,"statusCapabilities":255}"#.into()),
        ..device_item(None)
    };

    let h = Harness::without_phone(Arc::new(MemoryStore::new()), EngineConfig::default());
    h.app.set_phone(phone(), &item).unwrap();
    let f = Fixture::build_with_item(h, StoreLocation::Memory, DeviceStatusConfig::default(), item);
    assert_eq!((f.record().platform, f.record().status_capabilities), (0, 0xFF));

    grant_notifications_sync(&f);
    let keys = ValueSet::new().with("doNotDisturbStatus", 1i32).with("ringerMode", 2i32);
    assert_eq!(f.status_item(keys).await, simple(0));
    settle().await;

    assert_eq!((f.status().do_not_disturb, f.status().ringer), (None, None));
    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::NotAvailable);
    assert_eq!(f.ds.cycle_ringer_mode(&phone()), ControlOutcome::NotAvailable);

    // The connect reply reports the platform: iOS (2) and Android (1) both have the strip.
    for platform in [2i32, 1] {
        f.h.messenger.set_responder(move |req| {
            let values = if req.message.route() == Some("/connect/v1") {
                ValueSet::new().with("result", 1i32).with("attributes", ValueSet::new().with("platform", platform))
            } else {
                ValueSet::new().with("result", 0i32)
            };
            Reply::Now(response_with(values))
        });

        f.h.app.send_connect(phonelink_engine::app::ConnectKind::Immediate, "T").await;
        settle().await;
        assert_eq!(f.record().platform, platform);
        assert!(f.status().do_not_disturb.is_some() && f.status().ringer.is_some(), "platform {platform}");
    }
    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::Sent { command_type: 2, value: 2 });
}

/// A prompt acts on the prompt that applies when it is accepted: the notifications-sync prompt, accepted
/// after the permission was granted and the Android 15 DND rule disabled, sends command 9 — not
/// `/permission`.
#[tokio::test(start_paused = true)]
async fn a_dnd_prompt_accepted_later_acts_on_the_prompt_that_applies_then() {
    let f = Fixture::new();
    assert_eq!(f.status_item(ValueSet::new().with("doNotDisturbStatus", 1i32)).await, simple(0));
    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::Prompt(Prompt::DndNotificationsPermission));

    grant_notifications_sync(&f);
    assert_eq!(f.status_item(ValueSet::new().with("doNotDisturbRuleEnabled", false)).await, simple(0));

    f.ds.accept_prompt(&phone(), Prompt::DndNotificationsPermission);
    settle().await;
    assert!(f.h.messenger.requests_on("/permission").is_empty());
    assert_eq!(commands(&f), vec![(9, 0)]);
}

/// The DND command follows the shown value, a reverted one included: a shown `0` sends `1` (turn off)
/// yet shows *turning on*, which reverts to `1`; the next activation sends `2` (turn on).
#[tokio::test(start_paused = true)]
async fn the_next_dnd_command_follows_a_reverted_value() {
    let f = ready(0, 2).await;
    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::Sent { command_type: 2, value: 1 });
    tokio::time::sleep(Duration::from_millis(5_100)).await;
    assert_eq!(dnd(&f).shown, known(1), "reverted over a selected 0");

    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::Sent { command_type: 2, value: 2 });
    settle().await;
    assert_eq!(commands(&f), vec![(2, 1), (2, 2)]);
}

/// Trace context names: the commands and the notifications-sync `/permission` request carry Phone
/// Link's trigger and scenario names.
#[tokio::test(start_paused = true)]
async fn the_commands_and_the_permission_request_carry_phone_link_trace_names() {
    fn names(req: &phonelink_engine::seams::PlatformRequest) -> (String, String) {
        let state = &req.trace.trace_state;
        assert_eq!((state[0].0.as_str(), state[1].0.as_str()), ("ypTriggerId", "ypScenarioId"));
        (state[0].1.clone(), state[1].1.clone())
    }
    let named = |trigger: &str, scenario: &str| (trigger.to_string(), scenario.to_string());

    // Command 2, command 3, and command 9 from a reverted turn-off's failure prompt.
    let f = ready(2, 2).await;
    assert_eq!(f.ds.toggle_do_not_disturb(&phone()), ControlOutcome::Sent { command_type: 2, value: 1 });
    assert_eq!(f.ds.cycle_ringer_mode(&phone()), ControlOutcome::Sent { command_type: 3, value: 1 });
    tokio::time::sleep(Duration::from_millis(5_100)).await;
    f.ds.accept_prompt(&phone(), Prompt::DndTurnOffFailed);
    settle().await;

    assert_eq!(commands(&f), vec![(2, 1), (3, 1), (9, 0)]);
    let sent: Vec<(String, String)> = f.h.messenger.requests_on(COMMAND).iter().map(names).collect();
    assert_eq!(
        sent,
        [
            named("ToggleDoNotDisturbIndicator", "SendDeviceStatusCommand"),
            named("ToggleRingerIndicator", "SendDeviceStatusCommand"),
            named("LaunchDoNotDisturbSettings", "SendDeviceStatusCommand"),
        ]
    );

    // The notifications-sync permission request of the DND and ringer prompts.
    let f = Fixture::new();
    assert_eq!(f.status_item(ValueSet::new().with("doNotDisturbStatus", 1i32)).await, simple(0));
    f.ds.accept_prompt(&phone(), Prompt::DndNotificationsPermission);
    f.ds.accept_prompt(&phone(), Prompt::RingerNotificationsPermission);
    settle().await;

    let sent: Vec<(String, String)> = f.h.messenger.requests_on("/permission").iter().map(names).collect();
    assert_eq!(sent, vec![named("DeviceActionIndicatorsSetup", "NotificationsDevicePermission"); 2]);
}

// ------------------------------------------------------------------------------------------------
// Low-battery notification
// ------------------------------------------------------------------------------------------------

fn posted(f: &Fixture) -> Vec<phonelink_engine::features::SystemNotification> {
    f.fw.notifier.posted()
}

#[tokio::test(start_paused = true)]
async fn low_battery_notification_rule() {
    let mut f = Fixture::new();
    let battery =
        |level: i32, charging: bool| ValueSet::new().with("batteryPercent", level).with("batteryCharging", charging);

    assert_eq!(f.status_item(battery(10, false)).await, simple(0));
    settle().await;
    assert!(posted(&f).is_empty());

    assert_eq!(f.status_item(battery(9, false)).await, simple(0));
    settle().await;
    let notes = posted(&f);
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].id, format!("device_status/low-battery/{PHONE_ID}.Prod"));
    assert_eq!(notes[0].title, "Pixel: low battery");
    assert_eq!(notes[0].body, "9% battery remaining.");

    // Within the hour: no second one.
    assert_eq!(f.status_item(battery(8, false)).await, simple(0));
    settle().await;
    assert_eq!(posted(&f).len(), 1);

    // An hour later, a change of the charging state alone is not evaluated …
    f.fw.clock.advance(Duration::from_secs(3600));
    assert_eq!(f.status_item(battery(8, true)).await, simple(0));
    assert_eq!(f.status_item(battery(8, false)).await, simple(0));
    settle().await;
    assert_eq!(posted(&f).len(), 1);
    // … a change of the level is.
    assert_eq!(f.status_item(battery(7, false)).await, simple(0));
    settle().await;
    assert_eq!(posted(&f).len(), 2);

    // Charging, unknown, the setting off, or notifications not authorized: none.
    f.fw.clock.advance(Duration::from_secs(3600));
    assert_eq!(f.status_item(battery(6, true)).await, simple(0));
    assert_eq!(f.status_item(battery(-1, false)).await, simple(0));
    settle().await;
    assert_eq!(posted(&f).len(), 2);

    assert!(f.ds.low_battery_notifications_enabled(&phone()).unwrap());
    f.ds.set_low_battery_notifications_enabled(&phone(), false).unwrap();
    assert_eq!(f.status_item(battery(5, false)).await, simple(0));
    settle().await;
    assert_eq!(posted(&f).len(), 2);

    f.ds.set_low_battery_notifications_enabled(&phone(), true).unwrap();
    f.fw.notifier.set_authorized(false);
    assert_eq!(f.status_item(battery(4, false)).await, simple(0));
    settle().await;
    assert_eq!(posted(&f).len(), 2);
    // Not shown means not recorded: authorized again, the next change posts at once.
    f.fw.notifier.set_authorized(true);
    assert_eq!(f.status_item(battery(3, false)).await, simple(0));
    settle().await;
    assert_eq!(posted(&f).len(), 3);

    // Activating it is reported to the UI; the notification names its phone (it may outlive a run).
    f.take_events();
    let note = posted(&f)[2].clone();
    assert_eq!(note.user_info.get("phoneId").map(String::as_str), Some(PHONE_ID));
    assert_eq!(note.user_info.get("environment").map(String::as_str), Some("Prod"));

    f.fw.responses.route(&note.id, &note.user_info, NotificationResponse::Dismissed);
    assert!(f.take_events().is_empty());
    f.fw.responses.route(&note.id, &note.user_info, NotificationResponse::Activated);
    assert!(f.take_events().contains(&DeviceStatusEvent::LowBatteryNotificationActivated { phone: phone() }));
    f.fw.responses.route(&note.id, &HashMap::new(), NotificationResponse::Activated);
    assert!(f.take_events().is_empty(), "no phone named: dropped");
    assert!(f.fw.notifier.calls().iter().all(|c| matches!(c, NotifierCall::Post(_))));
}

#[tokio::test(start_paused = true)]
async fn low_battery_is_evaluated_at_start_and_its_time_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let stores = StoreLocation::Directory(dir.path().to_path_buf());
    let store = Arc::new(MemoryStore::new());
    let harness = || {
        let h = Harness::without_phone(store.clone(), EngineConfig::default());
        h.app.set_phone(phone(), &device_item(Some(ASYNC_TRUST_CUSTOM_DATA))).unwrap();
        h
    };

    let h = harness();
    h.app.update_phone_record(|r| r.status_keys.battery_percent = Some(5));
    let f = Fixture::build(h, stores.clone(), DeviceStatusConfig::default());
    settle().await;
    assert_eq!(posted(&f).len(), 1, "evaluated at start");
    drop(f);

    let f = Fixture::build(harness(), stores.clone(), DeviceStatusConfig::default());
    settle().await;
    assert!(posted(&f).is_empty(), "shown less than an hour ago (persisted)");
    drop(f);

    // The setting is persisted per phone too.
    let f = Fixture::build(harness(), stores.clone(), DeviceStatusConfig::default());
    f.ds.set_low_battery_notifications_enabled(&phone(), false).unwrap();
    drop(f);

    let f = Fixture::build(harness(), stores, DeviceStatusConfig::default());
    assert!(!f.ds.low_battery_notifications_enabled(&phone()).unwrap());
}

#[test]
fn the_confirmation_timers_and_low_battery_rule_have_their_defaults() {
    let defaults = DeviceStatusConfig::default();
    assert_eq!(
        (defaults.do_not_disturb_confirmation, defaults.ringer_confirmation, defaults.low_battery_threshold),
        (Duration::from_secs(5), Duration::from_secs(20), 9)
    );
    assert_eq!(defaults.low_battery_interval, Duration::from_secs(3600));
    assert!(defaults.android15_do_not_disturb);
}

/// `android15_do_not_disturb` gates reading `doNotDisturbRuleEnabled`, which the shared
/// App layer records from every payload whatever the setting, so the engine supports only on and
/// refuses off at install.
#[tokio::test(start_paused = true)]
async fn android15_dnd_off_is_refused_at_install() {
    let h = Harness::new();
    let world = platform_world::World::new(EngineConfig::default());
    let fw = feature_world::feature_world(&h);
    let pc = phonelink_engine::features::device_status::edge::standins::FixedPcStatus::new(Default::default());

    let r = phonelink_engine::features::device_status::DeviceStatus::install(
        &fw.env,
        DeviceStatusConfig { android15_do_not_disturb: false, ..DeviceStatusConfig::default() },
        world.routes.msaep.clone(),
        world.routes.drm.clone(),
        pc,
    );
    assert!(
        matches!(&r, Err(phonelink_engine::features::FeatureError::Config(m)) if m.contains("android15_do_not_disturb")),
        "{:?}",
        r.as_ref().err()
    );
}
