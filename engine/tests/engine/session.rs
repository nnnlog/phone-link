//! The engine while a phone session runs, end to end over the fakes (`support/engine_world`):
//! every device-list refresh rewrites the phone record; the account gate follows the MSA token and
//! the selected phone's link; the removal of the account stops the features and needs a rebuild,
//! which the `uniffi` `Engine`'s next start performs, keeping the per-process identifiers; the app's
//! foreground state reaches the wire and its activation — reported before, during or after the
//! start — sends an immediate connect, as a refresh does; every content type without a processor is
//! answered `2` / `1` with every feature installed.

use crate::support::auth_world;
use crate::support::engine_world as world;
use crate::support::scripted_phone;
use crate::support::session_log;

use std::time::Duration;

use auth_world::fake_dcg::error_body;
use auth_world::harness::eventually;
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::events::EngineEvent;
use phonelink_engine::app::{AppOutcome, AppRequest, LocalGate, RequestOptions};
use phonelink_engine::auth::{AuthError, FetchMode};
use phonelink_engine::codec::valueset::ValueSet;
use phonelink_engine::edge::SecureStore;
use tokio::sync::broadcast::error::TryRecvError;
use world::{World, default_connect_reply};

/// The phone record from the device list (Android): every application of the device list rewrites
/// the record — a refresh whose entry did not change too — and the device list and the connect
/// reply write the same fields, the last writer winning (country code, `statusCapabilities`,
/// `yppVersion`, the OS version). No peer event is raised for an unchanged entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_device_list_refresh_rewrites_the_phone_record() {
    let w = World::new(Some(r#"{"yppCapabilities":127,"defaultLocale":"en-GB","statusCapabilities":"1023"}"#)).await;
    *w.connect_reply.lock().unwrap() = default_connect_reply()
        .with("yppVersion", 9i32)
        .with("attributes", ValueSet::new().with("countryCode", "US").with("osVersion", "15.1"))
        .with("capabilities", ValueSet::new().with("statusCapabilities", 0x1FFFi32));

    let engine = w.engine().await;
    let record = || engine.app.phone_record().expect("the phone is selected");

    let r = record();
    assert_eq!(
        (r.country_code.as_deref(), r.status_capabilities, r.ypp_version, r.android_major_version()),
        (Some("en-GB"), 0x3FF, 8, Some(14)),
        "the device list at start"
    );

    let mut peer_events = engine.auth.subscribe_peer_events();
    w.connect_phone(|| engine.phone_connected()).await;
    assert!(
        eventually(Duration::from_secs(20), || {
            let r = record();
            r.country_code.as_deref() == Some("US")
                && r.status_capabilities == 0x1FFF
                && r.ypp_version == 9
                && r.android_major_version() == Some(15)
        })
        .await,
        "the connect reply wrote the record: {:?}",
        record()
    );

    // A device-list refresh whose entry is unchanged.
    engine.auth.sync_trust(FetchMode::ForceCloud).await.expect("cloud refresh");
    assert!(
        eventually(Duration::from_secs(10), || {
            let r = record();
            r.country_code.as_deref() == Some("en-GB")
                && r.status_capabilities == 0x3FF
                && r.ypp_version == 8
                && r.android_major_version() == Some(14)
        })
        .await,
        "the refresh rewrote the record from the device list: {:?}",
        record()
    );
    assert!(matches!(peer_events.try_recv(), Err(TryRecvError::Empty)), "no peer event for an unchanged entry");
}

/// The account gate before an App request is sent stops a request while the account's token cannot
/// be refreshed (outcome canceled, nothing sent, the "all features disabled" flag cleared); a later
/// successful grant opens it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_account_gate_stops_requests_while_the_token_cannot_be_refreshed() {
    let w = World::new(None).await;
    let engine = w.engine().await;
    let request = || {
        AppRequest::legacy("/gate", ValueSet::new())
            .with_options(RequestOptions::user_initiated().with_timeout(Duration::from_secs(1)))
    };
    assert!(!engine.auth.needs_user_attention());

    // The cached MSA token expired and its refresh needs the user.
    w.h.store.delete("auth/live/user".into()).unwrap();
    w.h.web.queue(Err(phonelink_engine::edge::WebSignInError::Failed { reason: "no view".into() }));
    w.h.advance(Duration::from_secs(2 * 3600));
    assert!(engine.auth.sync_trust(FetchMode::ForceCloud).await.is_err(), "the MSA token cannot be refreshed");
    assert!(engine.auth.needs_user_attention());

    engine.app.update_phone_record(|r| r.all_features_disabled = true);
    let response = engine.app.request(request()).await.unwrap();
    assert_eq!((response.outcome, response.local_gate), (AppOutcome::Canceled, Some(LocalGate::AccountNotReady)));
    assert!(!engine.app.phone_record().unwrap().all_features_disabled, "the account gate clears the flag");

    // A later grant succeeds: the gate opens (the request then waits for the phone, which never
    // comes up here).
    w.h.seed_session();
    engine.auth.sync_trust(FetchMode::ForceCloud).await.expect("cloud refresh");
    assert!(!engine.auth.needs_user_attention());
    let response = engine.app.request(request()).await.unwrap();
    assert_eq!(response.local_gate, None, "{:?}", response.outcome);
}

