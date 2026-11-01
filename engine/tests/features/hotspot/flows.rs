//! Instant hotspot and the shared phone Bluetooth device — the connect brings what is missing first
//! (one wait for the phone's permissions, then pairing), the phone's Bluetooth name in the device
//! match, one pairing at a time across features, the service-record refresh when the side channel is
//! missing, how a connect reports a closed or silent side channel, and the joined state a
//! successful connect leaves, which disconnect ends.

use crate::support::app_world;
use crate::support::common::{eventually, payload};
use crate::support::feature_world;
use phonelink_engine::config::features::BluetoothConfig;
use phonelink_engine::config::features::HotspotConfig;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use app_world::*;
use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use phonelink_engine::app::consts::{permission, platform, ypp};
use phonelink_engine::app::{BluetoothRecord, HotspotRecord};
use phonelink_engine::auth::AuthError;
use phonelink_engine::codec::json::OrderedStringMap;
use phonelink_engine::codec::valueset::ValueSet;
use phonelink_engine::features::bluetooth::edge::testing::Answer;
use phonelink_engine::features::bluetooth::edge::testing::ScriptedBluetooth;
use phonelink_engine::features::bluetooth::{Bluetooth, BluetoothFailure, RfcommError};
use phonelink_engine::features::hotspot::edge::WifiError;
use phonelink_engine::features::hotspot::edge::testing::ScriptedWifi;
use phonelink_engine::features::hotspot::{Hotspot, HotspotFailure, JwtRejection, SideChannelAuth, wire};
use phonelink_engine::proto::side_channel::v1::{
    SideChannelAuthorization, SideChannelHotspotReservation, SideChannelHotspotResponse,
    SideChannelHotspotResponseStatus, SideChannelServerMessage, SideChannelServerResponse, side_channel_server_message,
    side_channel_server_response,
};
use phonelink_engine::seams::PlatformRequest;
use phonelink_engine::types::DcgClient;
use prost::Message;
use rsa::pkcs8::DecodePublicKey;

const PHONE_ADDRESS: &str = "aa-bb-cc-dd-ee-ff";

/// Signs every request; accepts no reply (these tests never get that far).
struct StubAuth;

#[async_trait]
impl SideChannelAuth for StubAuth {
    fn sign(&self, _phone: &DcgClient, _data: &OrderedStringMap, _scope: &str) -> Result<String, AuthError> {
        Ok("jwt".into())
    }
    fn validate(&self, _phone: &DcgClient, _token: &str, _scope: &str) -> Result<Option<String>, JwtRejection> {
        Err(JwtRejection::Invalid("not in these tests".into()))
    }
    async fn refresh_trust(&self) {}
}

/// The phone's half of the crypto trust: keeps the data it was asked to sign, and accepts its own
/// reply JWT, whose `Data` carries `SSID` + `PASSPHRASE` encrypted to the request's public key.
#[derive(Default)]
struct PhoneAuth {
    signed: Mutex<Option<OrderedStringMap>>,
}

const SSID: &str = "Galaxy S24";
const PASSPHRASE: &str = "correct horse";

impl PhoneAuth {
    fn signed(&self, key: &str) -> String {
        self.signed.lock().unwrap().as_ref().expect("a signed request").get(key).expect("a signed key").to_string()
    }
}

#[async_trait]
impl SideChannelAuth for PhoneAuth {
    fn sign(&self, _phone: &DcgClient, data: &OrderedStringMap, _scope: &str) -> Result<String, AuthError> {
        *self.signed.lock().unwrap() = Some(data.clone());
        Ok("jwt".into())
    }
    fn validate(&self, _phone: &DcgClient, _token: &str, _scope: &str) -> Result<Option<String>, JwtRejection> {
        let der = B64.decode(self.signed("publicKey")).unwrap();
        let key = rsa::RsaPublicKey::from_public_key_der(&der).unwrap();

        let plain = format!("{SSID}{PASSPHRASE}");
        let encrypted =
            key.encrypt(&mut rand_core06::OsRng, rsa::Oaep::new::<sha2_010::Sha256>(), plain.as_bytes()).unwrap();

        let data = serde_json::json!({
            "encryptedCredentials": B64.encode(encrypted),
            "encryptedSsidLength": SSID.encode_utf16().count().to_string(),
        });
        Ok(Some(data.to_string()))
    }
    async fn refresh_trust(&self) {}
}

