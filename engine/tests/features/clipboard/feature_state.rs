//! The clipboard's feature-state sync and settings over the platform world — the feature-state sync
//! (its send rule and triggers), the refusals of the PC-initiated request wrapper, startup and
//! shutdown, the inputs (persistence, the `CLIPBOARD` capability) and the settings UI's view.

use crate::support::clipboard_world as clip;
use crate::support::platform_world as fakes;

use std::time::Duration;

use clip::*;
use fakes::{Behavior, WakeEnd, WakeScript, kvp_reply, phone_id, platform, settle};
use phonelink_engine::edge::LockState;
use phonelink_engine::features::StoreLocation;
use phonelink_engine::features::clipboard::{
    ClipboardEvent, ClipboardPhone, global_switch_persisted, with_clipboard_capability,
};
use phonelink_engine::platform::route;
use phonelink_engine::proto::clipboard::v1 as cb;
use phonelink_engine::proto::platform_sdk::v1 as pb;
use phonelink_engine::seams::PartnerSessionControl;
use phonelink_engine::types::{DcgSendStatus, PartnerDisconnectReason, SendOptions};

const ON: i32 = cb::ClipboardRequestType::FeatureOn as i32;
const OFF: i32 = cb::ClipboardRequestType::FeatureOff as i32;
const DISABLE: i32 = cb::ClipboardRequestType::FeatureDisable as i32;

#[tokio::test(start_paused = true)]
async fn startup_sends_the_feature_state_of_an_available_phone() {
    let w = ClipWorld::new().await;
    // The toggle defaults to off: FEATURE_OFF.
    let requests = w.clipboard_requests();
    assert_eq!(requests.len(), 1);
    let (drm, request) = &requests[0];
    assert_eq!(drm.resource_type, pb::ProtoDeviceResourceType::Unknown as i32, "path-addressed");
    assert_eq!(drm.request_type, pb::ProtoDeviceResourceRequestType::Get as i32);
    assert_eq!(drm.resource_path, "/clipboard");
    assert_eq!(drm.payload, vec![0x08, 0x03], "FEATURE_OFF, no correlation id");
    assert_eq!(request.request_type, OFF);

    let sent = w.w.pair.pc_link.sent_on(route::DEVICE_RESOURCE_MANAGER).pop().unwrap();
    assert_eq!(sent.message.headers.get("ms-content-type"), Some("application/x-binary"));
    assert_eq!(
        sent.options,
        SendOptions { ignore_device_disabled: true, skip_wake_if_not_connected: true, ..SendOptions::NONE }
    );
    assert!(w.up_to_date());
}

#[tokio::test(start_paused = true)]
async fn an_unavailable_phone_is_marked_and_gets_its_state_when_it_connects() {
    let w = ClipWorld::with(Setup { connected: false, ..Setup::default() }).await;
    assert!(w.clipboard_requests().is_empty(), "nothing is sent to an unavailable phone");
    assert!(!w.up_to_date(), "marked not up to date");

    w.w.connect_phone().await;
    settle().await;
    assert_eq!(w.request_types(), vec![OFF], "sent at the first moment the phone is available");
    assert!(w.up_to_date());

    // Up to date: availability changes send nothing more.
    w.w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::NoticeFromHub);
    settle().await;
    w.sessions.send_event(phonelink_engine::seams::SessionEvent::Connected(phone_id()));
    settle().await;
    assert_eq!(w.request_types(), vec![OFF]);
    assert!(w.up_to_date());
}

#[tokio::test(start_paused = true)]
async fn the_request_type_follows_the_switch_the_capability_and_the_toggle() {
    let mut w = ClipWorld::new().await;
    w.take_events();
    w.toggle_on().await;
    assert_eq!(w.request_types(), vec![OFF, ON]);
    assert_eq!(w.take_events(), vec![ClipboardEvent::PhoneChanged { phone: phone_id() }]);

    w.clipboard.set_global_switch(false).unwrap();
    settle().await;
    assert_eq!(w.request_types(), vec![OFF, ON, DISABLE]);
    assert_eq!(w.take_events(), vec![ClipboardEvent::GlobalSwitchChanged { enabled: false }]);
    w.clipboard.set_global_switch(false).unwrap();
    settle().await;
    assert_eq!(w.request_types().len(), 3, "no change, nothing sent");

    w.clipboard.set_global_switch(true).unwrap();
    settle().await;
    assert_eq!(w.request_types(), vec![OFF, ON, DISABLE, ON]);

    // The phone stops advertising CLIPBOARD.
    let mut device = phone_device();
    device.capabilities = Some(vec!["DEVICE_STATUS".into()]);
    w.peers.upsert(phone_peer(device));
    settle().await;
    assert_eq!(w.request_types(), vec![OFF, ON, DISABLE, ON, DISABLE]);

    w.clipboard.set_phone_toggle(&phone_id(), false).unwrap();
    settle().await;
    assert_eq!(w.request_types().last(), Some(&DISABLE), "without CLIPBOARD the state stays FEATURE_DISABLE");
}