/// The clipboard on account removal or shutdown stops all four parts and sends nothing — the
/// account is removed at runtime (the device was unlinked, every account deprovisioned), then a
/// trigger fires: no `/clipboard` request reaches the still-connected phone, and the core needs a
/// rebuild.
///
/// It runs on one thread, so the captured log holds every line of this engine: the stop lines of the
/// clipboard's loop and of the capability bridge say that nothing is left to send.
#[tokio::test]
async fn after_account_removal_the_clipboard_sends_nothing_and_the_core_needs_a_rebuild() {
    let log = session_log::capture_thread();
    let w = World::new(None).await;
    let engine = w.engine().await;
    w.connect_phone(|| engine.phone_connected()).await;
    let features = engine.features().expect("installed");

    // The feature state reaches the phone at startup, and a trigger sends it again.
    assert!(
        eventually(Duration::from_secs(20), || !w.drm_requests_for("/clipboard").is_empty()).await,
        "the startup feature state"
    );

    let sent = w.drm_requests_for("/clipboard").len();
    features.clipboard.set_phone_toggle(&w.phone(), true).unwrap();
    assert!(
        eventually(Duration::from_secs(20), || w.drm_requests_for("/clipboard").len() > sent).await,
        "a toggle change sends the feature state"
    );

    // The device is unlinked: the next device-list fetch answers 403 + 13.
    w.h.dcg.script("GetDeviceInfoList", 403, &error_body(Some(13), "UnlinkedDevice"));
    assert!(matches!(engine.auth.sync_trust(FetchMode::ForceCloud).await, Err(AuthError::UnlinkedDevice)));
    assert!(eventually(Duration::from_secs(10), || engine.needs_rebuild()).await, "the core needs a rebuild");
    assert!(
        eventually(Duration::from_secs(10), || log.logged("feature-state loop stopped")).await,
        "the removal stops the clipboard's feature-state loop"
    );

    let sent = w.drm_requests_for("/clipboard").len();
    let set_device_info = w.h.dcg.count("SetDeviceInfo");
    features.clipboard.set_phone_toggle(&w.phone(), false).unwrap();
    // A global-switch change would re-advertise CLIPBOARD (SetDeviceInfo) and re-send every phone's
    // feature state: after the removal, neither happens.
    features.clipboard.set_global_switch(false).unwrap();
    assert!(
        eventually(Duration::from_secs(10), || log.logged("capability bridge stopped; the account was removed")).await,
        "the global switch's change finds the capability bridge stopping"
    );

    assert_eq!(w.drm_requests_for("/clipboard").len(), sent, "after account removal the clipboard sends nothing");
    assert_eq!(w.h.dcg.count("SetDeviceInfo"), set_device_info, "nor does the capability bridge");
    assert!(w.h.events.take().iter().any(|e| matches!(e, EngineEvent::AccountRemoved)), "the UI hears of the removal");

    let again = engine.start(None).await.expect_err("a core whose account was removed does not start again");
    assert!(again.to_string().contains("rebuilt"), "{again}");
}

/// The `uniffi` `Engine`'s next start replaces a core whose account was removed: it stops the old core
/// and starts a new one, which provisions a new identity.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_next_start_rebuilds_a_core_whose_account_was_removed() {
    let w = World::new(None).await;
    let engine = Engine::with_config(w.config(), w.h.edges(), World::feature_edges(), w.h.events.clone());
    let first = engine.start().await.expect("the first start");
    let old = engine.current_core().expect("a core");

    // The device is unlinked at runtime: the next device-list fetch answers 403 + 13.
    w.h.dcg.script("GetDeviceInfoList", 403, &error_body(Some(13), "UnlinkedDevice"));
    assert!(matches!(old.auth.sync_trust(FetchMode::ForceCloud).await, Err(AuthError::UnlinkedDevice)));
    assert!(eventually(Duration::from_secs(10), || old.needs_rebuild()).await);

    let second = engine.start().await.expect("the next start builds a new core");
    let new = engine.current_core().expect("a core");
    assert!(!std::sync::Arc::ptr_eq(&old, &new), "a new core");
    assert!(!new.needs_rebuild());
    assert!(new.features().is_some(), "the new core installed the features");
    assert_ne!(second.own_device_id, first.own_device_id, "a new identity was provisioned");

    // The per-process identifiers stay: `DCG-SessionId` and the app session id, and the fragment
    // layer's `MessageId` counter with each target's `SessionId` and `SequenceNumber`.
    assert_eq!(new.ident.session_id, old.ident.session_id);
    assert_eq!(new.app.app_session_id(), old.app.app_session_id());
    assert!(std::sync::Arc::ptr_eq(new.dcg.identifiers(), old.dcg.identifiers()));
    assert_eq!(new.dcg.session_id(&w.phone()), old.dcg.session_id(&w.phone()));
}

