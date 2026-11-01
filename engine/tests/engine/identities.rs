//! The engine's two PC identities, end to end over the fakes (`support/engine_world`): Phone
//! Link's `PL` identity and the Cross Device Experience Host's `WEA` identity enroll into one
//! account, each with its own keys, capabilities and device, while the machine's logical device id is
//! shared; only the CDEH identity opens a hub connection, which carries the App layer too, and its
//! session follows the Mac's lock, the per-phone switch and the phone's Link to Windows version.
//!
//! A CDEH session change or an App-layer connect that starts nothing sends nothing either: the tests
//! wait for its line in the engine's log (`support/session_log`), written once it has run, before
//! they check that nothing started. The CDEH changes run in arrival order, so a later change's line
//! also says that every change before it has run.

use crate::support::auth_world;
use crate::support::engine_world as world;
use crate::support::fake_hub::on_connected;
use crate::support::session_log;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use auth_world::harness::eventually;
use phonelink_engine::auth::{FetchMode, PeerEvent};
use phonelink_engine::config::EngineConfig;
use phonelink_engine::constants::{PROFILE_PL, PROFILE_WEA};
use phonelink_engine::edge::standins::FixedLock;
use phonelink_engine::edge::{Edges, LockState};
use phonelink_engine::engine::{EngineCore, SystemEvent};
use phonelink_engine::identification::LOGICAL_DEVICE_ID_KEY;
use phonelink_engine::platform::SessionPhase;
use phonelink_engine::tap::MemoryTap;
use phonelink_engine::types::DcgClient;
use world::World;