/// The phone answers every request with its hotspot reserved for the request's nonce.
fn phone_reserves(auth: Arc<PhoneAuth>) -> Answer {
    Box::new(move |_request| {
        let response = SideChannelServerResponse {
            ok: true,
            error: None,
            authorization: Some(SideChannelAuthorization { signed_jwt_payload: "reply-jwt".into() }),
            context: None,
            response: Some(side_channel_server_response::Response::HotspotResponse(SideChannelHotspotResponse {
                status: SideChannelHotspotResponseStatus::Success as i32,
                reservation: Some(SideChannelHotspotReservation { status: 2, nonce: auth.signed("nonce") }),
                r#type: 2,
            })),
        };

        let message =
            SideChannelServerMessage { body: Some(side_channel_server_message::Body::RequestResponse(response)) };
        Some(wire::encode_frame(&message.encode_to_vec()))
    })
}

struct World {
    h: Harness,
    _w: feature_world::FeatureWorld,
    bluetooth: Arc<ScriptedBluetooth>,
    phone_bluetooth: Arc<Bluetooth>,
    wifi: Arc<ScriptedWifi>,
    hotspot: Arc<Hotspot>,
}

/// A supported Android 14 phone named "Galaxy" whose Bluetooth name is "Galaxy S24 BT".
fn world() -> World {
    world_with(Arc::new(StubAuth))
}

fn world_with(auth: Arc<dyn SideChannelAuth>) -> World {
    let h = Harness::new();
    let w = feature_world::feature_world(&h);
    let bluetooth = ScriptedBluetooth::new();
    let phone_bluetooth = Bluetooth::install(&w.env, BluetoothConfig::default(), bluetooth.clone()).unwrap();
    let wifi = ScriptedWifi::new();
    let hotspot =
        Hotspot::install(&w.env, HotspotConfig::default(), auth, phone_bluetooth.clone(), wifi.clone()).unwrap();

    h.app.update_phone_record(|r| {
        r.ypp_capabilities |= ypp::SIDE_CHANNEL_OEM_SUPPORT;
        r.platform = platform::ANDROID;
        r.android_sdk_version = 34;
        r.display_name = Some("Galaxy".into());
        r.status_keys.bluetooth_name = Some("Galaxy S24 BT".into());
        r.hotspot.capable = true;
    });

    World { h, _w: w, bluetooth, phone_bluetooth, wifi, hotspot }
}

fn route(req: &PlatformRequest) -> String {
    req.message.route().unwrap_or_default().to_string()
}

/// The routes sent so far, each `/permission` with its type.
fn sent(h: &Harness) -> Vec<String> {
    h.messenger
        .requests()
        .iter()
        .map(|req| match route(req).as_str() {
            "/permission" => format!("/permission {:#x}", payload(req).i32("permissionType").unwrap()),
            r => r.to_string(),
        })
        .collect()
}

fn phone_answers(h: &Harness, bluetooth_permission_result: i32) {
    h.messenger.set_responder(move |req| {
        let code = match payload(req).i32("permissionType") {
            Some(t) if t == permission::BLUETOOTH as i32 => bluetooth_permission_result,
            _ => 0,
        };
        Reply::Now(result_response(code))
    });
}