#[tokio::test(start_paused = true)]
async fn every_capability_list_change_resends_and_nothing_else_does() {
    let w = ClipWorld::new().await;
    // Same list: no change.
    w.peers.upsert(phone_peer(phone_device()));
    settle().await;
    assert_eq!(w.request_types().len(), 1);

    // Only the app version: no capability change.
    let mut device = phone_device();
    device.client_version = Some("1.25000.1.0".into());
    w.peers.upsert(phone_peer(device));
    settle().await;
    assert_eq!(w.request_types().len(), 1);

    // Only the order changed: a change.
    let mut device = phone_device();
    device.capabilities = Some(vec!["CLIPBOARD".into(), "DEVICE_STATUS".into()]);
    w.peers.upsert(phone_peer(device));
    settle().await;
    assert_eq!(w.request_types(), vec![OFF, OFF]);

    // Capability case matters: `clipboard` is not `CLIPBOARD`.
    let mut device = phone_device();
    device.capabilities = Some(vec!["clipboard".into()]);
    w.peers.upsert(phone_peer(device));
    settle().await;
    assert_eq!(w.request_types(), vec![OFF, OFF, DISABLE]);
}

#[tokio::test(start_paused = true)]
async fn a_missing_and_an_empty_capability_list_are_the_same_list() {
    let mut device = phone_device();
    device.capabilities = None;
    let w = ClipWorld::with(Setup { device: device.clone(), ..Setup::default() }).await;
    assert_eq!(w.request_types(), vec![DISABLE], "no list: no CLIPBOARD");

    // A refresh reports `capabilities: []`, the next one omits the list again: no change either time.
    device.capabilities = Some(Vec::new());
    w.peers.upsert(phone_peer(device.clone()));
    settle().await;
    device.capabilities = None;
    w.peers.upsert(phone_peer(device.clone()));
    settle().await;
    assert_eq!(w.request_types(), vec![DISABLE]);

    // A real change is still one.
    device.capabilities = Some(vec!["CLIPBOARD".into()]);
    w.peers.upsert(phone_peer(device));
    settle().await;
    assert_eq!(w.request_types(), vec![DISABLE, OFF]);
}

#[tokio::test(start_paused = true)]
async fn a_disabled_phone_gets_nothing_until_it_is_enabled_again() {
    let w = ClipWorld::new().await;
    w.switches.set_enabled(&phone_id(), false).unwrap();
    settle().await;

    w.clipboard.set_phone_toggle(&phone_id(), true).unwrap();
    w.clipboard.set_global_switch(false).unwrap();
    settle().await;
    assert_eq!(w.request_types(), vec![OFF], "nothing is sent to a disabled phone");
    assert!(w.up_to_date(), "and its flag is unchanged");

    w.switches.set_enabled(&phone_id(), true).unwrap();
    settle().await;
    assert_eq!(w.request_types(), vec![OFF, DISABLE], "sent once it is enabled again");
}

#[tokio::test(start_paused = true)]
async fn an_enabled_change_while_not_up_to_date_sends_for_both_triggers() {
    let w = ClipWorld::new().await;
    // The feature sees the phone unavailable: the toggle change marks it not up to date.
    w.sessions.override_available(Some(false));
    w.toggle_on().await;
    assert!(!w.up_to_date());

    w.switches.set_enabled(&phone_id(), false).unwrap();
    settle().await;
    assert_eq!(w.request_types(), vec![OFF]);

    w.sessions.override_available(None);
    w.switches.set_enabled(&phone_id(), true).unwrap();
    settle().await;
    assert_eq!(
        w.request_types(),
        vec![OFF, ON, ON],
        "the enabled-flag trigger and the not-up-to-date trigger each send"
    );
    assert!(w.up_to_date());
}