/// The App layer's connects for a wake or an unlock to `phone` have all ended, and there was one.
fn unlock_or_wake_connect_ended(phone: &DcgClient) -> bool {
    session_log::connects_ended(phone, "UnlockOrWake").is_some_and(|n| n > 0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pl_and_wea_enroll_with_their_own_keys_and_capabilities_over_one_connection() {
    session_log::install();
    let w = World::new(None).await;

    // The CDEH identity keeps the unprefixed keys (World seeds them); the primary one has its own.
    w.h.seed_session();

    let production = EngineConfig::production();
    assert_eq!(production.identity.profile.client_type, "PL");
    assert_eq!(production.identity.cdeh.as_ref().map(|c| c.profile.client_type), Some("WEA"));

    let engine =
        EngineCore::build(w.two_identities(), w.h.edges(), World::feature_edges(), MemoryTap::new(), w.h.core_events())
            .expect("engine builds");
    engine.start(None).await.expect("start sequence");

    let enrollments: Vec<_> =
        w.h.dcg.requests().into_iter().filter(|r| r.path == "DeviceAuthProxy/EnrollDevice").map(|r| r.json()).collect();

    let by_type = |t: &str| {
        enrollments
            .iter()
            .find(|b| b["metadata"]["clientType"] == t)
            .unwrap_or_else(|| panic!("a {t} enrollment"))
            .clone()
    };

    assert_eq!(enrollments.len(), 2, "one enrollment per identity");
    assert_eq!(by_type("PL")["metadata"]["capabilities"], serde_json::json!(["UNLINKABLE_V1", "UNLINKABLE_V2"]));

    // The clipboard's global switch (on in these stores) adds `CLIPBOARD` to the identity that carries
    // the clipboard, the CDEH one; remote control's options (on by default) add its two strings there.
    assert_eq!(
        by_type("WEA")["metadata"]["capabilities"],
        serde_json::json!([
            "CLIPBOARD",
            "DEVICE_STATUS",
            "UNLINKABLE_V1",
            "REMOTE_CONTROL",
            "REMOTE_CONTROL_V2",
            "FILE_TRANSFER"
        ])
    );

    let cdeh = engine.cdeh.as_ref().expect("the CDEH identity's stack");
    let (pl, wea) = (engine.auth.own_client().unwrap(), cdeh.auth.own_client().unwrap());
    assert_ne!(pl.id, wea.id, "two devices of the account");
    assert!(engine.phone().is_some(), "the phone is selected from the primary identity's device list");
    assert!(cdeh.routes().is_some(), "the CDEH identity has its own platform routes");

    let keys = w.h.store.keys();
    assert!(keys.iter().any(|k| k.starts_with("pl/auth/")), "the primary identity's keys are prefixed: {keys:?}");
    assert!(keys.iter().any(|k| k == LOGICAL_DEVICE_ID_KEY), "the machine's logical device id");
    assert!(!keys.iter().any(|k| k == &format!("pl/{LOGICAL_DEVICE_ID_KEY}")), "shared, not per identity");
    assert_eq!(engine.ident.logical_device_id, cdeh.ident.logical_device_id);
    assert_ne!(engine.ident.app_id, cdeh.ident.app_id, "each identity's package family");

    // Only the CDEH identity opens a hub connection: the App layer's start and the engine's session
    // start both connect it.
    assert!(eventually(Duration::from_secs(20), || !w.hub.negotiates().is_empty()).await, "a hub connection");

    let phone = w.phone();
    assert!(
        eventually(Duration::from_secs(20), || {
            session_log::changes(&phone).contains(&"starting CDEH.ProcessSessionChanges".to_string())
        })
        .await,
        "the engine's session start ran: {:?}",
        session_log::changes(&phone)
    );

    let app_ids: BTreeSet<String> =
        w.hub.negotiates().iter().filter_map(|n| n.header("dcg-appid").map(str::to_string)).collect();
    assert_eq!(app_ids, BTreeSet::from([PROFILE_WEA.package_family.to_string()]));
    assert!(Arc::ptr_eq(&engine.sessions, &cdeh.sessions), "the App layer's connection is the CDEH one");
}

/// The CDEH identity's session follows the Mac's lock, as CDEH's session service does: the start and
/// the unlock start it with a wake that asks for a new session and ignores the device-disabled status;
/// a lock stops it; a wake while the Mac is still locked starts nothing.
///
/// The App layer's own connect at the start and at the unlock runs on the same connection and may
/// wake the phone first, as a `Default` start: the session start's new-session request is then kept
/// for the connection's next wake. Wakes that wait 1 s for the phone, which never answers here, bring
/// that next wake within a second.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cdeh_session_stops_on_lock_and_starts_again_on_unlock() {
    session_log::install();
    let w = World::new(None).await;
    w.h.seed_session();

    let lock = Arc::new(FixedLock::new(LockState::Unlocked));
    let edges = Edges { lock: lock.clone(), ..w.h.edges() };
    let mut config = w.two_identities();
    config.auth.wake.ttl_default = Duration::from_secs(1);
    config.auth.wake.minimum_partner_wait = Duration::from_secs(1);

    let engine = EngineCore::build(config, edges, World::feature_edges(), MemoryTap::new(), w.h.core_events())
        .expect("engine builds");
    engine.start(None).await.expect("start sequence");

    let cdeh = engine.cdeh.as_ref().expect("the CDEH identity's stack");
    let phone = w.phone();
    let cdeh_wakes = || {
        w.h.dcg
            .of("Wake")
            .into_iter()
            .filter(|r| r.header("dcg-appid") == Some(PROFILE_WEA.package_family))
            .map(|r| r.json()["data"].clone())
            .collect::<Vec<_>>()
    };
    let new_session = |data: &serde_json::Value| {
        data["DCG-RequestNewSession"] == "True" && data["IgnoreDeviceDisabledStatus"] == "True"
    };

    // The hub announces the connection it accepts, after which the PC wakes the phone.
    assert!(eventually(Duration::from_secs(20), || !w.hub.sessions().is_empty()).await, "a hub connection");

    for session in w.hub.sessions() {
        session.send(on_connected(Some("westus"), &[]));
    }

    assert!(
        eventually(Duration::from_secs(20), || cdeh_wakes().iter().any(new_session)).await,
        "the start wakes the phone for a new session: {:?}",
        cdeh_wakes()
    );

    lock.set(LockState::Locked);
    engine.system_event(SystemEvent::ScreenLocked);
    assert!(
        eventually(Duration::from_secs(10), || cdeh.sessions.phase(&phone) == SessionPhase::Stopped).await,
        "the lock stops the CDEH session"
    );

    let (wakes, negotiates) = (cdeh_wakes().len(), w.hub.negotiates().len());
    let changes = session_log::changes(&phone).len();

    engine.system_event(SystemEvent::DidWake);
    // The wake's session start is refused for the lock and its App connect stops at the App layer's
    // lock gate.
    assert!(
        eventually(Duration::from_secs(20), || {
            session_log::changes(&phone)[changes..] == ["not started: the Mac is locked"]
                && unlock_or_wake_connect_ended(&phone)
        })
        .await,
        "the wake ran: {:?} {:?}",
        &session_log::changes(&phone)[changes..],
        session_log::connects(&phone)
    );

    assert_eq!(cdeh.sessions.phase(&phone), SessionPhase::Stopped, "a wake while locked starts nothing");
    assert_eq!((cdeh_wakes().len(), w.hub.negotiates().len()), (wakes, negotiates), "nothing reached the hub");

    // The unlock event starts it even before the lock test reads unlocked.
    engine.system_event(SystemEvent::ScreenUnlocked);
    assert!(
        eventually(Duration::from_secs(20), || {
            session_log::changes(&phone).contains(&"starting CDEH.SessionUnlocked".to_string())
        })
        .await,
        "the unlock starts the session: {:?}",
        session_log::changes(&phone)
    );

    assert!(eventually(Duration::from_secs(20), || w.hub.sessions().len() >= 2).await, "a new hub connection");
    for session in &w.hub.sessions()[1..] {
        session.send(on_connected(Some("westus"), &[]));
    }

    assert!(
        eventually(Duration::from_secs(20), || cdeh_wakes()[wakes..].iter().any(new_session)).await,
        "the unlock wakes the phone again for a new session: {:?}",
        &cdeh_wakes()[wakes..]
    );

    assert_ne!(cdeh.sessions.phase(&phone), SessionPhase::Stopped);
}

/// With a CDEH identity the App layer rides the CDEH identity's connection: its `connect` reaches the phone
/// over that one connection, the primary identity never wakes the phone, and the lock that stops the CDEH
/// session takes the App layer off the phone with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_app_layer_rides_the_cdeh_connection() {
    let w = World::new(None).await;
    w.h.seed_session();

    let lock = Arc::new(FixedLock::new(LockState::Unlocked));
    let edges = Edges { lock: lock.clone(), ..w.h.edges() };

    let engine =
        EngineCore::build(w.two_identities(), edges, World::feature_edges(), MemoryTap::new(), w.h.core_events())
            .expect("engine builds");
    engine.start(None).await.expect("start sequence");

    let phone = w.phone();
    let wakes_from =
        |family: &str| w.h.dcg.of("Wake").into_iter().filter(|r| r.header("dcg-appid") == Some(family)).count();

    w.connect_phone(|| engine.phone_connected()).await;
    assert!(
        eventually(Duration::from_secs(20), || !w.requests_on("/connect/v1").is_empty()).await,
        "the App layer's connect reaches the phone"
    );

    assert_eq!(w.hub.sessions().len(), 1, "one hub connection");
    assert!(
        w.hub.negotiates().iter().all(|n| n.header("dcg-appid") == Some(PROFILE_WEA.package_family)),
        "the connection is the CDEH identity's"
    );
    assert!(wakes_from(PROFILE_WEA.package_family) > 0);
    assert_eq!(wakes_from(PROFILE_PL.package_family), 0, "the primary identity never wakes the phone");

    lock.set(LockState::Locked);
    engine.system_event(SystemEvent::ScreenLocked);
    assert!(
        eventually(Duration::from_secs(10), || engine.sessions.phase(&phone) == SessionPhase::Stopped).await,
        "the lock stops the one session"
    );
    assert!(!engine.phone_connected());
}