#[tokio::test(start_paused = true)]
async fn the_first_connect_waits_for_the_phone_permissions_then_pairs() {
    let world = world();
    phone_answers(&world.h, 0);
    *world.bluetooth.discovery.lock().unwrap() = Ok(PHONE_ADDRESS.into());

    let hotspot = world.hotspot.clone();
    let connect = tokio::spawn(async move { hotspot.connect(&phone()).await });
    assert!(
        eventually(|| sent(&world.h).iter().any(|r| r == "/hotspotsetupnotificationrequest")).await,
        "condition never held"
    );

    // Nothing is paired before the phone has opened its side-channel server.
    assert!(world.bluetooth.paired.lock().unwrap().is_none());

    // The phone allows instant hotspot; its OEM Bluetooth permission follows.
    world.h.app.update_phone_record(|r| r.hotspot.apply_permissions_update(Some(true), Some(true), Some(true)));

    // Then the credentials request goes to the device just paired; the phone stays silent.
    assert_eq!(connect.await.unwrap(), Err(HotspotFailure::NoAnswer));
    assert_eq!(world.bluetooth.opened.lock().unwrap()[0].0, PHONE_ADDRESS);

    assert_eq!(
        sent(&world.h),
        [
            "/hotspotsetupnotificationrequest",
            "/permission 0x2000000",
            "/permission 0x80",
            "/devicestatus",
            "/hotspotdevicedatasyncrequest",
        ]
    );

    let record = world.h.app.phone_record().unwrap();
    assert!(record.hotspot.setup_complete && record.hotspot.phone_permissions_granted(&record.bluetooth));

    // The discovery looked for the phone's Bluetooth name too.
    let asked = world.bluetooth.asked_names.lock().unwrap().clone();
    assert!(asked.last().unwrap().iter().any(|n| n == "Galaxy S24 BT"), "{asked:?}");

    // The next connect needs nothing first and sends no second setup-complete sync.
    let before = sent(&world.h).len();
    assert_eq!(world.hotspot.connect(&phone()).await, Err(HotspotFailure::NoAnswer));
    assert_eq!(sent(&world.h).len(), before);
}

#[tokio::test(start_paused = true)]
async fn an_ios_phone_offers_no_bluetooth_name() {
    let world = world();
    world.h.app.update_phone_record(|r| r.platform = platform::IOS);
    world.bluetooth.set_paired(None);
    phone_answers(&world.h, 0);

    assert_eq!(world.phone_bluetooth.pair(&phone(), "InstantHotspot").await, Err(BluetoothFailure::PhoneNotFound));
    let asked = world.bluetooth.asked_names.lock().unwrap().clone();
    assert!(asked.iter().flatten().all(|n| n != "Galaxy S24 BT"), "{asked:?}");

    // No device-status refresh for an iPhone either.
    assert_eq!(sent(&world.h), ["/permission 0x80"]);
}

#[tokio::test(start_paused = true)]
async fn a_bluetooth_capability_asks_for_the_permission_before_pairing() {
    let world = world();
    world.h.app.update_phone_record(|r| {
        r.bluetooth.capabilities = BluetoothRecord::REQUEST_BLUETOOTH_ADVERTISE_CONNECT_SCAN;
    });
    phone_answers(&world.h, 5);

    assert_eq!(
        world.phone_bluetooth.pair(&phone(), "InstantHotspot").await,
        Err(BluetoothFailure::PermissionNotGranted)
    );
    assert_eq!(sent(&world.h), ["/permission 0x2000000"]);
    assert!(!world.h.app.phone_record().unwrap().bluetooth.permission);
}

