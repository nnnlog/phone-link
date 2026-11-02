//! The `uniffi` feature objects of `src/api/` over the fakes (`support/engine_world`): before a
//! start every object reports `NotStarted` and the same objects work once the engine starts (each call
//! resolves the current core); they read the selected phone, whose OS version is the last writer's; a
//! notification the phone pushes reaches the event sink as a typed change and is listed, pinned and
//! dismissed through the object; waking the Mac removes the phone with `SystemReset`, re-wakes it and
//! sends the unlock-or-wake connect; a stale PC of the account is listed and unlinked.

use crate::support::auth_world;
use crate::support::engine_world as world;
use crate::support::scripted_phone;

use std::time::{Duration, UNIX_EPOCH};

use auth_world::harness::eventually;
use phonelink_engine::api::FeatureApiError;
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::events::EngineEvent;
use phonelink_engine::api::messaging::MessagePartRef;
use phonelink_engine::api::notifications::{NotificationOpenTarget, NotificationsChange};
use phonelink_engine::codec::valueset::ValueSet;
use phonelink_engine::edge::Clock;
use phonelink_engine::engine::SystemEvent;
use scripted_phone::app_request;
use world::World;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn objects_taken_before_the_start_report_not_started_then_read_the_phone() {
    let w = World::new(None).await;
    let engine = Engine::with_config(w.config(), w.h.edges(), World::feature_edges(), w.h.events.clone());
    let (notifications, messaging, photos, contacts, clipboard, status, phone) = (
        engine.clone().notifications(),
        engine.clone().messaging(),
        engine.clone().photos(),
        engine.clone().contacts(),
        engine.clone().clipboard(),
        engine.clone().device_status(),
        engine.clone().phone(),
    );
    let wallpaper = engine.clone().wallpaper();

    assert_eq!(notifications.list().unwrap_err(), FeatureApiError::NotStarted);
    assert_eq!(messaging.conversations().unwrap_err(), FeatureApiError::NotStarted);
    assert_eq!(photos.gallery().unwrap_err(), FeatureApiError::NotStarted);
    assert_eq!(contacts.state().unwrap_err(), FeatureApiError::NotStarted);
    assert_eq!(clipboard.global_switch().unwrap_err(), FeatureApiError::NotStarted);
    assert_eq!(status.status().unwrap_err(), FeatureApiError::NotStarted);
    assert_eq!(phone.info().unwrap_err(), FeatureApiError::NotStarted);
    assert_eq!(wallpaper.wallpaper().unwrap_err(), FeatureApiError::NotStarted);
    engine.system_event(SystemEvent::DidWake); // before a start: ignored

    engine.start().await.expect("start");
    let info = phone.info().expect("the selected phone");
    assert_eq!((info.id.as_str(), info.name.as_str(), info.connected), (w.phone_id.as_str(), "Pixel", false));

    assert!(notifications.list().unwrap().is_empty());
    assert!(notifications.state().unwrap().pc_permission, "show phone notifications is on by default");
    assert!(messaging.conversations().unwrap().is_empty());
    assert!(messaging.state().unwrap().settings.pc_consent);
    assert!(messaging.new_conversation_messages().unwrap().is_empty());
    let limits = messaging.new_message_limits().unwrap();
    assert!(limits.attachments_allowed && limits.group_allowed, "no SIM reported: nothing is limited");
    assert_eq!(
        messaging.part_bytes(MessagePartRef::MmsPart { id: 7 }).unwrap_err(),
        FeatureApiError::UnknownItem { what: "part".into() }
    );
    assert!(contacts.suggestions("a".into(), 8).unwrap().is_empty());
    assert!(photos.gallery().unwrap().is_empty());
    assert!(contacts.state().unwrap().pc_consent);
    assert!(clipboard.global_switch().unwrap(), "the global switch is on by default");
    let clip = clipboard.phone_state().unwrap().expect("the phone is in the account's list");
    assert!(!clip.toggle, "the per-phone toggle is off by default");
    assert!(!status.status().unwrap().live, "nothing is live before the phone connects");
    assert_eq!(wallpaper.wallpaper().unwrap(), None, "no wallpaper yet");

    w.connect_phone(|| engine.status().phone_connected).await;
    assert!(eventually(Duration::from_secs(10), || phone.info().unwrap().connected).await);
    assert!(
        w.h.events.take().iter().any(|e| matches!(e, EngineEvent::PhoneSession { connected: true, failure: None })),
        "the session event"
    );
}

