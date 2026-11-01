//! Device status — the Cross Device Experience Host contract over the platform layer:
//! `DeviceStatusResponseMessage` and `DeviceStatus`, the status pull and push, setting DND and the
//! ringer and the DND permission request (never sent), the PC status for the phone; the lifecycle
//! sequence and the sender preconditions; the derived example vectors.

use crate::support::device_status_world;
use crate::support::platform_world;

use std::time::Duration;

use prost::Message;

use device_status_world::*;
use phonelink_engine::edge::LockState;
use phonelink_engine::features::PeerDirectory;
use phonelink_engine::features::device_status::edge::standins::PcState;
use phonelink_engine::features::device_status::{
    BatteryItem, CdehStatus, CellularItem, ControlOutcome, DeviceStatusEvent, DndShown, PcBattery, PcInternetPath,
    RadioState, WifiItem,
};
use phonelink_engine::platform::route;
use phonelink_engine::proto::device_info::v1::{
    DeviceStatus as PbDeviceStatus, DeviceStatusRequestMessage, DeviceStatusRequestType, DeviceStatusResponseMessage,
    DeviceStatusResponseStatus, NetworkType,
};
use phonelink_engine::proto::permissions::v1::{FeaturePermissions, PermissionsRequestMessage};
use phonelink_engine::proto::platform_sdk::v1 as pb;
use phonelink_engine::seams::PlatformRequestStatus;
use phonelink_engine::types::PartnerDisconnectReason;
use platform_world::{drm_answer, phone_id, settle};

const OK: i32 = DeviceStatusResponseStatus::Ok as i32;

/// The page vector's status read as CDEH does.
fn page_status() -> CdehStatus {
    CdehStatus {
        has_info: true,
        battery_percent: 57,
        battery_charging: true,
        battery_power_save: false,
        bluetooth: RadioState::On,
        do_not_disturb_status: 1,
        do_not_disturb_rule_enabled: true,
        cellular_signal_level: 4,
        cellular_signal_level_max: 5,
        cellular: RadioState::On,
        ringer_mode: 2,
        wifi_signal_level: 3,
        wifi: RadioState::On,
        airplane_mode: false,
        is_connection_metered: false,
    }
}

// ------------------------------------------------------------------------------------------------
// The pull
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn the_pull_is_the_page_vector_and_an_ok_reply_fills_the_cdeh_model() {
    let sw = StatusWorld::new(phone_item());
    sw.phone_answers_pulls(PAGE_STATUS_VECTOR.to_vec());
    let ds = sw.install();
    let mut events = ds.subscribe();

    settle().await;
    assert!(sw.pulls().is_empty(), "not available at start");
    assert_eq!(ds.cdeh_status(&phone_id()), None);

    sw.w.connect_phone().await;
    settle().await;

    let received = sw.w.phone_drm.received();
    assert_eq!(received.len(), 1, "one pull when the phone became available");
    assert_eq!(received[0].message.payload, PAGE_PULL_VECTOR, "GET /devicestatus, UNKNOWN, empty payload");
    assert_eq!(received[0].message.headers.get("ms-content-type"), Some("application/x-binary"));
    assert_eq!(ds.cdeh_status(&phone_id()), Some(page_status()));
    assert!(StatusWorld::events(&mut events).contains(&DeviceStatusEvent::Changed { phone: phone_id() }));
}

#[tokio::test(start_paused = true)]
async fn the_start_pulls_every_available_phone_with_device_status() {
    let sw = StatusWorld::new(phone_item());
    sw.phone_answers_pulls(PAGE_STATUS_VECTOR.to_vec());
    sw.w.connect_phone().await;
    let ds = sw.install();
    settle().await;

    assert_eq!(sw.pulls().len(), 1);
    assert_eq!(ds.cdeh_status(&phone_id()), Some(page_status()));

    // Without DEVICE_STATUS (exact string): no pull, at start or on a connection.
    let mut item = phone_item();
    item.capabilities = Some(vec!["device_status".into(), "CLIPBOARD".into()]);
    let sw = StatusWorld::new(item);
    sw.phone_answers_pulls(PAGE_STATUS_VECTOR.to_vec());

    sw.w.connect_phone().await;
    let ds = sw.install();
    settle().await;

    sw.w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::LocalHubDisconnect);
    settle().await;
    sw.w.connect_phone().await;
    settle().await;

    assert!(sw.pulls().is_empty());
    assert_eq!(ds.cdeh_status(&phone_id()), None);
}

