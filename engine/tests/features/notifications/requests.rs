//! The PC's direct requests — outbound notification requests and permission requests: dismiss,
//! clear all, full syncs (b) and (c) with their guards, the silent permission check, the permission
//! prompts and their outcome mapping.

use crate::support::app_world;
use crate::support::common::payload;
use crate::support::notifications_world as support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use app_world::*;
use phonelink_engine::app::consts::permission;
use phonelink_engine::app::{AppOutcome, ConnectKind};
use phonelink_engine::codec::valueset::{Value, ValueSet};
use phonelink_engine::features::notifications::{NotificationsError, NotificationsEvent};
use support::*;

fn is_hex16(s: &str) -> bool {
    s.len() == 16 && s.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

async fn seed(w: &World, keys: &[(&str, i64)]) {
    let texts: Vec<String> = keys.iter().map(|(k, t)| item(k, *t, json!({}))).collect();
    let ks: Vec<&str> = keys.iter().map(|(k, _)| *k).collect();
    let ops = vec![3; keys.len()];
    deliver(&w.h, batch(&ks, &ops, &texts)).await;
}

/// A responder answering `/permission` with `permission_result` and everything else `0`.
fn answer_permissions_with(w: &World, permission_result: i32) {
    w.h.messenger.set_responder(move |req| {
        if req.message.route() == Some("/permission") {
            Reply::Now(result_response(permission_result))
        } else {
            Reply::Now(result_response(0))
        }
    });
}

#[tokio::test(start_paused = true)]
async fn dismiss_deletes_and_withdraws_before_sending_then_checks_permission_silently() {
    let w = world();
    deliver(&w.h, batch(&["k1"], &[1], &[alerting("k1")])).await;

    let notifier = w.w.notifier.clone();
    let seen_withdrawn = Arc::new(Mutex::new(Vec::new()));
    let seen = seen_withdrawn.clone();
    w.h.messenger.set_responder(move |req| {
        if req.message.route() == Some("/notifications") {
            *seen.lock().unwrap() = notifier.withdrawn();
        }
        Reply::Now(result_response(0))
    });

    assert_eq!(w.n.dismiss("k1").await.unwrap(), AppOutcome::Success);
    assert_eq!(*seen_withdrawn.lock().unwrap(), [format!("notifications/{}/k1", phone())]);
    assert!(w.n.list().unwrap().is_empty());

    let req = &notification_requests(&w.h)[0];
    let vs = payload(req);
    let cv = vs.str("correlationVector").unwrap().to_string();
    assert!(is_hex16(&cv));
    assert_eq!(req.trace.correlation_id, cv, "the request's trace correlation id");
    assert_eq!(
        vs.keys().collect::<Vec<_>>(),
        [
            "correlationVector",
            "contentType",
            "operation",
            "key",
            "isuserinitiated",
            "ignoreResponse",
            "transportType",
            "displayName",
            "appVersion",
            "installationId",
            "contractVersion"
        ]
    );
    assert_eq!(vs.str("contentType"), Some("notifications"));
    assert_eq!(vs.i32("operation"), Some(2));
    assert_eq!(vs.str("key"), Some("k1"));
    assert_eq!(vs.bool("isuserinitiated"), Some(false));
    assert_eq!(req.priority, 100);
    assert_eq!(trace_names(req), ("NotificationsDismiss".into(), "Notifications".into()));
    assert!(req.trace.trace_state.contains(&("isuserinitiated".into(), "False".into())));

    settle().await;
    let perm = permission_requests(&w.h);
    assert_eq!(perm.len(), 1, "the silent check follows");
    let p = payload(&perm[0]);
    assert_eq!(p.str("contentType"), Some("permission"));
    assert_eq!(p.i32("permissionType"), Some(64));
    assert_eq!(p.bool("silent"), Some(true));
    assert_eq!(p.str("permissionSessionId"), Some(w.h.app.app_session_id()));
    assert_eq!(perm[0].priority, 100);
    assert_eq!(trace_names(&perm[0]), ("CheckPermissionStatus".into(), "Notifications".into()));
}

#[tokio::test(start_paused = true)]
async fn no_silent_check_after_needs_permission_and_its_outcome_maps_the_device_permission() {
    let w = world();
    seed(&w, &[("a", NOW_MS), ("b", NOW_MS)]).await;
    w.h.messenger.answer_all(result_response(7));

    assert_eq!(w.n.dismiss("a").await.unwrap(), AppOutcome::NeedsPermission);
    settle().await;
    assert!(permission_requests(&w.h).is_empty());

    // The check answered Denied revokes the device permission.
    answer_permissions_with(&w, 5);
    w.n.dismiss("b").await.unwrap();
    settle().await;
    assert!(!w.n.status().unwrap().device_permission);
}

#[tokio::test(start_paused = true)]
async fn clear_all_sends_the_stored_visible_unpinned_items_and_removes_them_on_success() {
    let w = world();
    seed(&w, &[("a", NOW_MS - 1), ("b", NOW_MS - 2)]).await;
    deliver(&w.h, batch(&["m"], &[3], &[item("m", NOW_MS, json!({"notificationClass": 2}))])).await;
    let hidden = r#"{"key":"h","packageName":"com.hidden","postTime":1792454400000}"#.to_string();
    deliver(&w.h, batch(&["h"], &[3], &[hidden])).await;
    w.n.set_pinned("b", true).unwrap();

    // The phone says com.hidden does not sync: its unpinned items are hidden from the list.
    w.h.messenger.answer_all(answer(
        0,
        ValueSet::new().with(
            "savedSetting",
            r#"{"settingKey":"com.hidden","settingValue":"0","settingType":11,"settingGroupId":"NotificationSyncSetting"}"#,
        ),
    ));
    w.n.toggle_app_from_list("com.hidden", false).await.unwrap();
    assert_eq!(w.n.list().unwrap().iter().map(|v| v.key.as_str()).collect::<Vec<_>>(), ["a", "b"]);

    w.h.messenger.answer_all(result_response(0));
    w.w.notifier.take();

    assert_eq!(w.n.clear_all().await.unwrap(), Some(AppOutcome::Success));

    let req = notification_requests(&w.h).pop().unwrap();
    let vs = payload(&req);
    assert_eq!(vs.i32("operation"), Some(4));
    assert_eq!(vs.i32("notificationKeysCount"), Some(1));
    assert_eq!(vs.get("notificationKeys"), Some(&Value::StringArray(vec!["a".into()])));
    assert!(!vs.contains_key("postTimes") && !vs.contains_key("key"));
    assert_eq!(trace_names(&req), ("NotificationsClearAll".into(), "Notifications".into()));
    assert_eq!(w.w.notifier.withdrawn(), [format!("notifications/{}/a", phone())]);
    assert_eq!(w.n.list().unwrap().iter().map(|v| v.key.as_str()).collect::<Vec<_>>(), ["b"]);

    settle().await;
    assert_eq!(permission_requests(&w.h).len(), 1, "the silent check follows a clear all");

    // Nothing left to clear: nothing is sent, not even an empty key list.
    let sent = w.h.messenger.requests().len();
    assert_eq!(w.n.clear_all().await.unwrap(), None);
    settle().await;
    assert_eq!(w.h.messenger.requests().len(), sent);
}

#[tokio::test(start_paused = true)]
async fn a_failed_clear_all_keeps_the_items() {
    let w = world();
    seed(&w, &[("a", NOW_MS)]).await;
    w.h.messenger.answer_all(result_response(1));
    assert_eq!(w.n.clear_all().await.unwrap(), Some(AppOutcome::Failure));
    assert_eq!(w.n.list().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_device_permission_grant_starts_full_sync_b_and_the_silent_check() {
    let w = world_with(Setup { device_permission: false, ..Setup::default() });
    seed(&w, &[("a", NOW_MS - 1), ("b", NOW_MS)]).await;
    let updated = w.n.status().unwrap().last_updated_ms;
    w.w.clock.advance(Duration::from_secs(90));

    let push = phone_content(
        "permissions",
        ValueSet::new()
            .with("itemCount", 1i32)
            .with("permissionTypes", vec![64i32])
            .with("permissionStates", vec![0i32]),
    );

    deliver(&w.h, push).await;
    w.h.messenger.wait_requests(2).await;

    let req = &notification_requests(&w.h)[0];
    let vs = payload(req);
    assert_eq!(vs.i32("operation"), Some(3));
    assert_eq!(vs.i32("notificationKeysCount"), Some(2));
    assert_eq!(vs.get("notificationKeys"), Some(&Value::StringArray(vec!["b".into(), "a".into()])));
    assert_eq!(vs.get("postTimes"), Some(&Value::Int64Array(vec![NOW_MS, NOW_MS - 1])));
    assert_eq!(trace_names(req), ("NotificationsPermissionsChanged".into(), "SyncAllNotifications".into()));
    assert!(req.trace.trace_state.contains(&("isuserinitiated".into(), "False".into())));

    assert_eq!(permission_requests(&w.h).len(), 1);
    settle().await;
    assert_eq!(w.n.status().unwrap().last_updated_ms, updated, "(b) does not touch last updated");
}

/// Holds every `/notifications` request until released; answers the rest at once.
fn hold_notifications(w: &World) -> Held {
    hold_requests(&w.h, |r| r.message.route() == Some("/notifications"))
}

#[tokio::test(start_paused = true)]
async fn full_sync_b_follows_the_pc_switch_and_is_skipped_while_one_is_in_flight() {
    let w = world();
    let held = hold_notifications(&w);

    w.n.set_pc_permission(false).unwrap();
    settle().await;
    assert!(notification_requests(&w.h).is_empty(), "both permissions are needed");

    w.n.set_pc_permission(true).unwrap();
    w.h.messenger.wait_requests(1).await;
    assert!(w.n.status().unwrap().syncing);

    // While it is in flight another qualifying change sends nothing.
    w.n.set_pc_permission(false).unwrap();
    w.n.set_pc_permission(true).unwrap();
    settle().await;
    assert_eq!(notification_requests(&w.h).len(), 1);

    release(&held, 0);
    settle().await;
    assert!(!w.n.status().unwrap().syncing);
    assert_eq!(permission_requests(&w.h).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn full_sync_b_is_skipped_while_full_sync_a_is_in_flight() {
    let w = world();
    let held = hold_notifications(&w);

    w.h.app.send_connect(ConnectKind::Background, "EngineStart").await;
    settle().await;
    assert_eq!(full_syncs(&w.h).len(), 1, "(a) in flight");

    let device =
        |granted: bool| w.h.app.update_phone_record(|r| r.set_permission(permission::NOTIFICATIONS_SYNC, granted));

    device(false);
    settle().await;
    device(true);
    settle().await;
    assert_eq!(full_syncs(&w.h).len(), 1, "(b) is skipped while (a) is in flight");

    // Once (a) has ended, the same change starts (b).
    release(&held, 0);
    settle().await;

    device(false);
    settle().await;
    device(true);
    settle().await;

    let syncs = full_syncs(&w.h);
    assert_eq!(syncs.len(), 2);
    assert_eq!(trace_names(&syncs[1]).0, "NotificationsPermissionsChanged");
}

#[tokio::test(start_paused = true)]
async fn the_sensitive_permission_grant_starts_full_sync_c_only_with_both_permissions() {
    let w = world();
    deliver(&w.h, sensitive_command(true)).await;
    w.h.messenger.wait_requests(1).await;

    let req = &notification_requests(&w.h)[0];
    assert_eq!(payload(req).i32("operation"), Some(3));
    assert_eq!(trace_names(req), ("NotificationsSensitivePermissionsChanged".into(), "SyncAllNotifications".into()));
    settle().await;
    assert_eq!(permission_requests(&w.h).len(), 1);

    // Off, then on again with the PC permission off: nothing.
    deliver(&w.h, sensitive_command(false)).await;
    w.n.set_pc_permission(false).unwrap();
    deliver(&w.h, sensitive_command(true)).await;
    settle().await;
    assert_eq!(notification_requests(&w.h).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn full_sync_c_is_not_skipped_while_one_is_in_flight() {
    let w = world();
    let _held = hold_notifications(&w);

    w.n.set_pc_permission(false).unwrap();
    w.n.set_pc_permission(true).unwrap();
    settle().await;
    assert_eq!(full_syncs(&w.h).len(), 1, "(b) in flight");

    deliver(&w.h, sensitive_command(true)).await;
    settle().await;
    let syncs = full_syncs(&w.h);
    assert_eq!(syncs.len(), 2, "(c) has no in-progress guard");
    assert_eq!(trace_names(&syncs[1]).0, "NotificationsSensitivePermissionsChanged");
}

#[tokio::test(start_paused = true)]
async fn the_access_prompt_maps_its_outcome() {
    let w = world_with(Setup { device_permission: false, ..Setup::default() });
    w.h.messenger.answer_all(result_response(0));
    assert_eq!(w.n.request_access().await.unwrap(), AppOutcome::Success);
    settle().await;
    let req = &permission_requests(&w.h)[0];
    let vs = payload(req);
    assert_eq!(vs.i32("permissionType"), Some(64));
    assert_eq!(vs.bool("silent"), Some(false));
    assert_eq!(req.priority, 0, "user-initiated");
    assert_eq!(trace_names(req), ("PermissionRequired".into(), "Notifications".into()));
    let status = w.n.status().unwrap();
    assert!(status.device_permission && status.setup_completed, "Success: on, and setup completed");

    w.h.messenger.answer_all(result_response(7));
    assert_eq!(w.n.request_access().await.unwrap(), AppOutcome::NeedsPermission);
    settle().await;
    assert!(!w.n.status().unwrap().device_permission, "NeedsPermission: off");

    w.h.messenger.answer_all(result_response(4));
    assert_eq!(w.n.request_access().await.unwrap(), AppOutcome::Accepted);
    settle().await;
    assert!(w.n.status().unwrap().device_permission, "Accepted: on");

    w.h.messenger.answer_all(result_response(6));
    assert_eq!(w.n.request_access().await.unwrap(), AppOutcome::PermanentlyDenied);
    settle().await;
    assert!(!w.n.status().unwrap().device_permission);

    w.h.messenger.answer_all(result_response(18));
    w.n.request_access().await.unwrap();
    assert!(w.n.status().unwrap().low_ram_device);
    settle().await;

    // Unknown leaves the permission as it was.
    w.h.app.update_phone_record(|r| r.set_permission(permission::NOTIFICATIONS_SYNC, true));
    w.h.messenger.answer_all(result_response(2));
    w.n.request_access().await.unwrap();
    assert!(w.n.status().unwrap().device_permission);
}

/// The post-notification setup stage — Android 13 or later, the phone's
/// post-notification permission reported missing (unreported: granted), its auto-launch permission
/// not held (unreported: not held). Every change of the stage is announced as a status change.
#[tokio::test(start_paused = true)]
async fn the_post_notification_stage_follows_the_phones_permissions_and_version() {
    fn needed(w: &World) -> bool {
        w.n.status().unwrap().post_notification_needed
    }
    fn status_changed(events: &mut tokio::sync::broadcast::Receiver<NotificationsEvent>) -> bool {
        drain(events).iter().any(|e| matches!(e, NotificationsEvent::StatusChanged { .. }))
    }

    let w = world();
    let mut events = w.n.subscribe();
    assert!(!needed(&w), "an unreported post-notification permission counts as granted");

    w.h.app.update_phone_record(|r| r.set_permission(permission::POST_NOTIFICATIONS, false));
    settle().await;
    assert!(needed(&w), "Android 14, reported missing");
    assert!(status_changed(&mut events));

    w.h.app.update_phone_record(|r| r.set_permission(permission::AUTO_LAUNCH, true));
    settle().await;
    assert!(!needed(&w), "auto-launch held");
    assert!(status_changed(&mut events));

    w.h.app.update_phone_record(|r| r.set_permission(permission::AUTO_LAUNCH, false));
    settle().await;
    assert!(needed(&w));

    // Below Android 13 (the device list's osVersion) there is no stage.
    let mut item = device_item(Some(ASYNC_TRUST_CUSTOM_DATA));
    item.os_version = Some("12.1".into());
    drain(&mut events);
    w.h.app.update_phone_record(|r| r.apply_device_list(&item));
    settle().await;
    assert!(!needed(&w));
    assert!(status_changed(&mut events));

    // A record change that leaves the stage as it is announces nothing.
    w.h.app.update_phone_record(|r| r.set_permission(permission::CONTACTS, true));
    settle().await;
    assert!(!status_changed(&mut events));
}

#[tokio::test(start_paused = true)]
async fn the_sensitive_prompt_needs_its_capabilities() {
    let w = world();
    assert!(matches!(w.n.request_sensitive_access().await, Err(NotificationsError::NotAvailable(_))));

    w.h.messenger.answer_all(response_with(
        ValueSet::new().with("result", 0i32).with("capabilities", ValueSet::new().with("notifications", 12i32)),
    ));
    w.h.app.send_connect(ConnectKind::Background, "Test").await;

    let status = w.n.status().unwrap();
    assert_eq!(status.capabilities, 12);
    assert!(status.sensitive_prompt_offered);

    let before = permission_requests(&w.h).len();
    w.h.messenger.answer_all(result_response(4));
    w.n.request_sensitive_access().await.unwrap();

    let req = permission_requests(&w.h)[before].clone();
    let vs = payload(&req);
    assert_eq!(vs.i32("permissionType"), Some(-2147483648));
    assert_eq!(vs.bool("silent"), Some(false));
    assert_eq!(req.priority, 0);
    assert_eq!(
        trace_names(&req),
        ("LaunchNotificationsSensitivePermission".into(), "NotificationsSensitivePermission".into())
    );

    // The grant arrives later; the prompt's own outcome changes nothing.
    assert!(!w.n.status().unwrap().sensitive_permission);
}

#[tokio::test(start_paused = true)]
async fn dismiss_sends_operation_2_for_a_key_that_is_not_stored() {
    let w = world();
    assert_eq!(w.n.dismiss("not-stored").await.unwrap(), AppOutcome::Success);
    assert_eq!(payload(&notification_requests(&w.h)[0]).str("key"), Some("not-stored"));
    assert!(w.w.notifier.calls().is_empty(), "nothing was presented, nothing is withdrawn");
}

#[tokio::test(start_paused = true)]
async fn quick_permission_flips_start_full_syncs_b_and_c() {
    let w = world();
    // A revoke and a re-grant that both apply before the feature hears of either: (b) all the same.
    deliver(&w.h, device_permission_push(false)).await;
    deliver(&w.h, device_permission_push(true)).await;
    settle().await;

    assert!(w.n.status().unwrap().device_permission);
    let syncs = full_syncs(&w.h);
    assert_eq!(syncs.len(), 1);
    assert_eq!(trace_names(&syncs[0]).0, "NotificationsPermissionsChanged");

    // The sensitive permission granted, then denied and granted again at once: (c) twice.
    deliver(&w.h, sensitive_command(true)).await;
    settle().await;
    deliver(&w.h, sensitive_command(false)).await;
    deliver(&w.h, sensitive_command(true)).await;
    settle().await;

    let names: Vec<String> = full_syncs(&w.h).iter().map(|r| trace_names(r).0).collect();
    assert_eq!(
        names,
        [
            "NotificationsPermissionsChanged",
            "NotificationsSensitivePermissionsChanged",
            "NotificationsSensitivePermissionsChanged"
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn full_sync_c_leaves_last_updated_alone() {
    let w = world();
    seed(&w, &[("a", NOW_MS)]).await;
    assert_eq!(w.n.status().unwrap().last_updated_ms, Some(NOW_MS));

    w.w.clock.advance(Duration::from_secs(30));
    deliver(&w.h, sensitive_command(true)).await;
    settle().await;

    assert_eq!(full_syncs(&w.h).len(), 1, "(c) answered Success");
    assert_eq!(w.n.status().unwrap().last_updated_ms, Some(NOW_MS), "(c) does not touch last updated");
}

#[tokio::test(start_paused = true)]
async fn the_ancs_capability_bit_is_only_stored() {
    let w = world_with(Setup { device_permission: false, ..Setup::default() });
    seed(&w, &[("k1", NOW_MS)]).await;
    store_capabilities(&w, 1).await;
    let before = permission_requests(&w.h).len();

    // The grant-access prompt and the silent check after a dismiss are sent as without it.
    w.n.request_access().await.unwrap();
    settle().await;
    let after_prompt = permission_requests(&w.h).len();
    assert!(after_prompt > before, "the prompt is sent");

    w.n.dismiss("k1").await.unwrap();
    settle().await;
    let checks =
        permission_requests(&w.h)[after_prompt..].iter().filter(|r| payload(r).bool("silent") == Some(true)).count();
    assert!(checks >= 1, "the silent check follows the dismiss");
}