#[tokio::test(start_paused = true)]
async fn bluetooth_off_stops_before_any_request() {
    let world = world();
    world.bluetooth.radio.store(false, Ordering::SeqCst);
    assert_eq!(world.hotspot.connect(&phone()).await, Err(HotspotFailure::Bluetooth(BluetoothFailure::BluetoothOff)));
    assert!(sent(&world.h).is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_second_pairing_waits_for_the_first_and_finds_the_phone_paired() {
    let world = world();
    phone_answers(&world.h, 0);
    *world.bluetooth.discovery.lock().unwrap() = Ok(PHONE_ADDRESS.into());
    let (a, b) = (world.phone_bluetooth.clone(), world.phone_bluetooth.clone());

    let first = tokio::spawn(async move { a.pair(&phone(), "InstantHotspot").await });
    let second = tokio::spawn(async move { b.pair(&phone(), "SomeOtherFeature").await });
    assert_eq!(first.await.unwrap(), Ok(PHONE_ADDRESS.to_string()));
    assert_eq!(second.await.unwrap(), Ok(PHONE_ADDRESS.to_string()));
    assert_eq!(sent(&world.h).iter().filter(|r| *r == "/permission 0x80").count(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_denied_bluetooth_permission_ends_the_connect_at_once() {
    let world = world();
    world.bluetooth.set_paired(Some(PHONE_ADDRESS));
    world.h.app.update_phone_record(|r| r.hotspot.apply_permissions_update(Some(true), Some(true), None));
    phone_answers(&world.h, 5);

    assert_eq!(
        world.hotspot.connect(&phone()).await,
        Err(HotspotFailure::Bluetooth(BluetoothFailure::PermissionNotGranted))
    );
    assert_eq!(sent(&world.h), ["/permission 0x2000000"]);
    assert!(!world.h.app.phone_record().unwrap().hotspot.setup_complete);
}

#[tokio::test(start_paused = true)]
async fn the_connect_times_out_when_the_phone_never_consents() {
    let world = world();
    world.bluetooth.set_paired(Some(PHONE_ADDRESS));
    phone_answers(&world.h, 0);
    assert_eq!(world.hotspot.connect(&phone()).await, Err(HotspotFailure::ConsentTimedOut));
    assert_eq!(sent(&world.h), ["/hotspotsetupnotificationrequest"]);
}

#[test]
fn the_connect_reply_keys_land_in_their_records() {
    let reply = ValueSet::new()
        .with("hotspotUserConsent", true)
        .with("oemBluetoothRfcommServerOpen", true)
        .with("capabilities", ValueSet::new().with("hotspot", 1i32).with("bluetoothCapabilities", 1i32))
        .with("permissions", ValueSet::new().with("bluetooth", true).with("oem_bluetooth", true));

    let mut bluetooth = BluetoothRecord::default();
    bluetooth.apply_connect_reply(&reply);
    assert_eq!(bluetooth, BluetoothRecord { permission: true, capabilities: 1 });

    let mut hotspot = HotspotRecord::default();
    hotspot.apply_connect_reply(&reply);
    assert!(hotspot.capable && hotspot.user_consent && hotspot.oem_bluetooth_permission && hotspot.oem_rfcomm_enabled);
    assert!(hotspot.phone_permissions_granted(&bluetooth));

    // An undefined bluetoothCapabilities bit leaves the fact alone.
    let odd = ValueSet::new().with("capabilities", ValueSet::new().with("bluetoothCapabilities", 2i32));
    bluetooth.apply_connect_reply(&odd);
    assert_eq!(bluetooth.capabilities, 1);
}

/// A phone set up on this Mac and paired with it.
fn set_up_world() -> World {
    set_up(world())
}

fn set_up(world: World) -> World {
    world.bluetooth.set_paired(Some(PHONE_ADDRESS));
    world.h.app.update_phone_record(|r| {
        r.hotspot.apply_permissions_update(Some(true), Some(true), Some(true));
        r.bluetooth.permission = true;
        r.hotspot.setup_complete = true;
    });
    world
}

#[tokio::test(start_paused = true)]
async fn a_missing_side_channel_service_says_it_is_not_open() {
    let world = set_up_world();
    *world.bluetooth.refresh.lock().unwrap() = Ok(false);
    world.bluetooth.open_failures.lock().unwrap().extend([
        RfcommError::ServiceNotFound,
        RfcommError::ServiceNotFound,
        RfcommError::ServiceNotFound,
    ]);

    assert_eq!(world.hotspot.connect(&phone()).await, Err(HotspotFailure::SideChannelNotOpen));
    assert_eq!(world.bluetooth.opened.lock().unwrap().len(), 3);

    // The records were read again once, between the first and second attempts.
    assert_eq!(world.bluetooth.refreshed.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn service_records_read_before_the_server_opened_are_refreshed() {
    let world = set_up_world();
    world.bluetooth.open_failures.lock().unwrap().push_back(RfcommError::ServiceNotFound);

    // After the refresh the channel opens; the phone stays silent.
    assert_eq!(world.hotspot.connect(&phone()).await, Err(HotspotFailure::NoAnswer));
    assert_eq!(
        *world.bluetooth.refreshed.lock().unwrap(),
        [(PHONE_ADDRESS.to_string(), "111FAB8F-00A2-4935-B6B4-ACA388CDD473".to_string())]
    );
    assert_eq!(world.bluetooth.opened.lock().unwrap().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_failed_refresh_still_tries_again() {
    let world = set_up_world();
    *world.bluetooth.refresh.lock().unwrap() = Err(RfcommError::Unreachable);
    world.bluetooth.open_failures.lock().unwrap().push_back(RfcommError::ServiceNotFound);
    assert_eq!(world.hotspot.connect(&phone()).await, Err(HotspotFailure::NoAnswer));
    assert_eq!(world.bluetooth.opened.lock().unwrap().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_transport_failure_on_the_last_attempt_is_unreachable() {
    let world = set_up_world();
    world.bluetooth.open_failures.lock().unwrap().extend([
        RfcommError::ServiceNotFound,
        RfcommError::ServiceNotFound,
        RfcommError::Unreachable,
    ]);

    assert!(matches!(world.hotspot.connect(&phone()).await, Err(HotspotFailure::Unreachable(_))));
}

#[tokio::test(start_paused = true)]
async fn a_silent_phone_is_not_retried() {
    let world = set_up_world();
    // The channel opens; the phone never answers.
    assert_eq!(world.hotspot.connect(&phone()).await, Err(HotspotFailure::NoAnswer));
    assert_eq!(world.bluetooth.opened.lock().unwrap().len(), 1);
    assert_eq!(world.bluetooth.closed.lock().unwrap().len(), 1);

    // A service that was found needs no refresh.
    assert!(world.bluetooth.refreshed.lock().unwrap().is_empty());

    // A paired phone that granted everything needs no request before the side channel.
    assert!(sent(&world.h).is_empty());
}

/// A phone set up on this Mac that answers with its hotspot's credentials.
fn answering_world() -> World {
    let auth = Arc::new(PhoneAuth::default());
    let world = set_up(world_with(auth.clone()));
    world.bluetooth.set_answer(phone_reserves(auth));
    world
}

fn joined(world: &World) -> bool {
    world.hotspot.view(&phone()).expect("the selected phone").joined
}

#[tokio::test(start_paused = true)]
async fn a_connect_joins_and_reads_as_joined_while_the_mac_stays_on_the_network() {
    let world = answering_world();
    assert!(!joined(&world));

    assert_eq!(world.hotspot.connect(&phone()).await, Ok(SSID.to_string()));
    assert_eq!(*world.wifi.joins.lock().unwrap(), [(SSID.to_string(), PASSPHRASE.to_string())]);
    assert_eq!(world.h.app.phone_record().unwrap().hotspot.ssid.as_deref(), Some(SSID));
    assert!(joined(&world));

    // The Mac moves to another network: no longer joined; back on the hotspot: joined again.
    *world.wifi.current.lock().unwrap() = Some("Office".into());
    assert!(!joined(&world));
    *world.wifi.current.lock().unwrap() = Some(SSID.into());
    assert!(joined(&world));
}

#[tokio::test(start_paused = true)]
async fn a_network_of_the_same_name_reads_as_joined_only_after_a_connect() {
    let world = set_up_world();
    *world.wifi.current.lock().unwrap() = Some(SSID.into());
    assert!(!joined(&world));
}

#[tokio::test(start_paused = true)]
async fn disconnect_leaves_the_hotspot_and_keeps_its_name() {
    let world = answering_world();
    world.hotspot.connect(&phone()).await.unwrap();

    assert_eq!(world.hotspot.disconnect(&phone()).await, Ok(()));
    assert_eq!(world.wifi.leaves.load(Ordering::SeqCst), 1);
    assert!(!joined(&world));
    assert_eq!(world.h.app.phone_record().unwrap().hotspot.ssid.as_deref(), Some(SSID));

    // Not on the hotspot any more: a second disconnect leaves nothing.
    assert_eq!(world.hotspot.disconnect(&phone()).await, Ok(()));
    assert_eq!(world.wifi.leaves.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn disconnect_never_leaves_another_network() {
    let world = answering_world();
    world.hotspot.connect(&phone()).await.unwrap();
    *world.wifi.current.lock().unwrap() = Some("Office".into());

    assert_eq!(world.hotspot.disconnect(&phone()).await, Ok(()));
    assert_eq!(world.wifi.leaves.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn a_failed_leave_is_reported() {
    let world = answering_world();
    world.hotspot.connect(&phone()).await.unwrap();
    *world.wifi.leave_result.lock().unwrap() = Err(WifiError::Failed { reason: "busy".into() });

    assert_eq!(world.hotspot.disconnect(&phone()).await, Err(HotspotFailure::LeaveFailed("busy".into())));
    assert!(joined(&world));
}