#[tokio::test(start_paused = true)]
async fn the_forced_session_start_follows_the_trigger() {
    // The feature sees the phone available while its session is down, so the request wrapper starts
    // one: `NewSession` for a forced start, `Default` otherwise.
    let w = ClipWorld::with(Setup { connected: false, ..Setup::default() }).await;
    w.sessions.override_available(Some(true));
    w.w.waker.script(WakeScript::now(WakeEnd::Succeed));

    w.toggle_on().await;
    assert_eq!(w.request_types().first(), Some(&ON));
    assert!(w.w.waker.options()[0].request_new_session, "a toggle change forces a session start");

    let w = ClipWorld::with(Setup { connected: false, ..Setup::default() }).await;
    w.sessions.override_available(Some(true));
    w.w.waker.script(WakeScript::now(WakeEnd::Succeed));

    let mut device = phone_device();
    device.capabilities = Some(vec!["CLIPBOARD".into()]);
    w.peers.upsert(phone_peer(device));
    settle().await;

    assert_eq!(w.request_types().first(), Some(&OFF));
    assert!(!w.w.waker.options()[0].request_new_session, "a capability change does not");
}

/// The feature sees the phone available while its session is down, so the request wrapper starts a
/// session and the wake shows the start policy: `NewSession` (`request_new_session`) for a forced
/// start, `Default` otherwise. (Toggle: forced; capability change: not forced — the test above.)
#[tokio::test(start_paused = true)]
async fn the_forced_session_start_of_the_other_triggers() {
    let forced = |w: &ClipWorld| w.w.waker.options().iter().map(|o| o.request_new_session).collect::<Vec<_>>();

    // Startup: forced.
    let w = ClipWorld::with(Setup {
        connected: false,
        available: Some(true),
        wake: Some(WakeScript::now(WakeEnd::Succeed)),
        ..Setup::default()
    })
    .await;
    assert_eq!(w.request_types(), vec![OFF]);
    assert_eq!(forced(&w), vec![true], "startup forces a session start");

    // A phone newly linked: forced.
    let w = ClipWorld::with(Setup { connected: false, available: Some(true), linked: false, ..Setup::default() }).await;
    assert!(w.request_types().is_empty());

    w.w.waker.script(WakeScript::now(WakeEnd::Succeed));
    w.peers.upsert(phone_peer(phone_device()));
    settle().await;
    assert_eq!(w.request_types(), vec![OFF]);
    assert_eq!(forced(&w), vec![true], "a newly linked phone forces a session start");

    // The global switch: not forced. (At startup the feature saw the phone unavailable, so the
    // phone's arrival may also resend — the not-up-to-date trigger — once its session is up.)
    let w = ClipWorld::with(Setup { connected: false, ..Setup::default() }).await;
    assert!(w.request_types().is_empty());

    w.sessions.override_available(Some(true));
    w.w.waker.script(WakeScript::now(WakeEnd::Succeed));
    w.clipboard.set_global_switch(false).unwrap();
    settle().await;
    assert_eq!(w.request_types().first(), Some(&DISABLE));
    assert_eq!(forced(&w), vec![false], "a global-switch change does not force one");

    // The enabled flag: not forced — neither its own send nor the not-up-to-date one.
    let w = ClipWorld::with(Setup { connected: false, ..Setup::default() }).await;
    w.switches.set_enabled(&phone_id(), false).unwrap();
    settle().await;

    w.sessions.override_available(Some(true));
    w.w.waker.set_default(WakeScript::now(WakeEnd::Succeed));
    w.switches.set_enabled(&phone_id(), true).unwrap();
    settle().await;
    let sent = w.request_types();
    assert!(sent.len() >= 2 && sent.iter().all(|t| *t == OFF), "both triggers sent: {sent:?}");
    assert!(!forced(&w).is_empty() && forced(&w).iter().all(|f| !f), "{:?}", forced(&w));

    // Availability while not up to date: not forced.
    let w = ClipWorld::with(Setup { connected: false, ..Setup::default() }).await;
    assert!(!w.up_to_date());

    w.sessions.override_available(Some(true));
    w.w.waker.script(WakeScript::now(WakeEnd::Succeed));
    w.sessions.send_event(phonelink_engine::seams::SessionEvent::Connected(phone_id()));
    settle().await;
    assert_eq!(w.request_types(), vec![OFF]);
    assert_eq!(forced(&w), vec![false], "the not-up-to-date resend does not force one");
}