#[tokio::test(start_paused = true)]
async fn a_phone_added_to_the_device_list_is_pulled_when_available() {
    let sw = StatusWorld::new(phone_item());
    let peer = sw.peers.peer(&phone_id()).unwrap();
    sw.peers.remove(&phone_id());
    sw.phone_answers_pulls(PAGE_STATUS_VECTOR.to_vec());

    let ds = sw.install();
    sw.w.connect_phone().await;
    settle().await;

    assert!(sw.pulls().is_empty(), "available, but not in the account's device list");

    sw.peers.upsert(peer.clone());
    settle().await;
    assert_eq!(sw.pulls().len(), 1, "added while available");
    assert_eq!(ds.cdeh_status(&phone_id()), Some(page_status()));

    // An update of the entry is not an addition.
    sw.peers.upsert(peer);
    settle().await;
    assert_eq!(sw.pulls().len(), 1);

    // A removed phone's status goes with it.
    sw.peers.remove(&phone_id());
    settle().await;
    assert_eq!(ds.cdeh_status(&phone_id()), None);
}

#[tokio::test(start_paused = true)]
async fn the_sender_preconditions_send_nothing() {
    // The phone switch off (UserDisabled).
    let sw = StatusWorld::new(phone_item());
    sw.switches.set_enabled(&phone_id(), false).unwrap();
    let _ds = sw.install();
    sw.w.connect_phone().await;
    settle().await;
    assert!(sw.pulls().is_empty(), "switch off");

    // A Link to Windows below 1.23112.189.0.
    let mut item = phone_item();
    item.client_version = Some("1.23112.189".into());
    let sw = StatusWorld::new(item);
    let _ds = sw.install();
    sw.w.connect_phone().await;
    settle().await;
    assert!(sw.pulls().is_empty(), "LtwMinVersionRequired");

    // Locked, and not repeated on unlock.
    let sw = StatusWorld::new(phone_item());
    let _ds = sw.install();
    sw.w.lock.set(LockState::Locked);
    sw.w.connect_phone().await;
    settle().await;

    sw.w.lock.set(LockState::Unlocked);
    tokio::time::sleep(Duration::from_secs(600)).await;
    assert!(sw.pulls().is_empty(), "locked; no retry on unlock");

    // No current account.
    let sw = StatusWorld::new(phone_item());
    sw.peers.set_enrolled(false);
    sw.w.connect_phone().await;
    let _ds = sw.install();
    settle().await;

    sw.w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::LocalHubDisconnect);
    settle().await;
    sw.w.connect_phone().await;
    settle().await;
    assert!(sw.pulls().is_empty(), "no current account");
}

/// The sender's preconditions: the pull asks for no forced session start, so the request wrapper starts a
/// `Default` session (not `NewSession`) for a phone that is not available.
#[test]
fn the_pull_asks_for_no_forced_session_start() {
    use phonelink_engine::features::device_status::cdeh::pull_request;
    let peer = phonelink_engine::auth::PeerDevice { client: phone_id(), info: phone_item() };
    let request = pull_request(&peer, true, Duration::from_secs(120), tokio_util::sync::CancellationToken::new());
    assert!(!request.force_session_start);
    assert_eq!(request.timeout, Duration::from_secs(120));
}