/// The per-phone switch drives the CDEH session as CDEH's does: turning it off stops the session, and
/// the App layer's connects start nothing while it is off; turning it on starts it again with a new
/// connection whose trace state names the trigger.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_per_phone_switch_stops_and_starts_the_cdeh_session() {
    session_log::install();
    let w = World::new(None).await;
    w.h.seed_session();

    let engine =
        EngineCore::build(w.two_identities(), w.h.edges(), World::feature_edges(), MemoryTap::new(), w.h.core_events())
            .expect("engine builds");
    engine.start(None).await.expect("start sequence");
    let phone = w.phone();
    w.connect_phone(|| engine.phone_connected()).await;

    engine.switches.set_enabled(&phone, false).expect("switch off");
    assert!(
        eventually(Duration::from_secs(10), || engine.sessions.phase(&phone) == SessionPhase::Stopped).await,
        "switching the phone off stops the session"
    );

    let negotiates = w.hub.negotiates().len();
    let changes = session_log::changes(&phone).len();

    engine.system_event(SystemEvent::DidWake);
    assert!(
        eventually(Duration::from_secs(20), || {
            session_log::changes(&phone)[changes..] == ["not started: the phone is disabled"]
                && unlock_or_wake_connect_ended(&phone)
        })
        .await,
        "the wake ran: {:?} {:?}",
        &session_log::changes(&phone)[changes..],
        session_log::connects(&phone)
    );

    assert_eq!(w.hub.negotiates().len(), negotiates, "nothing connects a disabled phone");
    assert_eq!(engine.sessions.phase(&phone), SessionPhase::Stopped);

    engine.switches.set_enabled(&phone, true).expect("switch on");

    let traces = || {
        w.hub.negotiates()[negotiates..]
            .iter()
            .map(|n| n.header("tracestate").unwrap_or_default().to_string())
            .collect::<Vec<_>>()
    };
    assert!(
        eventually(Duration::from_secs(20), || traces().iter().any(|t| t.contains("ypTriggerId=CDEH.DeviceEnabled")))
            .await,
        "switching it on starts the session with its trigger: {:?}",
        traces()
    );
}