#[tokio::test(start_paused = true)]
async fn failures_leave_the_flag_and_are_not_retried() {
    let w = ClipWorld::with(Setup { connected: false, drm: PhoneDrm::Silent, ..Setup::default() }).await;
    assert!(!w.up_to_date());

    w.w.connect_phone().await;
    tokio::time::sleep(Duration::from_secs(300)).await;
    assert_eq!(w.request_types(), vec![OFF], "one request, no retry after the 120 s timeout");
    assert!(!w.up_to_date());

    // A rejection and a RESOURCE_HANDLER_NOT_REGISTERED answer are failures too.
    w.w.phone_drm.set_reply(Box::new(|_| {
        Some(platform(&[("_rejectedReason", "NotSubscribedToRoute"), ("ms-content-type", "text/json")], b"{}"))
    }));
    w.toggle_on().await;
    assert_eq!(w.request_types(), vec![OFF, ON]);
    assert!(!w.up_to_date());

    w.w.phone_drm.set_reply(Box::new(|_| {
        let body = pb::DeviceResourceManagerResponseMessage { payload: Vec::new(), response_type: Some(2) };
        Some(platform(&[("ms-content-type", "application/x-binary")], &prost::Message::encode_to_vec(&body)))
    }));
    w.clipboard.set_phone_toggle(&phone_id(), false).unwrap();
    settle().await;
    assert_eq!(w.request_types(), vec![OFF, ON, OFF]);
    assert!(!w.up_to_date());

    // A failed send.
    phone_drm(&w.w, PhoneDrm::Answer(None));
    w.w.pair.pc_link.script(Behavior::Fail(DcgSendStatus::TimedOut));
    w.toggle_on().await;
    assert!(!w.up_to_date());

    // The next trigger succeeds.
    w.clipboard.set_phone_toggle(&phone_id(), false).unwrap();
    settle().await;
    assert!(w.up_to_date());
}

#[tokio::test(start_paused = true)]
async fn the_request_wrapper_refusals_send_nothing() {
    let w = ClipWorld::new().await;
    w.w.lock.set(LockState::Locked);
    w.toggle_on().await;
    w.w.lock.set(LockState::Unknown);
    w.clipboard.set_phone_toggle(&phone_id(), false).unwrap();
    settle().await;
    assert_eq!(w.request_types(), vec![OFF], "a locked PC (or an unknown lock state) sends nothing");
    w.w.lock.set(LockState::Unlocked);

    let mut device = phone_device();
    device.client_version = Some("1.23112.188.9".into());
    device.capabilities = Some(vec!["CLIPBOARD".into()]);
    w.peers.upsert(phone_peer(device));
    settle().await;
    assert_eq!(w.request_types(), vec![OFF], "an old phone app gets nothing");
    assert!(w.up_to_date(), "refusals leave the flag unchanged");
}

