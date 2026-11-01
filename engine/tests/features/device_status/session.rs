//! Device status — both contracts on one session, as the app runs them: the lifecycle sequence,
//! and the status model with what is shown when data is missing — the precedence rule and the
//! presentation.

use crate::support::device_status_world;
use crate::support::platform_world;

use std::sync::Arc;
use std::time::Duration;

use device_status_world::*;
use phonelink_engine::app::consts::permission;
use phonelink_engine::codec::valueset::{Value, ValueSet};
use phonelink_engine::features::device_status::{
    BatteryItem, CellularItem, ControlOutcome, DeviceStatus, DeviceStatusEvent, DndControl, DndShown, Prompt,
    RadioState, RingerControl, RingerShown, WifiItem,
};
use phonelink_engine::proto::device_info::v1::{DeviceStatus as PbDeviceStatus, DeviceStatusResponseStatus};
use phonelink_engine::types::PartnerDisconnectReason;
use platform_world::{phone_id, settle};

const OK: i32 = DeviceStatusResponseStatus::Ok as i32;

/// An `OK` CDEH status with DND `dnd`, ringer `ringer` and battery 50.
fn cdeh(dnd: i32, ringer: i32) -> Vec<u8> {
    status_message(
        OK,
        Some(PbDeviceStatus {
            do_not_disturb_status: dnd,
            ringer_mode: ringer,
            battery_percent: 50,
            ..Default::default()
        }),
    )
}

/// A connected World phone whose DND and ringer come only from CDEH (Phone Link never delivered
/// them), with the notifications-sync permission granted.
async fn cdeh_only(dnd: i32, ringer: i32) -> (StatusWorld, Arc<DeviceStatus>) {
    let sw = StatusWorld::new(phone_item());
    sw.phone_answers_pulls(cdeh(dnd, ringer));
    let ds = sw.install();
    sw.w.connect_phone().await;
    settle().await;

    sw.app.update_phone_record(|r| r.set_permission(permission::NOTIFICATIONS_SYNC, true));
    settle().await;

    let keys = sw.app.phone_record().unwrap().status_keys;
    assert_eq!((keys.do_not_disturb_status, keys.ringer_mode), (None, None), "Phone Link never delivered them");
    (sw, ds)
}