/// The connect trigger "app came to the foreground": a coming to the foreground reported while the
/// engine is starting — here while the start fetches the device list, before the phone is selected
/// — still sends the immediate activation connect, once the start has registered every provider.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_foreground_report_during_the_start_sends_the_activation_connect() {
    session_log::install();
    let w = World::new(None).await;
    let engine = Engine::with_config(w.config(), w.h.edges(), World::feature_edges(), w.h.events.clone());
    let reporter = engine.clone();
    w.h.dcg.on_request("GetDeviceInfoList", move |_| reporter.set_foreground(true));

    engine.start().await.expect("start");
    w.connect_phone(|| engine.status().phone_connected).await;

    let connects = |trigger: &str| {
        w.requests_on("/connect/v1")
            .into_iter()
            .filter(|r| r.trace_state("ypTriggerId").as_deref() == Some(trigger))
            .collect::<Vec<_>>()
    };

    assert!(
        eventually(Duration::from_secs(30), || !connects("AppActivated").is_empty()).await,
        "the activation connect"
    );
    assert_eq!(connects("AppActivated")[0].trace_state("isuserinitiated").as_deref(), Some("True"));
    assert!(eventually(Duration::from_secs(30), || !connects("EngineStart").is_empty()).await, "the app-start connect");

    // Every activation connect the start and the report triggered has ended.
    let phone = w.phone();
    let mut activations = None;
    assert!(
        eventually(Duration::from_secs(30), || {
            activations = session_log::connects_ended(&phone, "AppActivated");
            activations.is_some()
        })
        .await,
        "{:?}",
        session_log::connects(&phone)
    );
    assert_eq!(activations, Some(1), "exactly one triggered");
    assert_eq!(connects("AppActivated").len(), 1, "exactly one");
}

/// The account gate before an App request is sent: once the selected phone leaves the account's
/// linked devices, App requests fail at the account gate (canceled, nothing sent, the "all features
/// disabled" flag cleared) — also after the phone is listed again — until a start selects a phone
/// again; the UI hears `PhoneUnlinked` once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn after_the_phone_is_unlinked_requests_fail_at_the_account_gate_until_a_start() {
    let w = World::new(None).await;
    let engine = w.engine().await;
    let request = || {
        AppRequest::legacy("/gate", ValueSet::new())
            .with_options(RequestOptions::user_initiated().with_timeout(Duration::from_secs(1)))
    };
    let gated = |response: &phonelink_engine::app::AppResponse| response.local_gate == Some(LocalGate::AccountNotReady);
    assert!(!gated(&engine.app.request(request()).await.unwrap()));
    w.h.events.take();

    w.h.dcg.peer_mut(&w.phone_id, |p| p.is_linked = Some(false));
    engine.auth.sync_trust(FetchMode::ForceCloud).await.expect("cloud refresh");
    assert!(eventually(Duration::from_secs(10), || engine.phone_unlinked()).await, "the phone left the account");

    engine.app.update_phone_record(|r| r.all_features_disabled = true);
    let response = engine.app.request(request()).await.unwrap();
    assert_eq!((response.outcome, response.local_gate), (AppOutcome::Canceled, Some(LocalGate::AccountNotReady)));
    assert!(!engine.app.phone_record().unwrap().all_features_disabled, "the account gate clears the flag");

    // Listed again: the gate stays closed until a start selects the phone.
    w.h.dcg.peer_mut(&w.phone_id, |p| p.is_linked = Some(true));
    engine.auth.sync_trust(FetchMode::ForceCloud).await.expect("cloud refresh");
    assert!(gated(&engine.app.request(request()).await.unwrap()));
    let unlinked = w.h.events.take().iter().filter(|e| matches!(e, EngineEvent::PhoneUnlinked)).count();
    assert_eq!(unlinked, 1);

    engine.start(None).await.expect("the next start selects the phone");
    assert!(!engine.phone_unlinked());
    assert!(!gated(&engine.app.request(request()).await.unwrap()), "the gate opened again");
}