/// A phone whose Link to Windows is older than the minimum version gets no CDEH session start; the
/// App layer's own connect still reaches it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_phone_below_the_minimum_version_gets_no_cdeh_session_start() {
    session_log::install();
    let w = World::new(None).await;
    w.h.seed_session();

    let mut config = w.two_identities();
    config.platform.drm_minimum_client_version = "99.0.0.0".into();

    let engine = EngineCore::build(config, w.h.edges(), World::feature_edges(), MemoryTap::new(), w.h.core_events())
        .expect("engine builds");
    engine.start(None).await.expect("start sequence");

    let phone = w.phone();
    assert!(
        eventually(Duration::from_secs(20), || session_log::changes(&phone) == [TOO_OLD]).await,
        "the start is refused: {:?}",
        session_log::changes(&phone)
    );

    w.connect_phone(|| engine.phone_connected()).await;
    assert_eq!(session_log::changes(&phone), [TOO_OLD], "no CDEH session start");

    for negotiate in w.hub.negotiates() {
        let trace = negotiate.header("tracestate").unwrap_or_default();
        assert!(!trace.contains("ypTriggerId=CDEH."), "a CDEH start opened a connection: {trace}");
    }
}

/// The session change a phone below the minimum version gets.
const TOO_OLD: &str = "not started: the phone's Link to Windows is older than the minimum version";

/// A phone whose device-list entry carries no Link to Windows version gets no CDEH session start, and
/// a later entry that brings a passing version starts none either, because the CDEH session
/// reacts to the device's enabled state only; the next start trigger starts the session, once.
///
/// The next trigger here is the phone leaving the device list and coming back: those changes reach the
/// session through the same device-list events as the update, after it, so a start the update had made
/// would be logged before theirs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_version_a_later_device_list_brings_starts_no_cdeh_session_by_itself() {
    session_log::install();
    let w = World::new(None).await;
    w.h.seed_session();
    let version = w.h.dcg.peer_mut(&w.phone_id, |p| p.client_version.take()).expect("the phone's version");

    let engine =
        EngineCore::build(w.two_identities(), w.h.edges(), World::feature_edges(), MemoryTap::new(), w.h.core_events())
            .expect("engine builds");
    engine.start(None).await.expect("start sequence");

    let cdeh = engine.cdeh.as_ref().expect("the CDEH identity's stack");
    let phone = w.phone();
    assert!(
        eventually(Duration::from_secs(20), || session_log::changes(&phone) == [TOO_OLD]).await,
        "the start is refused without a version: {:?}",
        session_log::changes(&phone)
    );

    let mut peer_events = cdeh.auth.subscribe_peer_events();
    w.h.dcg.peer_mut(&w.phone_id, |p| p.client_version = Some(version.clone()));
    cdeh.auth.sync_trust(FetchMode::ForceCloud).await.expect("device-list refresh");

    loop {
        match peer_events.try_recv().expect("the refresh reported the phone's new entry") {
            PeerEvent::Updated(peer) if peer.client.is_same_client(&phone) => {
                assert_eq!(peer.info.client_version.as_deref(), Some(version.as_str()));
                break;
            }
            _ => {}
        }
    }

    w.h.dcg.peer_mut(&w.phone_id, |p| p.is_linked = Some(false));
    cdeh.auth.sync_trust(FetchMode::ForceCloud).await.expect("device-list refresh");
    w.h.dcg.peer_mut(&w.phone_id, |p| p.is_linked = Some(true));
    cdeh.auth.sync_trust(FetchMode::ForceCloud).await.expect("device-list refresh");

    let started = "starting CDEH.ProcessSessionChanges";
    assert!(
        eventually(Duration::from_secs(20), || session_log::changes(&phone).last().map(String::as_str)
            == Some(started))
        .await,
        "the phone's return starts the session: {:?}",
        session_log::changes(&phone)
    );
    // The stop of the phone's leaving may still have been queued when its return came.
    let changes = session_log::changes(&phone);
    assert!(
        matches!(changes.iter().map(String::as_str).collect::<Vec<_>>()[..], [TOO_OLD, "stopping" | "superseded stop", s] if s == started),
        "the new entry started nothing; the phone's return started the session once: {changes:?}"
    );
}