#[tokio::test(start_paused = true)]
async fn the_connect_on_connection_asks_the_phone_for_a_status_sync() {
    let sw = StatusWorld::new(phone_item());
    sw.phone_answers_pulls(PAGE_STATUS_VECTOR.to_vec());
    let _ds = sw.install();
    sw.w.connect_phone().await;
    settle().await;

    // The became-Connected connect carries the six flags; its reply `{result: 0}` has no status
    // keys, so the sync request follows.
    let connects = sw.phone_app.received_on("/connect/v1");
    assert_eq!(connects.len(), 1);
    let configuration = connects[0].object("configuration").unwrap();
    assert_eq!(configuration.get("DeviceIndicatorsDND"), Some(&Value::Bool(true)));

    let syncs = sw.phone_app.received_on("/devicestatus");
    assert_eq!(syncs.len(), 1);
    assert_eq!(syncs[0].str("contentType"), Some("deviceStatus"));

    // CDEH pulled on the same session.
    assert_eq!(sw.pulls().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn one_status_from_both_models_by_the_precedence_rule() {
    let sw = StatusWorld::new(phone_item());
    sw.phone_answers_pulls(PAGE_STATUS_VECTOR.to_vec());
    let ds = sw.install();
    sw.w.connect_phone().await;
    settle().await;

    // CDEH only (the page vector): battery 57 charging, BT on, signal 4/5, data on, Wi-Fi 3.
    let v = ds.status(&phone_id());
    assert!(v.live);
    assert_eq!(v.battery, BatteryItem::Level { percent: 57, charging: true, power_save: false });
    assert_eq!(v.bluetooth, RadioState::On);
    assert_eq!(v.cellular, CellularItem::Bars { level: 4, max: 5, operator: None, four_bar_glyphs: false });
    assert_eq!(v.wifi, WifiItem::Bars(3));
    assert_eq!((v.mobile_data, v.metered), (Some(true), false));
    assert_eq!(v.do_not_disturb, Some(DndControl { shown: DndShown::Status { status: 1, on: false }, enabled: true }));
    assert_eq!(v.ringer, Some(RingerControl { shown: RingerShown::Mode(2), enabled: true }));

    // Phone Link delivers some quantities: they win; the rest stays CDEH's.
    let keys = ValueSet::new()
        .with("batteryPercent", 80i32)
        .with("batteryCharging", false)
        .with("networkSignalStrength", 2i32)
        .with("networkOperator", "Carrier")
        .with("bluetoothState", 10i32)
        .with("ringerMode", 0i32);
    assert_eq!(sw.phone_sends("deviceStatus", keys).await, ValueSet::new().with("result", 0i32));
    settle().await;

    let v = ds.status(&phone_id());
    assert_eq!(v.battery, BatteryItem::Level { percent: 80, charging: false, power_save: false });
    assert_eq!(
        v.cellular,
        CellularItem::Bars { level: 2, max: 5, operator: Some("Carrier".into()), four_bar_glyphs: false }
    );
    assert_eq!(v.bluetooth, RadioState::Off);
    assert_eq!(v.wifi, WifiItem::Bars(3), "still CDEH's");
    assert_eq!(v.ringer.unwrap().shown, RingerShown::Mode(0));

    // A push changes only what Phone Link has not delivered; `is_connection_metered` is CDEH's alone.
    let push = status_message(
        OK,
        Some(PbDeviceStatus {
            battery_percent: 5,
            wifi_signal_level: 0,
            is_connection_metered: true,
            mobile_data_status: 3,
            ..Default::default()
        }),
    );
    sw.phone_publishes(8, "m-1", push).await;

    let v = ds.status(&phone_id());
    assert_eq!(v.battery, BatteryItem::Level { percent: 80, charging: false, power_save: false });
    assert_eq!(v.wifi, WifiItem::NotConnected);
    assert_eq!((v.mobile_data, v.metered), (Some(false), true));

    // Disconnected: everything unknown, the controls disabled, both models kept.
    sw.w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::LocalHubDisconnect);
    settle().await;

    let v = ds.status(&phone_id());
    assert!(!v.live);
    assert_eq!(
        (v.battery, v.wifi, v.bluetooth, v.mobile_data, v.metered),
        (BatteryItem::Unknown, WifiItem::Unknown, RadioState::Unknown, None, false)
    );
    assert_eq!(v.do_not_disturb, Some(DndControl { shown: DndShown::Unknown, enabled: false }));
    assert!(ds.cdeh_status(&phone_id()).is_some());
    assert_eq!(sw.app.phone_record().unwrap().status_keys.battery_percent, Some(80));
}

#[tokio::test(start_paused = true)]
async fn a_timed_out_request_on_the_session_resets_the_wifi_state() {
    let sw = StatusWorld::new(phone_item());
    sw.phone_answers_pulls(PAGE_STATUS_VECTOR.to_vec());
    let _ds = sw.install();
    sw.w.connect_phone().await;
    settle().await;

    assert_eq!(
        sw.phone_sends("heartbeat", ValueSet::new().with("wifiState", false)).await,
        ValueSet::new().with("result", 0i32)
    );
    assert_eq!(sw.app.phone_record().unwrap().status_keys.wifi_state, Some(false));

    // The phone stops answering App requests: the next one times out and the value is released.
    sw.phone_app.set_answer(|_, _| None);
    let request = phonelink_engine::app::AppRequest::kvp("/x", ValueSet::new())
        .with_options(phonelink_engine::app::RequestOptions::default().with_timeout(Duration::from_secs(5)));
    let outcome = sw.app.request(request).await.unwrap().outcome;
    assert_eq!(outcome, phonelink_engine::app::AppOutcome::TimedOut);
    assert_eq!(sw.app.phone_record().unwrap().status_keys.wifi_state, None);
}

// ------------------------------------------------------------------------------------------------
// Reverted values and the selected value (confirmation re-reads)
// ------------------------------------------------------------------------------------------------

/// A reverted value is shown until the selected value changes, and not again when the selected value later
/// returns to what it was at the revert.
#[tokio::test(start_paused = true)]
async fn a_reverted_dnd_value_ends_when_the_selected_value_changes() {
    let (sw, ds) = cdeh_only(2, 2).await;
    let shown = || ds.status(&phone_id()).do_not_disturb.unwrap().shown;

    assert_eq!(shown(), DndShown::Status { status: 2, on: true });
    assert_eq!(ds.toggle_do_not_disturb(&phone_id()), ControlOutcome::Sent { command_type: 2, value: 1 });
    assert_eq!(shown(), DndShown::TurningOff);

    tokio::time::sleep(Duration::from_millis(5_100)).await;
    assert_eq!(shown(), DndShown::Status { status: 3, on: true }, "reverted (turning off → 3)");

    sw.phone_publishes(8, "m-1", cdeh(1, 2)).await;
    assert_eq!(shown(), DndShown::Status { status: 1, on: false }, "a change of the selected value ends the revert");
    sw.phone_publishes(8, "m-2", cdeh(2, 2)).await;
    assert_eq!(shown(), DndShown::Status { status: 2, on: true }, "the stale reverted 3 does not come back");
}

/// Ringer: a CDEH change while pending does not stop the 20 s delay (only a Phone Link change
/// does), so the revert lands over another selected value; once that value
/// changes, the reverted mode is gone for good.
#[tokio::test(start_paused = true)]
async fn a_reverted_ringer_value_ends_when_the_selected_value_changes() {
    let (sw, ds) = cdeh_only(1, 2).await;
    let shown = || ds.status(&phone_id()).ringer.unwrap().shown;

    assert_eq!(ds.cycle_ringer_mode(&phone_id()), ControlOutcome::Sent { command_type: 3, value: 1 });
    sw.phone_publishes(8, "m-1", cdeh(1, 0)).await;
    assert_eq!(shown(), RingerShown::TurningVibrate, "a CDEH change does not confirm");

    tokio::time::sleep(Duration::from_secs(21)).await;
    assert_eq!(shown(), RingerShown::Mode(2), "reverted to normal over a selected 0");

    sw.phone_publishes(8, "m-2", cdeh(1, 1)).await;
    assert_eq!(shown(), RingerShown::Mode(1));
    sw.phone_publishes(8, "m-3", cdeh(1, 0)).await;
    assert_eq!(shown(), RingerShown::Mode(0), "the stale reverted 2 does not come back");
}

/// On the wire, the next ringer command follows the selected mode, not a stale reverted one (`2 → 1 → 0 → 2`:
/// from silent, normal).
#[tokio::test(start_paused = true)]
async fn the_next_ringer_command_follows_the_selected_mode_after_a_revert_ended() {
    let (sw, ds) = cdeh_only(1, 2).await;
    assert_eq!(ds.cycle_ringer_mode(&phone_id()), ControlOutcome::Sent { command_type: 3, value: 1 });
    sw.phone_publishes(8, "m-1", cdeh(1, 0)).await;
    tokio::time::sleep(Duration::from_secs(21)).await;
    sw.phone_publishes(8, "m-2", cdeh(1, 1)).await;
    sw.phone_publishes(8, "m-3", cdeh(1, 0)).await;

    assert_eq!(ds.cycle_ringer_mode(&phone_id()), ControlOutcome::Sent { command_type: 3, value: 2 });
    settle().await;

    let values: Vec<Option<i32>> =
        sw.phone_app.received_on("/command/devicestatus").iter().map(|vs| vs.i32("deviceStatusCommandValue")).collect();
    assert_eq!(values, vec![Some(1), Some(2)]);
}

/// The rule-disabled prompt reads the rule flag of the one status per phone: CDEH's
/// `do_not_disturb_rule_enabled = false` while Phone Link never delivered `doNotDisturbRuleEnabled`
/// (Phone Link on Windows shows no prompt then); once Phone Link delivers the flag, its value wins.
#[tokio::test(start_paused = true)]
async fn the_rule_disabled_prompt_reads_the_selected_rule_flag() {
    let sw = StatusWorld::new(phone_item());
    sw.phone_answers_pulls(status_message(
        OK,
        Some(PbDeviceStatus {
            do_not_disturb_status: 1,
            ringer_mode: 2,
            battery_percent: 50,
            do_not_disturb_rule_enabled: Some(false),
            ..Default::default()
        }),
    ));

    let ds = sw.install();
    sw.w.connect_phone().await;
    settle().await;
    sw.app.update_phone_record(|r| r.set_permission(permission::NOTIFICATIONS_SYNC, true));
    settle().await;

    assert_eq!(sw.app.phone_record().unwrap().status_keys.do_not_disturb_rule_enabled, None);
    assert_eq!(ds.toggle_do_not_disturb(&phone_id()), ControlOutcome::Prompt(Prompt::DndRuleDisabled));
    ds.accept_prompt(&phone_id(), Prompt::DndRuleDisabled);
    settle().await;

    // Phone Link delivers the flag (enabled): its value is the selected one.
    let keys = ValueSet::new().with("doNotDisturbRuleEnabled", true);
    assert_eq!(sw.phone_sends("deviceStatus", keys).await, ValueSet::new().with("result", 0i32));
    settle().await;
    assert_eq!(ds.toggle_do_not_disturb(&phone_id()), ControlOutcome::Sent { command_type: 2, value: 2 });
    settle().await;

    let commands: Vec<(Option<i32>, Option<i32>)> = sw
        .phone_app
        .received_on("/command/devicestatus")
        .iter()
        .map(|vs| (vs.i32("deviceStatusCommandType"), vs.i32("deviceStatusCommandValue")))
        .collect();
    assert_eq!(commands, vec![(Some(9), Some(0)), (Some(2), Some(2))]);
}

/// A reverted turn-off over a selected DND that is already `1` (CDEH reported it while the change
/// was pending, which does not stop the timer) raises no failure prompt.
#[tokio::test(start_paused = true)]
async fn no_failure_prompt_when_the_selected_dnd_is_off_at_the_revert() {
    let (sw, ds) = cdeh_only(3, 2).await;
    let mut events = ds.subscribe();

    assert_eq!(ds.toggle_do_not_disturb(&phone_id()), ControlOutcome::Sent { command_type: 2, value: 1 });
    sw.phone_publishes(8, "m-1", cdeh(1, 2)).await;
    assert_eq!(ds.status(&phone_id()).do_not_disturb.unwrap().shown, DndShown::TurningOff);

    tokio::time::sleep(Duration::from_millis(5_100)).await;
    assert_eq!(ds.status(&phone_id()).do_not_disturb.unwrap().shown, DndShown::Status { status: 3, on: true });
    assert!(
        !StatusWorld::events(&mut events)
            .contains(&DeviceStatusEvent::PromptRequested { phone: phone_id(), prompt: Prompt::DndTurnOffFailed })
    );
}

/// The bars and their maximum are one quantity: a Phone Link level delivered without a maximum shows of Phone
/// Link's default 5, not of an `OK` CDEH status's `-1`.
#[tokio::test(start_paused = true)]
async fn cellular_bars_take_their_maximum_from_the_levels_model() {
    let sw = StatusWorld::new(phone_item());
    sw.phone_answers_pulls(status_message(OK, None));
    let ds = sw.install();
    sw.w.connect_phone().await;
    settle().await;

    assert!(ds.cdeh_status(&phone_id()).is_some(), "an OK status without info");

    let keys = ValueSet::new().with("networkSignalStrength", 3i32);
    assert_eq!(sw.phone_sends("deviceStatus", keys).await, ValueSet::new().with("result", 0i32));
    settle().await;
    assert_eq!(
        ds.status(&phone_id()).cellular,
        CellularItem::Bars { level: 3, max: 5, operator: None, four_bar_glyphs: false }
    );

    // A CDEH level shows of CDEH's maximum; one outside 4 or 5 (field 8 absent → 0) shows of 5.
    let sw = StatusWorld::new(phone_item());
    sw.phone_answers_pulls(status_message(
        OK,
        Some(PbDeviceStatus { network_signal_strength: 2, ..Default::default() }),
    ));

    let ds = sw.install();
    sw.w.connect_phone().await;
    settle().await;

    assert_eq!(
        ds.status(&phone_id()).cellular,
        CellularItem::Bars { level: 2, max: 5, operator: None, four_bar_glyphs: false }
    );

    sw.phone_publishes(
        8,
        "m-1",
        status_message(
            OK,
            Some(PbDeviceStatus { network_signal_strength: 2, network_signal_strength_max: 4, ..Default::default() }),
        ),
    )
    .await;
    assert_eq!(
        ds.status(&phone_id()).cellular,
        CellularItem::Bars { level: 2, max: 4, operator: None, four_bar_glyphs: false }
    );
}

// ------------------------------------------------------------------------------------------------
// Low battery from CDEH; re-reads of pending controls
// ------------------------------------------------------------------------------------------------

/// The low-battery rule runs on the battery the precedence rule selects, here a tag-8 push.
#[tokio::test(start_paused = true)]
async fn a_low_battery_pushed_over_cdeh_posts_the_notification() {
    let sw = StatusWorld::new(phone_item());
    let _ds = sw.install();
    sw.w.connect_phone().await;
    settle().await;

    let push = status_message(OK, Some(PbDeviceStatus { battery_percent: 5, ..Default::default() }));
    sw.phone_publishes(8, "m-1", push).await;
    settle().await;
    assert_eq!(sw.notifier.posted().len(), 1);
}

/// A change of the post-notification permission re-reads a pending DND, and a change of the
/// all-features-disabled flag a pending ringer: no revert follows.
#[tokio::test(start_paused = true)]
async fn permission_and_disabled_changes_end_pending_controls() {
    let sw = StatusWorld::new(phone_item());
    let ds = sw.install();
    sw.w.connect_phone().await;
    settle().await;

    sw.app.update_phone_record(|r| r.set_permission(permission::NOTIFICATIONS_SYNC, true));
    let keys = ValueSet::new().with("doNotDisturbStatus", 2i32).with("ringerMode", 2i32);
    sw.phone_sends("deviceStatus", keys).await;
    settle().await;

    let dnd = || ds.status(&phone_id()).do_not_disturb.unwrap().shown;
    let ringer = || ds.status(&phone_id()).ringer.unwrap().shown;

    assert_eq!(ds.toggle_do_not_disturb(&phone_id()), ControlOutcome::Sent { command_type: 2, value: 1 });
    assert_eq!(dnd(), DndShown::TurningOff);
    sw.app.update_phone_record(|r| r.set_permission(permission::POST_NOTIFICATIONS, false));
    settle().await;
    assert_eq!(dnd(), DndShown::Status { status: 2, on: true }, "a permission change re-reads");

    sw.app.update_phone_record(|r| r.set_permission(permission::POST_NOTIFICATIONS, true));
    settle().await;
    assert_eq!(ds.cycle_ringer_mode(&phone_id()), ControlOutcome::Sent { command_type: 3, value: 1 });
    assert_eq!(ringer(), RingerShown::TurningVibrate);

    sw.app.update_phone_record(|r| r.all_features_disabled = true);
    settle().await;
    sw.app.update_phone_record(|r| r.all_features_disabled = false);
    settle().await;
    assert_eq!(ringer(), RingerShown::Mode(2), "re-read");

    tokio::time::sleep(Duration::from_secs(21)).await;
    assert_eq!(ringer(), RingerShown::Mode(2));
    assert_eq!(dnd(), DndShown::Status { status: 2, on: true });
}