/// The connect triggers ("app came to the foreground" → app activation; a user refresh) and the
/// trace context's `isForeground`, through the `uniffi` `Engine`: the foreground state reported
/// before the start applies once it starts, coming to the foreground sends an immediate connect,
/// `refresh` sends one too, and `isForeground` follows the app on the wire.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_foreground_state_reaches_the_wire_and_activation_and_refresh_send_immediate_connects() {
    session_log::install();
    let w = World::new(None).await;
    let engine = Engine::with_config(w.config(), w.h.edges(), World::feature_edges(), w.h.events.clone());
    engine.set_foreground(true);
    engine.start().await.expect("start");
    w.connect_phone(|| engine.status().phone_connected).await;

    let connects = |trigger: &str| {
        w.requests_on("/connect/v1")
            .into_iter()
            .filter(|r| r.trace_state("ypTriggerId").as_deref() == Some(trigger))
            .collect::<Vec<_>>()
    };

    assert!(
        eventually(Duration::from_secs(30), || !connects("AppActivated").is_empty()).await,
        "the activation connect"
    );
    let activated = &connects("AppActivated")[0];
    assert_eq!(activated.trace_state("isuserinitiated").as_deref(), Some("True"), "an immediate connect");
    assert_eq!(activated.trace_state("isForeground").as_deref(), Some("True"));
    assert!(eventually(Duration::from_secs(30), || !connects("EngineStart").is_empty()).await, "the app-start connect");
    assert_eq!(connects("EngineStart")[0].trace_state("isuserinitiated").as_deref(), Some("False"));

    // Leaving the foreground sends nothing; a refresh is an immediate connect, now in the background.
    engine.set_foreground(false);
    engine.refresh().expect("refresh");
    assert!(eventually(Duration::from_secs(30), || !connects("Refresh").is_empty()).await, "the refresh connect");
    let refresh = &connects("Refresh")[0];
    assert_eq!(refresh.trace_state("isuserinitiated").as_deref(), Some("True"));
    assert_eq!(refresh.trace_state("isForeground").as_deref(), Some("False"));
    assert_eq!(connects("AppActivated").len(), 1);

    // Coming back sends one more activation connect; reporting the same state again sends nothing.
    engine.set_foreground(true);
    engine.set_foreground(true);

    let phone = w.phone();
    let mut activations = None;
    assert!(
        eventually(Duration::from_secs(30), || {
            activations = session_log::connects_ended(&phone, "AppActivated");
            activations.is_some()
        })
        .await,
        "{:?}",
        session_log::connects(&phone)
    );
    assert_eq!(activations, Some(2), "the second report triggered nothing");
    assert_eq!(connects("AppActivated").len(), 2);
    assert_eq!(connects("AppActivated")[1].trace_state("isForeground").as_deref(), Some("True"));
}

/// Content types and the engine's rule for them: with every feature installed, the processors are the
/// four shared App types and the features' own types; every other type — the out-of-scope ones
/// Phone Link on Windows processes, the iOS-only ones, `messages` and `messageAck` (differs from
/// Phone Link on Windows), the unknown strings classified as `test` — is answered `2` in the metadata
/// phase and `1` in the content phase of inbound dispatch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn content_types_without_a_processor_are_answered_2_and_1_with_every_feature_installed() {
    let w = World::new(None).await;
    let engine = w.engine().await;
    w.connect_phone(|| engine.phone_connected()).await;

    let types = [
        "test",
        "photos",
        "messages",
        "screenMirror",
        "contacts",
        "messageAck",
        "messagesMms",
        "callLogs",
        "thumbnails",
        "sharedContent",
        "copypaste_metadata",
        "copypaste_payload",
        "dragdrop_metadata",
        "sharedContentV2",
        "audioCommand",
        "recentApps",
        "continuity",
        "contactsVCard",
        "callHistoryPbap",
        "continuityAppContext",
        "fileTransfer",
        "connect",
        "Permissions",
        "noSuchType",
    ];

    for (i, t) in types.iter().enumerate() {
        let base = ValueSet::new().with("contentType", *t).with("contractVersion", 3.99f64);
        w.send(&scripted_phone::app_request(
            "/legacy/phonecontent",
            &format!("m-{i}"),
            &base.clone().with("syncID", format!("sm-{i}")).with("metadata", true),
        ));
        w.send(&scripted_phone::app_request(
            "/legacy/phonecontent",
            &format!("c-{i}"),
            &base.with("syncID", format!("sc-{i}")),
        ));
    }

    assert!(
        eventually(Duration::from_secs(30), || (0..types.len())
            .all(|i| w.response_to(&format!("m-{i}")).is_some() && w.response_to(&format!("c-{i}")).is_some()))
        .await,
        "every message was answered"
    );

    for (i, t) in types.iter().enumerate() {
        let result = |id: String| World::values(&w.response_to(&id).unwrap()).i32("result");
        assert_eq!(result(format!("m-{i}")), Some(2), "{t}: metadata phase");
        assert_eq!(result(format!("c-{i}")), Some(1), "{t}: content phase");
    }
}