#[tokio::test(start_paused = true)]
async fn a_failed_or_unsuccessful_pull_keeps_the_model_and_is_not_retried() {
    let sw = StatusWorld::new(phone_item());
    sw.phone_answers_pulls(PAGE_STATUS_VECTOR.to_vec());
    let ds = sw.install();
    sw.w.connect_phone().await;
    settle().await;
    assert_eq!(ds.cdeh_status(&phone_id()), Some(page_status()));

    let not_found = status_message(
        DeviceStatusResponseStatus::NotFound as i32,
        Some(PbDeviceStatus { battery_percent: 5, ..Default::default() }),
    );
    let replies: Vec<Box<dyn Fn() -> Option<phonelink_engine::types::TransportMessage> + Send + Sync>> = vec![
        Box::new(move || Some(drm_answer(&not_found, Some(1)))),
        Box::new(|| Some(drm_answer(&[0xFF, 0xFF, 0xFF], Some(1)))),
        Box::new(|| Some(drm_answer(&status_message(OK, None), Some(2)))),
        Box::new(|| Some(drm_answer(&[], Some(1)))),
        Box::new(|| None),
    ];

    let mut expected_pulls = 1;
    for reply in replies {
        sw.w.phone_drm.set_reply(Box::new(move |_| reply()));
        sw.w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::LocalHubDisconnect);
        settle().await;
        sw.w.connect_phone().await;
        tokio::time::sleep(Duration::from_secs(130)).await;

        expected_pulls += 1;
        assert_eq!(sw.pulls().len(), expected_pulls, "one pull per availability change, no retry");
        assert_eq!(ds.cdeh_status(&phone_id()), Some(page_status()), "status kept");
    }

    // An OK reply without info replaces the model with the "info absent" reading.
    sw.phone_answers_pulls(status_message(OK, None));
    sw.w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::LocalHubDisconnect);
    settle().await;
    sw.w.connect_phone().await;
    settle().await;
    assert_eq!(ds.cdeh_status(&phone_id()), Some(CdehStatus::read(None)));
}

// ------------------------------------------------------------------------------------------------
// The push
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_tag_8_push_replaces_the_status_of_the_phone_it_arrived_from() {
    let mut item = phone_item();
    item.capabilities = None; // a push is accepted without DEVICE_STATUS
    let sw = StatusWorld::new(item);

    let ds = sw.install();
    sw.w.connect_phone().await;
    settle().await;
    assert!(sw.pulls().is_empty());

    // `dcg_client_id` names someone else: not read; the connection's partner is the phone.
    sw.phone_publishes(8, "m-1", PAGE_STATUS_VECTOR.to_vec()).await;
    assert_eq!(ds.cdeh_status(&phone_id()), Some(page_status()));
    assert_eq!(sw.w.pair.pc_link.sent_on(route::INTERNAL_RESPONSE).len(), 1, "acknowledged {{}}");

    let metered = status_message(
        OK,
        Some(PbDeviceStatus { is_connection_metered: true, battery_percent: 20, ..Default::default() }),
    );
    sw.phone_publishes(8, "m-2", metered.clone()).await;
    assert!(ds.cdeh_status(&phone_id()).unwrap().is_connection_metered);
    assert_eq!(ds.cdeh_status(&phone_id()).unwrap().battery_percent, 20);

    // Not OK, undecodable, empty, another tag: ignored.
    let before = ds.cdeh_status(&phone_id());

    sw.phone_publishes(
        8,
        "m-3",
        status_message(DeviceStatusResponseStatus::UnknownError as i32, Some(PbDeviceStatus::default())),
    )
    .await;
    sw.phone_publishes(8, "m-4", vec![0xFF, 0xFF]).await;
    sw.phone_publishes(8, "m-5", Vec::new()).await;
    sw.phone_publishes(9, "m-6", PAGE_STATUS_VECTOR.to_vec()).await;
    assert_eq!(ds.cdeh_status(&phone_id()), before);

    // The id last delivered to the tag-8 subscriber again (tag 9's m-6 did not reach it): an OK status
    // under that id is dropped as a repeat, while the same status under a fresh id applies.
    sw.phone_publishes(8, "m-5", PAGE_STATUS_VECTOR.to_vec()).await;
    assert_eq!(ds.cdeh_status(&phone_id()), before, "a repeated message id is ignored");
    sw.phone_publishes(8, "m-8", PAGE_STATUS_VECTOR.to_vec()).await;
    assert_eq!(ds.cdeh_status(&phone_id()), Some(page_status()));

    // A publication from a phone that is not among the account's phones: ignored.
    sw.peers.remove(&phone_id());
    settle().await;
    sw.phone_publishes(8, "m-7", PAGE_STATUS_VECTOR.to_vec()).await;
    assert_eq!(ds.cdeh_status(&phone_id()), None);
}