#[tokio::test(start_paused = true)]
async fn phones_join_and_leave_the_account() {
    let w = ClipWorld::new().await;
    w.peers.remove(&phone_id());
    settle().await;

    w.clipboard.set_phone_toggle(&phone_id(), true).unwrap();
    settle().await;
    assert_eq!(w.request_types(), vec![OFF], "no manager, nothing sent");
    assert_eq!(w.clipboard.phones().unwrap(), Vec::<ClipboardPhone>::new());

    // Linked again: a new manager sends with a forced session start.
    w.peers.upsert(phone_peer(phone_device()));
    settle().await;
    assert_eq!(w.request_types(), vec![OFF, ON]);

    // A peer that is not an Android phone has no manager.
    let mut pc = phone_device();
    pc.id = "pc-99".into();
    pc.os_name = Some("Windows".into());
    w.peers.upsert(phonelink_engine::auth::PeerDevice {
        client: phonelink_engine::types::DcgClient::new("pc-99", phonelink_engine::types::Environment::Prod),
        info: pc,
    });
    settle().await;
    assert_eq!(w.request_types(), vec![OFF, ON]);
    assert_eq!(w.clipboard.phones().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn the_global_switch_drives_the_capability_and_persists() {
    let dir = tempfile::tempdir().unwrap();
    let stores = StoreLocation::Directory(dir.path().to_path_buf());
    let base = vec!["DEVICE_STATUS".to_owned(), "UNLINKABLE_V1".to_owned()];
    assert!(global_switch_persisted(&stores).unwrap(), "default on before anything is stored");

    {
        let mut w = ClipWorld::with(Setup { stores: stores.clone(), ..Setup::default() }).await;
        assert_eq!(
            w.clipboard.advertised_capabilities(&base).unwrap(),
            vec!["CLIPBOARD", "DEVICE_STATUS", "UNLINKABLE_V1"]
        );

        w.take_events();
        w.toggle_on().await;
        w.clipboard.set_global_switch(false).unwrap();
        settle().await;
        assert!(w.take_events().contains(&ClipboardEvent::GlobalSwitchChanged { enabled: false }));
        assert_eq!(w.clipboard.advertised_capabilities(&base).unwrap(), base);

        // Not up to date when it goes away.
        w.sessions.override_available(Some(false));
        w.clipboard.set_phone_toggle(&phone_id(), false).unwrap();
        w.clipboard.set_phone_toggle(&phone_id(), true).unwrap();
        settle().await;
        assert!(!w.up_to_date());
        w.clipboard.shutdown();
    }

    // Readable by the engine before provisioning, without installing the feature.
    assert!(!global_switch_persisted(&stores).unwrap());
    assert_eq!(with_clipboard_capability(&base, global_switch_persisted(&stores).unwrap()), base);

    let w = ClipWorld::with(Setup { stores, ..Setup::default() }).await;
    assert!(!w.clipboard.global_switch().unwrap());
    assert!(w.clipboard.phone_toggle(&phone_id()).unwrap(), "the toggle survived the restart");
    assert_eq!(w.request_types(), vec![DISABLE], "startup states the persisted switch");
    assert!(w.up_to_date());
}

#[tokio::test(start_paused = true)]
async fn the_settings_view() {
    let w = ClipWorld::new().await;
    let view = w.clipboard.phone(&phone_id()).unwrap().unwrap();
    assert_eq!(
        view,
        ClipboardPhone {
            phone: phone_id(),
            name: Some("Pixel".into()),
            toggle: false,
            applicable: true,
            capable: true,
            enabled: true,
            available: true,
            supported: false,
            feature_status_up_to_date: true,
        }
    );

    w.toggle_on().await;
    assert!(w.clipboard.phone(&phone_id()).unwrap().unwrap().supported);

    w.clipboard.set_global_switch(false).unwrap();
    let view = w.clipboard.phone(&phone_id()).unwrap().unwrap();
    assert!(!view.applicable, "not applicable while the global switch is off");
    assert!(view.supported, "the global switch is not part of a phone's support");
    assert_eq!(w.clipboard.phones().unwrap(), vec![view]);
    let unknown = phonelink_engine::types::DcgClient::new("nobody", phonelink_engine::types::Environment::Prod);
    assert_eq!(w.clipboard.phone(&unknown).unwrap(), None);
}

#[tokio::test(start_paused = true)]
async fn shutdown_stops_everything_and_sends_nothing() {
    let w = ClipWorld::new().await;
    w.toggle_on().await;
    let sent_before = w.w.pair.pc_link.sent().len();
    w.clipboard.shutdown();
    settle().await;

    w.clipboard.set_phone_toggle(&phone_id(), false).unwrap();
    w.clipboard.set_global_switch(false).unwrap();
    w.switches.set_enabled(&phone_id(), false).unwrap();
    w.switches.set_enabled(&phone_id(), true).unwrap();
    w.pasteboard.copy_text("after shutdown");
    after_copy().await;
    phone_announces(&w.w, "m-1", CID).await;
    let ask_status = ask(&w.w, phone_get(&clipboard_request(cb::ClipboardRequestType::Status as i32, CID))).await;
    assert_eq!(ask_status.status, phonelink_engine::seams::PlatformRequestStatus::ResponseTimedOut, "no answer");

    let sent: Vec<_> =
        w.w.pair.pc_link.sent()[sent_before..].iter().filter_map(|m| m.message.route().map(str::to_owned)).collect();
    // Only the platform layer's own `{}` acknowledgement of the publication.
    assert_eq!(sent, vec![route::INTERNAL_RESPONSE.to_owned()]);
    assert_eq!(w.w.pair.pc_link.sent().last().unwrap().message.payload, kvp_reply("{}").payload);
}

#[tokio::test(start_paused = true)]
async fn shutdown_abandons_a_feature_state_request_waiting_for_its_session() {
    // At startup the feature saw the phone unavailable: nothing sent.
    let w = ClipWorld::with(Setup { connected: false, ..Setup::default() }).await;
    w.sessions.override_available(Some(true));
    w.w.waker.script(WakeScript { delay: Duration::from_secs(5), end: WakeEnd::Succeed });
    w.clipboard.set_phone_toggle(&phone_id(), true).unwrap();

    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(w.w.waker.calls().len(), 1, "the request waits for the session it started");

    w.clipboard.shutdown();

    // The wake succeeds and the phone comes up: the abandoned request stays unsent.
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert!(w.w.sessions.is_connected(&phone_id()));
    assert!(w.clipboard_requests().is_empty(), "nothing is sent after shutdown");
}

#[tokio::test(start_paused = true)]
async fn triggers_still_queued_at_shutdown_send_nothing() {
    let w = ClipWorld::new().await;
    assert_eq!(w.request_types(), vec![OFF]);

    // A toggle change, a capability change and an availability change are queued …
    w.clipboard.set_phone_toggle(&phone_id(), true).unwrap();
    let mut device = phone_device();
    device.capabilities = Some(vec!["CLIPBOARD".into()]);
    w.peers.upsert(phone_peer(device));
    w.sessions.send_event(phonelink_engine::seams::SessionEvent::Connected(phone_id()));

    // … when the feature stops.
    w.clipboard.shutdown();
    tokio::time::sleep(Duration::from_secs(300)).await;
    assert_eq!(w.request_types(), vec![OFF]);
}

#[tokio::test(start_paused = true)]
async fn shutdown_is_idempotent_and_callable_from_a_task() {
    let w = ClipWorld::new().await;
    w.toggle_on().await;
    let sent_before = w.w.pair.pc_link.sent().len();

    let clipboard = w.clipboard.clone();
    tokio::spawn(async move {
        clipboard.shutdown();
        clipboard.shutdown();
    })
    .await
    .expect("shutdown from a task");

    w.clipboard.shutdown();
    w.clipboard.set_global_switch(false).unwrap();
    w.clipboard.set_phone_toggle(&phone_id(), false).unwrap();
    w.pasteboard.copy_text("after shutdown");
    after_copy().await;
    assert_eq!(w.w.pair.pc_link.sent().len(), sent_before, "nothing sent");
}

#[tokio::test(start_paused = true)]
async fn install_preconditions_fail_loudly() {
    let not_enrolled = ClipWorld::try_with(Setup { enrolled: false, ..Setup::default() }).await;
    assert!(matches!(not_enrolled, Err(phonelink_engine::features::FeatureError::Programming(_))));

    let config =
        phonelink_engine::config::features::ClipboardConfig { poll_interval: Duration::ZERO, ..Default::default() };
    let zero_poll = ClipWorld::try_with(Setup { config, ..Setup::default() }).await;
    assert!(matches!(zero_poll, Err(phonelink_engine::features::FeatureError::Config(_))));

    // One `/clipboard` receiver per path: a second install is refused.
    let w = ClipWorld::new().await;
    assert_eq!(w.request_types(), vec![OFF], "the first install's feature state");

    let again = phonelink_engine::features::clipboard::Clipboard::install(
        &w.env,
        Default::default(),
        w.w.routes.msaep.clone(),
        w.w.routes.drm.clone(),
        w.pasteboard.clone(),
        w.codec.clone(),
    );
    assert!(matches!(again, Err(phonelink_engine::features::FeatureError::Registration(_))));

    // The refused install started its parts in the startup order — managers, subscription, watcher — before
    // the receiver failed, and left nothing behind: no feature state of its own, no tag-9 subscriber, no
    // pasteboard watcher.
    settle().await;
    assert_eq!(w.request_types(), vec![OFF], "the refused install sent no feature state");
    w.toggle_on().await;
    phone_announces(&w.w, "m-1", CID).await;
    let content = cb::ClipboardRequestType::Content as i32;
    assert_eq!(w.request_types(), vec![OFF, ON, content], "one pull: the refused install's subscriber is gone");
    w.pasteboard.copy_text("copied on the Mac");
    after_copy().await;
    assert_eq!(w.announcements().len(), 1, "one announcement: its watcher is gone");
}