/// The phone record from the device list (Android): the record keeps one OS version, the last
/// writer winning — the phone object shows the connect reply's `attributes.osVersion` once the
/// reply came, and the device list's again after a refresh.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_phone_os_version_is_the_last_writers() {
    let w = World::new(None).await;
    *w.connect_reply.lock().unwrap() =
        world::default_connect_reply().with("attributes", ValueSet::new().with("osVersion", "15.1"));

    let engine = Engine::with_config(w.config(), w.h.edges(), World::feature_edges(), w.h.events.clone());
    engine.start().await.expect("start");
    let phone = engine.clone().phone();
    assert_eq!(phone.info().unwrap().os_version.as_deref(), Some("14"), "the device list's");

    w.connect_phone(|| engine.status().phone_connected).await;
    assert!(
        eventually(Duration::from_secs(20), || phone.info().unwrap().os_version.as_deref() == Some("15.1")).await,
        "the connect reply came last"
    );

    let core = engine.current_core().expect("a core");
    core.auth.sync_trust(phonelink_engine::auth::FetchMode::ForceCloud).await.expect("cloud refresh");
    assert!(
        eventually(Duration::from_secs(10), || phone.info().unwrap().os_version.as_deref() == Some("14")).await,
        "the device list came last"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pushed_notification_is_a_typed_change_and_is_listed_pinned_and_dismissed() {
    let w = World::new(None).await;
    let engine = Engine::with_config(w.config(), w.h.edges(), World::feature_edges(), w.h.events.clone());
    engine.start().await.expect("start");
    w.connect_phone(|| engine.status().phone_connected).await;
    let notifications = engine.clone().notifications();

    let now_ms = w.h.clock.now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
    let key = "0|com.example.chat|42|tag|10123";
    let item = serde_json::json!({
        "key": key, "packageName": "com.example.chat", "appName": "Chat",
        "title": "Ada", "text": "Lunch?", "postTime": now_ms, "importance": 4,
        "notificationActions": [{"actionName": "Reply", "isActionInlineReply": true, "actionIndex": 0}],
    })
    .to_string();

    w.send(&app_request(
        "/legacy/phonecontent",
        "nt-1",
        &ValueSet::new()
            .with("contentType", "notifications")
            .with("correlationVector", "0123456789abcdef")
            .with("contractVersion", 3.99f64)
            .with("notificationKeys", vec![key])
            .with("operations", vec![1i32])
            .with("notifications", vec![item.as_str()]),
    ));

    assert!(eventually(Duration::from_secs(20), || notifications.list().unwrap().len() == 1).await, "listed");
    assert!(
        w.h.events
            .take()
            .iter()
            .any(|e| matches!(e, EngineEvent::Notifications { change: NotificationsChange::ListChanged })),
        "the change reached the event sink"
    );

    let listed = notifications.list().unwrap().remove(0);
    assert_eq!(
        (listed.key.as_str(), listed.app_name.as_deref(), listed.title.as_deref(), listed.body.as_str()),
        (key, Some("Chat"), Some("Ada"), "Lunch?")
    );
    assert_eq!(listed.open_target, NotificationOpenTarget::Notifications);
    assert!(listed.reply.as_ref().is_some_and(|r| r.inline_reply && r.name == "Reply"));

    notifications.set_pinned(key.into(), true).unwrap();
    assert!(notifications.list().unwrap()[0].pinned);
    assert_eq!(
        notifications.set_pinned("no such key".into(), true).unwrap_err(),
        FeatureApiError::UnknownItem { what: "notification".into() }
    );

    // Dismiss removes the row at once and sends operation 2; the scripted phone does not answer it,
    // so the request is left to time out in its task.
    let dismissing = tokio::spawn({
        let notifications = notifications.clone();
        let key = key.to_string();
        async move { notifications.dismiss(key).await }
    });
    assert!(eventually(Duration::from_secs(10), || notifications.list().unwrap().is_empty()).await, "dismissed");
    assert!(
        eventually(Duration::from_secs(20), || w.requests_on("/notifications").iter().any(|r| World::values(
            &r.message
        )
        .i32("operation")
            == Some(2)))
        .await,
        "operation 2 reached the phone"
    );
    dismissing.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn waking_removes_the_phone_with_system_reset_rewakes_it_and_sends_the_wake_connect() {
    let w = World::new(None).await;
    let engine = Engine::with_config(w.config(), w.h.edges(), World::feature_edges(), w.h.events.clone());
    engine.start().await.expect("start");
    w.connect_phone(|| engine.status().phone_connected).await;
    let wakes = w.h.dcg.count("Wake");
    w.h.events.take();

    engine.system_event(SystemEvent::ScreenLocked);
    engine.system_event(SystemEvent::DidWake);
    assert!(
        eventually(Duration::from_secs(10), || w.h.events.take().iter().any(|e| matches!(
            e,
            EngineEvent::PhoneSession { connected: false, failure: Some(f) } if f.contains("SystemReset")
        )))
        .await,
        "the partner was removed with SystemReset"
    );

    assert!(eventually(Duration::from_secs(30), || w.h.dcg.count("Wake") > wakes).await, "the phone was woken again");
    w.connect_phone(|| engine.status().phone_connected).await;

    let connects = |trigger: &str| {
        w.requests_on("/connect/v1")
            .into_iter()
            .filter(|r| r.trace_state("ypTriggerId").as_deref() == Some(trigger))
            .count()
    };
    assert!(
        eventually(Duration::from_secs(30), || connects("UnlockOrWake") > 0).await,
        "the background connect of unlock or wake"
    );
    assert_eq!(connects("LockOrSleep"), 0, "lock and sleep send no connect");
}

/// The account's PCs: a stale PC of the account is listed and unlinked (it leaves the list after the
/// forced refresh); this Mac's own identities and a phone are refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stale_pc_of_the_account_is_listed_and_unlinked() {
    let w = World::new(None).await;
    let mut stale = w.h.phone("old-mac").item;
    stale.client_type = Some("WEA".into());
    stale.os_name = Some("Windows".into());
    stale.last_seen_time = Some(100);
    let stale_id = stale.id.clone();
    w.h.dcg.add_peer(stale);

    let engine = Engine::with_config(w.config(), w.h.edges(), World::feature_edges(), w.h.events.clone());
    engine.start().await.expect("start");
    let phone = engine.clone().phone();

    let pcs = phone.account_pcs().expect("the account's PCs");
    let listed = pcs.iter().find(|p| p.id == stale_id).expect("the stale PC is listed");
    assert_eq!((listed.name.as_str(), listed.client_type.as_str(), listed.this_mac), ("old-mac", "WEA", false));
    assert!(pcs.iter().all(|p| p.id != w.phone_id), "a phone is not a PC");

    let core = engine.current_core().expect("a core");
    for own in core.own_clients() {
        assert!(phone.unlink_pc(own.id.clone()).await.is_err(), "this Mac's own identity is refused");
    }
    assert!(phone.unlink_pc(w.phone_id.clone()).await.is_err(), "a phone is refused");

    phone.unlink_pc(stale_id.clone()).await.expect("unlinked");
    assert!(phone.account_pcs().unwrap().iter().all(|p| p.id != stale_id), "gone after the refresh");
}

/// Every feature object the UI holds that no other test drives reports `NotStarted` before a start
/// and answers a read once the engine has started.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_other_feature_objects_answer_once_started() {
    let w = World::new(None).await;
    let engine = Engine::with_config(w.config(), w.h.edges(), World::feature_edges(), w.h.events.clone());

    let audio = engine.clone().audio();
    let hotspot = engine.clone().hotspot();
    let remote_control = engine.clone().remote_control();
    let remote_snipping = engine.clone().remote_snipping();
    let mirroring = engine.clone().screen_mirroring();
    let phone_apps = engine.clone().phone_apps();
    let phone_files = engine.clone().phone_files();
    let file_transfer = engine.clone().file_transfer();
    let camera = engine.clone().connected_camera();
    let calling = engine.clone().calling();

    let not_started = |what: &str, e: FeatureApiError| assert_eq!(e, FeatureApiError::NotStarted, "{what}");
    not_started("audio", audio.controls().unwrap_err());
    not_started("hotspot", hotspot.phone_state().unwrap_err());
    not_started("remote control", remote_control.phone_state().unwrap_err());
    not_started("remote snipping", remote_snipping.settings().unwrap_err());
    not_started("screen mirroring", mirroring.status().unwrap_err());
    not_started("phone apps", phone_apps.state().unwrap_err());
    not_started("phone files", phone_files.state().unwrap_err());
    not_started("file transfer", file_transfer.settings().unwrap_err());
    not_started("connected camera", camera.status("en".into()).unwrap_err());
    not_started("calling", calling.state().unwrap_err());

    engine.start().await.expect("start");
    audio.controls().expect("audio");
    hotspot.phone_state().expect("hotspot");
    remote_control.phone_state().expect("remote control");
    remote_snipping.settings().expect("remote snipping");
    mirroring.status().expect("screen mirroring");
    phone_apps.state().expect("phone apps");
    phone_files.state().expect("phone files");
    file_transfer.settings().expect("file transfer");
    camera.status("en".into()).expect("connected camera");
    let calls = calling.state().expect("calling");
    assert!(!calls.pc_consent, "call history is off on a new Mac");
    assert!(calling.history().expect("call history").is_empty(), "history behind the prompt");
    assert_eq!(calling.refresh_if_empty().await.expect("refresh"), None, "refreshed without the consent");
}