/// An `OK` status without `info` replaces the CDEH model with CDEH's "info absent" reading, which the shown
/// status takes no value from — no "No SIM" for its `-1` level, no known DND for its `0` — so nothing
/// CDEH-sourced is shown and the DND control is disabled.
#[tokio::test(start_paused = true)]
async fn an_ok_status_without_info_shows_nothing_from_cdeh() {
    let sw = StatusWorld::new(phone_item());
    let ds = sw.install();
    sw.w.connect_phone().await;
    settle().await;

    sw.phone_publishes(8, "m-1", PAGE_STATUS_VECTOR.to_vec()).await;
    let shown = ds.status(&phone_id());
    assert!(matches!(shown.cellular, CellularItem::Bars { level: 4, max: 5, .. }));
    assert_eq!(shown.do_not_disturb.map(|c| c.shown), Some(DndShown::Status { status: 1, on: false }));

    sw.phone_publishes(8, "m-2", status_message(OK, None)).await;
    assert_eq!(ds.cdeh_status(&phone_id()), Some(CdehStatus::read(None)));

    let shown = ds.status(&phone_id());
    assert_eq!(shown.cellular, CellularItem::Unknown);
    assert_eq!(
        (shown.battery, shown.wifi, shown.bluetooth),
        (BatteryItem::Unknown, WifiItem::Unknown, RadioState::Unknown)
    );
    let dnd = shown.do_not_disturb.expect("the phone has the DND control");
    assert_eq!((dnd.shown, dnd.enabled), (DndShown::Unknown, false));
    assert_eq!(ds.toggle_do_not_disturb(&phone_id()), ControlOutcome::NotAvailable, "nothing is sent");
}

// ------------------------------------------------------------------------------------------------
// The PC's status for the phone
// ------------------------------------------------------------------------------------------------

fn pc_answer(response: &phonelink_engine::seams::PlatformResponse) -> DeviceStatusResponseMessage {
    assert_eq!(response.status, PlatformRequestStatus::Success);
    let body = pb::DeviceResourceManagerResponseMessage::decode(response.message.as_ref().unwrap().payload.as_slice())
        .unwrap();
    assert_eq!(body.response_type, Some(1), "SUCCESS");
    DeviceStatusResponseMessage::decode(body.payload.as_slice()).unwrap()
}

#[tokio::test(start_paused = true)]
async fn remotedevicestatus_answers_the_pcs_own_status() {
    let sw = StatusWorld::new(phone_item());
    let _ds = sw.install();
    sw.w.connect_phone().await;

    sw.pc.set(|s| {
        *s = PcState {
            battery: Some(PcBattery { current_capacity: 73, max_capacity: 100, on_ac_power: false }),
            low_power_mode: true,
            path: PcInternetPath::WiFi,
            wifi_rssi: Some(-50),
            failing: false,
        }
    });

    let answer = pc_answer(&sw.phone_drm_request(4, 1, "/remotedevicestatus", b"").await);
    assert_eq!(answer.status, OK);
    assert_eq!(
        answer.info,
        Some(PbDeviceStatus {
            battery_percent: 73,
            battery_charging: false,
            battery_power_save: true,
            network_type: NetworkType::Wifi as i32,
            wifi_signal_level: 4,
            ..Default::default()
        })
    );

    // Whatever the request type, query or payload.
    sw.pc.set(|s| {
        s.battery = None;
        s.path = PcInternetPath::Cellular;
    });
    let answer = pc_answer(&sw.phone_drm_request(4, 2, "/remotedevicestatus?x=1", b"\x08\x02").await);
    assert_eq!(
        answer.info,
        Some(PbDeviceStatus { battery_percent: -1, network_type: NetworkType::Cellular as i32, ..Default::default() })
    );

    // Reading fails: UNKNOWN_ERROR without info.
    sw.pc.set(|s| s.failing = true);
    let answer = pc_answer(&sw.phone_drm_request(4, 4, "/remotedevicestatus", b"").await);
    assert_eq!((answer.status, answer.info), (DeviceStatusResponseStatus::UnknownError as i32, None));
}

