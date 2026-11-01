//! Per-app notification settings: requests 25 / 26 / 27 with their triggers and options, the
//! answers, one request in flight at a time, full sync (d), and the local-only settings.

use crate::support::app_world;
use crate::support::common::payload;
use crate::support::notifications_world as support;
use phonelink_engine::config::features::NotificationsConfig;

use serde_json::json;

use app_world::*;
use phonelink_engine::app::AppOutcome;
use phonelink_engine::app::consts::capability;
use phonelink_engine::codec::valueset::ValueSet;
use phonelink_engine::features::notifications::{
    AppFlags, MuteDuration, PreferencesChange, PreferencesResult, PreferencesSource,
};
use support::*;

fn saved(package: &str, value: &str) -> ValueSet {
    ValueSet::new().with(
        "savedSetting",
        format!(
            r#"{{"settingKey":"{package}","settingValue":"{value}","settingType":11,"settingGroupId":"NotificationSyncSetting"}}"#
        ),
    )
}

fn flags(w: &World, package: &str) -> AppFlags {
    w.n.app_setting(package).unwrap().flags
}

#[tokio::test(start_paused = true)]
async fn opening_the_settings_view_syncs_all_settings_with_the_per_app_list() {
    let w = world();
    let sync = r#"{"settings":[
        {"settingKey":"com.a","settingValue":"0","settingType":11,"settingGroupId":"NotificationSyncSetting"},
        {"settingKey":"com.b","settingValue":"1","settingType":11,"settingGroupId":"NotificationSyncSetting","settingMuteExpirationTimestamp":0}]}"#;
    w.h.messenger.answer_all(answer(0, ValueSet::new().with("syncSetting", sync)));
    assert_eq!(w.n.open_settings_view().await.unwrap(), Some(AppOutcome::Success));

    let req = &settings_requests(&w.h)[0];
    let vs = payload(req);
    assert_eq!(
        vs.keys().take(3).collect::<Vec<_>>(),
        ["triggerType", "contentType", "correlationVector"],
        "no setting keys"
    );
    assert_eq!(vs.i32("triggerType"), Some(25));
    assert_eq!(vs.str("contentType"), Some("settings"));
    assert_eq!(vs.str("correlationVector"), Some(req.trace.correlation_id.as_str()));
    assert_eq!(vs.bool("isuserinitiated"), Some(false));
    assert_eq!(req.priority, 100);
    assert_eq!(trace_names(req), ("InitializeNotificationsSettings".into(), "SyncAllSettings".into()));

    // The stored sync rows decide the feed bit once the app has a local row.
    w.n.set_app_local_setting("com.a", AppFlags::BANNER).unwrap();
    w.n.set_app_local_setting("com.b", AppFlags::BANNER).unwrap();
    assert_eq!(flags(&w, "com.a"), AppFlags::BANNER);
    assert_eq!(flags(&w, "com.b"), AppFlags::ALL);
    assert_eq!(flags(&w, "com.unknown"), AppFlags::ALL, "no rows: All");

    // Without the per-app list nothing is sent.
    w.h.app.update_phone_record(|r| r.capability_bits = capability::NOTIFICATIONS_LISTENER);
    assert_eq!(w.n.open_settings_view().await.unwrap(), None);
    assert_eq!(settings_requests(&w.h).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn the_list_toggle_and_its_undo_send_trigger_26_and_write_the_local_flag() {
    let w = world();
    w.h.messenger.answer_all(answer(0, saved("com.a", "0")));
    assert_eq!(w.n.toggle_app_from_list("com.a", false).await.unwrap(), AppOutcome::Success);

    let req = &settings_requests(&w.h)[0];
    let vs = payload(req);
    assert_eq!(
        vs.to_json(),
        json!({
            "settingKey": {"String": "com.a"},
            "settingValue": {"String": "0"},
            "settingType": {"Int32": 11},
            "settingGroupId": {"String": "NotificationSyncSetting"},
            "settingMuteExpirationTimestamp": {"Int64": 0},
            "triggerType": {"Int32": 26},
            "contentType": {"String": "settings"},
            "correlationVector": {"String": req.trace.correlation_id},
            "isuserinitiated": {"Bool": true},
            "ignoreResponse": {"Bool": false},
            "transportType": {"Int32": 0},
            "displayName": {"String": "test-mac"},
            "appVersion": {"String": "1.26082.121.0"},
            "installationId": {"String": w.h.app.installation_id()},
            "contractVersion": {"Double": 3.99}
        })
    );
    assert_eq!(req.priority, 0, "user-initiated");
    assert_eq!(trace_names(req), ("NotificationSettingChangedFromContextMenu".into(), "UpdateSetting".into()));
    assert!(req.trace.trace_state.contains(&("isuserinitiated".into(), "True".into())));
    assert_eq!(flags(&w, "com.a"), AppFlags::NONE);

    w.h.messenger.answer_all(answer(0, saved("com.a", "1")));
    w.n.undo_app_turn_off("com.a", AppFlags::ALL).await.unwrap();
    let req = &settings_requests(&w.h)[1];
    assert_eq!(payload(req).str("settingValue"), Some("1"));
    assert_eq!(trace_names(req).0, "UpdateNotificationsSettingsValue");
    assert_eq!(flags(&w, "com.a"), AppFlags::ALL);
}

#[tokio::test(start_paused = true)]
async fn the_settings_list_sends_26_only_for_a_feed_change_and_falls_back_to_full_sync_d() {
    let w = world();
    // Banner only: local, nothing sent.
    assert_eq!(w.n.change_app_setting("com.a", AppFlags::FEED).await.unwrap(), AppOutcome::Success);
    assert!(settings_requests(&w.h).is_empty());
    assert_eq!(flags(&w, "com.a"), AppFlags::FEED);

    // The feed bit: trigger 26; Unknown ⇒ full sync (d), which records last updated on Success.
    w.w.clock.advance(std::time::Duration::from_secs(7));
    w.h.messenger.set_responder(|req| {
        if req.message.route() == Some("/settings") {
            Reply::Now(result_response(2))
        } else {
            Reply::Now(result_response(0))
        }
    });
    assert_eq!(w.n.change_app_setting("com.a", AppFlags::NONE).await.unwrap(), AppOutcome::Success);

    let req = &settings_requests(&w.h)[0];
    assert_eq!(payload(req).str("settingValue"), Some("0"));
    assert_eq!(trace_names(req), ("NotificationSettingChanged".into(), "UpdateSetting".into()));
    let sync = &notification_requests(&w.h)[0];
    assert_eq!(payload(sync).i32("operation"), Some(3));
    assert_eq!(trace_names(sync), ("NotificationSettingChanged".into(), "SyncAllNotifications".into()));
    assert!(sync.trace.trace_state.contains(&("isuserinitiated".into(), "False".into())));
    assert_eq!(w.n.status().unwrap().last_updated_ms, Some(NOW_MS + 7_000));
    assert!(permission_requests(&w.h).is_empty(), "no silent check after (d)");

    // A failed feed change leaves the banner flag alone.
    w.h.messenger.answer_all(result_response(1));
    assert_eq!(w.n.change_app_setting("com.b", AppFlags::NONE).await.unwrap(), AppOutcome::Failure);
    assert_eq!(flags(&w, "com.b"), AppFlags::ALL);
}

/// The outcome the banner flag waits for is the feed part's: the trigger-26 request's, or that of the full
/// sync (d) that replaced its *Unknown*.
#[tokio::test(start_paused = true)]
async fn the_settings_list_writes_a_changed_banner_flag_when_full_sync_d_succeeds() {
    let w = world();
    // Feed and banner both change; trigger 26 ends Unknown and full sync (d) succeeds.
    w.h.messenger.set_responder(|req| {
        if req.message.route() == Some("/settings") {
            Reply::Now(result_response(2))
        } else {
            Reply::Now(result_response(0))
        }
    });
    assert_eq!(w.n.change_app_setting("com.a", AppFlags::NONE).await.unwrap(), AppOutcome::Success);
    assert_eq!(payload(&notification_requests(&w.h)[0]).i32("operation"), Some(3));

    // No sync row came back, so the feed bit still reads on; the local row lost the banner bit.
    assert_eq!(flags(&w, "com.a"), AppFlags::FEED, "the banner flag was written");

    // Full sync (d) failing leaves the banner flag alone.
    w.h.messenger.set_responder(|req| {
        if req.message.route() == Some("/settings") {
            Reply::Now(result_response(2))
        } else {
            Reply::Now(result_response(1))
        }
    });
    assert_eq!(w.n.change_app_setting("com.b", AppFlags::NONE).await.unwrap(), AppOutcome::Failure);
    assert_eq!(notification_requests(&w.h).len(), 2, "full sync (d) again");
    assert_eq!(flags(&w, "com.b"), AppFlags::ALL);
}

#[tokio::test(start_paused = true)]
async fn one_settings_request_is_in_flight_at_a_time() {
    let w = world();
    let held = hold_requests(&w.h, |_| true);
    let n = w.n.clone();
    let first = tokio::spawn(async move { n.toggle_app_from_list("com.a", false).await });
    let n = w.n.clone();
    let second = tokio::spawn(async move { n.toggle_app_from_list("com.b", false).await });

    settle().await;
    assert_eq!(settings_requests(&w.h).len(), 1);

    release(&held, 0);
    settle().await;
    assert_eq!(settings_requests(&w.h).len(), 2);

    release(&held, 0);
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn preferences_send_one_26_per_app_without_batching() {
    let w = world();
    let change = PreferencesChange {
        source: PreferencesSource::SettingsDialog,
        overrides: vec![("com.a".into(), AppFlags::FEED), ("com.b".into(), AppFlags::NONE)],
        default_matching: vec!["com.c".into()],
        new_default: None,
    };
    assert_eq!(w.n.apply_preferences(change).await.unwrap(), PreferencesResult { succeeded: 3, failed: 0 });

    let sent: Vec<_> = settings_requests(&w.h)
        .iter()
        .map(|r| {
            let vs = payload(r);
            (vs.str("settingKey").unwrap().to_string(), vs.str("settingValue").unwrap().to_string(), trace_names(r))
        })
        .collect();
    let names = ("QbDDialogApply".to_string(), "UpdateSetting".to_string());
    assert_eq!(
        sent,
        [
            ("com.a".to_string(), "1".to_string(), names.clone()),
            ("com.b".to_string(), "0".to_string(), names.clone()),
            ("com.c".to_string(), "1".to_string(), names)
        ]
    );

    // The overrides' local banner flags were written (no sync rows came back: feed reads on).
    assert_eq!(flags(&w, "com.a"), AppFlags::FEED);
    assert_eq!(flags(&w, "com.b"), AppFlags::FEED);
}

#[tokio::test(start_paused = true)]
async fn quiet_by_default_batches_with_trigger_27_and_drives_the_default_banner() {
    let config = NotificationsConfig { quiet_by_default: true, ..NotificationsConfig::default() };
    let w = world_with(Setup { config, ..Setup::default() });
    w.h.app.update_phone_record(|r| r.device_capability_negotiation = Some(8));

    // First setup save: the default is stored, the apps that follow it are not synced.
    let first = PreferencesChange {
        source: PreferencesSource::Setup,
        overrides: vec![("com.a".into(), AppFlags::NONE)],
        default_matching: vec!["com.b".into()],
        new_default: Some(AppFlags::FEED),
    };
    assert_eq!(w.n.apply_preferences(first).await.unwrap(), PreferencesResult { succeeded: 1, failed: 0 });

    let req = &settings_requests(&w.h)[0];
    let vs = payload(req);
    assert_eq!(vs.i32("triggerType"), Some(27));
    assert_eq!(
        vs.str("settings"),
        Some(
            r#"{"settings":[{"settingKey":"com.a","settingValue":"0","settingType":11,"settingGroupId":"NotificationSyncSetting","settingMuteExpirationTimestamp":0}]}"#
        )
    );
    assert_eq!(req.priority, 0);
    assert_eq!(trace_names(req), ("QbDBatchSave".into(), "BatchUpdateSettings".into()));

    // An app without a local row: feed from its sync row, banner from the default (feed only).
    assert_eq!(flags(&w, "com.b"), AppFlags::FEED);

    // A later save syncs the default-following apps too, and clears their local rows.
    w.n.set_app_local_setting("com.b", AppFlags::BANNER).unwrap();
    let later = PreferencesChange {
        source: PreferencesSource::SettingsDialog,
        overrides: vec![],
        default_matching: vec!["com.b".into()],
        new_default: Some(AppFlags::ALL),
    };
    w.h.messenger.answer_all(result_response(1));
    assert_eq!(w.n.apply_preferences(later).await.unwrap(), PreferencesResult { succeeded: 0, failed: 1 });
    assert_eq!(settings_requests(&w.h).len(), 2);
    assert_eq!(flags(&w, "com.b"), AppFlags::ALL, "the local row is gone; the default has a banner");

    // Without the negotiation bit: one 26 per app.
    w.h.app.update_phone_record(|r| r.device_capability_negotiation = Some(0));
    w.h.messenger.answer_all(result_response(0));
    let change = PreferencesChange {
        source: PreferencesSource::SettingsDialog,
        overrides: vec![("com.x".into(), AppFlags::ALL)],
        default_matching: vec![],
        new_default: None,
    };
    w.n.apply_preferences(change).await.unwrap();
    assert_eq!(payload(settings_requests(&w.h).last().unwrap()).i32("triggerType"), Some(26));
}

#[tokio::test(start_paused = true)]
async fn preferences_keep_mutes_and_a_muted_default_row_takes_the_default_banner_bit() {
    let config = NotificationsConfig { quiet_by_default: true, ..NotificationsConfig::default() };
    let w = world_with(Setup { config, ..Setup::default() });

    // A mute writes a row reading All; com.plain's row has no banner bit and no mute.
    w.n.mute_app("com.muted", MuteDuration::OneHour).unwrap();
    w.n.mute_app("com.override", MuteDuration::EightHours).unwrap();
    w.n.set_app_local_setting("com.plain", AppFlags::FEED).unwrap();
    let change = PreferencesChange {
        source: PreferencesSource::SettingsDialog,
        overrides: vec![("com.override".into(), AppFlags::FEED)],
        default_matching: vec!["com.muted".into(), "com.plain".into()],
        new_default: Some(AppFlags::FEED),
    };
    w.n.apply_preferences(change).await.unwrap();

    // The override's local banner flag is written and its mute kept.
    let o = w.n.app_setting("com.override").unwrap();
    assert_eq!((o.flags, o.muted_until_ms), (AppFlags::FEED, Some(NOW_MS + 8 * 3600 * 1000)));

    // A muted default-following app keeps its row and its mute, now holding the default's banner
    // bit (none) instead of All.
    let m = w.n.app_setting("com.muted").unwrap();
    assert_eq!((m.flags, m.muted_until_ms), (AppFlags::FEED, Some(NOW_MS + 3600 * 1000)));

    // An unmuted one loses its row: it follows the default, also when the default changes later.
    assert_eq!(flags(&w, "com.plain"), AppFlags::FEED);
    let later = PreferencesChange {
        source: PreferencesSource::SettingsDialog,
        overrides: vec![],
        default_matching: vec![],
        new_default: Some(AppFlags::ALL),
    };
    w.n.apply_preferences(later).await.unwrap();
    assert_eq!(flags(&w, "com.plain"), AppFlags::ALL);
    assert_eq!(flags(&w, "com.muted"), AppFlags::FEED, "the kept row holds its own banner bit");
}

#[tokio::test(start_paused = true)]
async fn mutes_and_mute_all_are_local_and_reported() {
    let w = world();
    w.n.mute_app("com.a", MuteDuration::EightHours).unwrap();
    let view = w.n.app_setting("com.a").unwrap();
    assert_eq!(view.muted_until_ms, Some(NOW_MS + 8 * 3600 * 1000));
    assert_eq!(view.flags, AppFlags::ALL, "a new row reads All");

    w.n.unmute_app("com.a").unwrap();
    assert_eq!(w.n.app_setting("com.a").unwrap().muted_until_ms, None);

    w.n.set_mute_all(true).unwrap();
    assert_eq!(w.n.status().unwrap().muted_all_until_ms, Some(NOW_MS + 24 * 3600 * 1000));
    w.n.set_mute_all(false).unwrap();
    assert_eq!(w.n.status().unwrap().muted_all_until_ms, None);

    w.n.set_throttling_enabled(false).unwrap();
    assert!(!w.n.status().unwrap().throttling_enabled);

    settle().await;
    assert!(w.h.messenger.requests().is_empty(), "nothing local is sent");
}

#[tokio::test(start_paused = true)]
async fn the_toggle_and_its_undo_write_the_local_flag_whatever_the_request_status() {
    let w = world();
    w.h.messenger.answer_all(result_response(1));
    assert_eq!(w.n.toggle_app_from_list("com.a", false).await.unwrap(), AppOutcome::Failure);
    assert_eq!(flags(&w, "com.a"), AppFlags::FEED, "the banner flag is off although trigger 26 failed");

    w.h.messenger.answer_all(result_response(2));
    assert_eq!(w.n.undo_app_turn_off("com.a", AppFlags::ALL).await.unwrap(), AppOutcome::Unknown);
    assert_eq!(flags(&w, "com.a"), AppFlags::ALL, "and on again although the undo's request ended Unknown");
}

#[tokio::test(start_paused = true)]
async fn preferences_count_accepted_as_synced() {
    let change = || PreferencesChange {
        source: PreferencesSource::SettingsDialog,
        overrides: vec![("com.a".into(), AppFlags::FEED), ("com.b".into(), AppFlags::NONE)],
        default_matching: vec![],
        new_default: None,
    };

    // One trigger 26 per app.
    let w = world();
    w.h.messenger.answer_all(result_response(4));
    assert_eq!(w.n.apply_preferences(change()).await.unwrap(), PreferencesResult { succeeded: 2, failed: 0 });
    assert_eq!(settings_requests(&w.h).len(), 2);

    // One trigger 27.
    let config = NotificationsConfig { quiet_by_default: true, ..NotificationsConfig::default() };
    let w = world_with(Setup { config, ..Setup::default() });
    w.h.app.update_phone_record(|r| r.device_capability_negotiation = Some(8));
    w.h.messenger.answer_all(result_response(4));
    assert_eq!(w.n.apply_preferences(change()).await.unwrap(), PreferencesResult { succeeded: 2, failed: 0 });
    assert_eq!(settings_requests(&w.h).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn the_ancs_capability_bit_changes_no_settings_request() {
    let w = world();
    store_capabilities(&w, 1).await;
    let before = settings_requests(&w.h).len();

    w.n.toggle_app_from_list("com.a", false).await.unwrap();
    assert_eq!(settings_requests(&w.h).len(), before + 1, "trigger 26 of the toggle");

    assert!(w.n.open_settings_view().await.unwrap().is_some(), "trigger 25");
    assert_eq!(settings_requests(&w.h).len(), before + 2);

    let change = PreferencesChange {
        source: PreferencesSource::SettingsDialog,
        overrides: vec![("com.b".into(), AppFlags::NONE)],
        default_matching: vec![],
        new_default: None,
    };
    w.n.apply_preferences(change).await.unwrap();
    assert_eq!(settings_requests(&w.h).len(), before + 3, "trigger 26 of the preferences");
}