// ------------------------------------------------------------------------------------------------
// The requests the engine never sends, and the page's vectors
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn only_get_devicestatus_is_ever_sent_on_device_resource_manager() {
    let sw = StatusWorld::new(phone_item());
    sw.phone_answers_pulls(PAGE_STATUS_VECTOR.to_vec());
    let ds = sw.install();
    sw.w.connect_phone().await;
    settle().await;

    sw.app
        .update_phone_record(|r| r.set_permission(phonelink_engine::app::consts::permission::NOTIFICATIONS_SYNC, true));
    let keys =
        phonelink_engine::codec::valueset::ValueSet::new().with("doNotDisturbStatus", 2i32).with("ringerMode", 1i32);
    sw.phone_sends("deviceStatus", keys).await;
    settle().await;

    ds.toggle_do_not_disturb(&phone_id());
    ds.cycle_ringer_mode(&phone_id());
    tokio::time::sleep(Duration::from_secs(21)).await;
    ds.accept_prompt(&phone_id(), phonelink_engine::features::device_status::Prompt::DndTurnOffFailed);
    settle().await;

    let pulls = sw.pulls();
    assert!(!pulls.is_empty());
    for p in pulls {
        assert_eq!(
            (p.resource_type, p.request_type, p.resource_path.as_str(), p.payload.len()),
            (4, 1, "/devicestatus", 0)
        );
    }

    // The commands went over Phone Link's contract instead.
    let commands = sw.phone_app.received_on("/command/devicestatus");
    assert_eq!(commands.len(), 3);
}

#[test]
fn the_page_vectors_of_the_unsent_requests_and_negative_values() {
    let set = pb::DeviceResourceManagerMessage {
        resource_type: pb::ProtoDeviceResourceType::Unknown as i32,
        request_type: pb::ProtoDeviceResourceRequestType::Update as i32,
        payload: DeviceStatusRequestMessage { request_type: Some(DeviceStatusRequestType::SetDoNotDisturbOn as i32) }
            .encode_to_vec(),
        resource_path: "/devicestatus".into(),
    }
    .encode_to_vec();
    assert_eq!(
        set,
        [
            0x08, 0x04, 0x10, 0x02, 0x1A, 0x02, 0x08, 0x02, 0x22, 0x0D, 0x2F, 0x64, 0x65, 0x76, 0x69, 0x63, 0x65, 0x73,
            0x74, 0x61, 0x74, 0x75, 0x73
        ]
    );

    let permissions = pb::DeviceResourceManagerMessage {
        resource_type: pb::ProtoDeviceResourceType::Permissions as i32,
        request_type: pb::ProtoDeviceResourceRequestType::Sync as i32,
        payload: PermissionsRequestMessage { feature_permissions: vec![FeaturePermissions::DoNotDisturb as i32] }
            .encode_to_vec(),
        resource_path: "/permissions".into(),
    }
    .encode_to_vec();
    assert_eq!(
        permissions,
        [
            0x08, 0x03, 0x10, 0x04, 0x1A, 0x03, 0x0A, 0x01, 0x05, 0x22, 0x0C, 0x2F, 0x70, 0x65, 0x72, 0x6D, 0x69, 0x73,
            0x73, 0x69, 0x6F, 0x6E, 0x73
        ]
    );

    let ringer = PbDeviceStatus { ringer_mode: -5, ..Default::default() }.encode_to_vec();
    assert_eq!(ringer, [0x50, 0xFB, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]);

    let decoded = DeviceStatusResponseMessage::decode(PAGE_STATUS_VECTOR.as_slice()).unwrap();
    assert_eq!(decoded.status, OK);
    assert_eq!(CdehStatus::read(decoded.info.as_ref()), page_status());
    assert_eq!(decoded.encode_to_vec(), PAGE_STATUS_VECTOR, "fields in number order, empty fields omitted");
}
