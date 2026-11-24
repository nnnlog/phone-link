//! The Bluetooth module's unit tests: device and name matching, the watcher's copy of `BlueZ`'s
//! objects, the profile handler, bounded calls and the pairing agent.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io::{ErrorKind, Read};
use std::os::unix::net::UnixStream;
use std::pin::pin;
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
use std::time::{Duration, Instant};

use phonelink_core::sync::MutexExt;
use phonelink_messaging::ProximityState;
use zbus::zvariant::{ObjectPath, OwnedFd, OwnedObjectPath, OwnedValue, Str, Value};

use super::bluez::{ADAPTER, AGENT_MANAGER, DEVICE};
use super::deadline::{Bounded, call_until};
use super::names::names_match;
use super::objects::{
    Objects, Properties, bool_of, chosen_adapter, discovered_phone, engine_address, paired_phone_device, proximity_of,
};
use super::pairing::{AgentError, PairingAgent};
use super::profile::{
    HANDSFREE, HANDSFREE_AUDIO_GATEWAY, Opened, ProfileHandler, Registrations, Waiter, Waiters, deliver,
    registration_for, stop_waiting, waiter_key,
};
use super::rfcomm::{Channel, Channels};
use super::watch::{add_interfaces, change_properties, mirrored, remove_interfaces};
use super::{answer_pairing_question, on_pairing_question};

const ADAPTER_PATH: &str = "/org/bluez/hci0";
const OTHER_ADAPTER_PATH: &str = "/org/bluez/hci1";
const DEVICE_PATH: &str = "/org/bluez/hci0/dev_A4_5E_60_0B_1C_2D";
const SIDE_CHANNEL: &str = "111fab8f-00a2-4935-b6b4-aca388cdd473";

fn object_path(path: &str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(path).unwrap()
}

fn adapter(powered: bool) -> HashMap<String, Properties> {
    HashMap::from([(ADAPTER.to_owned(), HashMap::from([("Powered".to_owned(), OwnedValue::from(powered))]))])
}

/// A Bluetooth Classic device on `hci0`.
fn device_properties(name: &str, paired: bool, connected: bool) -> Properties {
    HashMap::from([
        ("Name".to_owned(), OwnedValue::from(Str::from(name))),
        ("Address".to_owned(), OwnedValue::from(Str::from("A4:5E:60:0B:1C:2D"))),
        ("Adapter".to_owned(), OwnedValue::from(ObjectPath::try_from(ADAPTER_PATH).unwrap())),
        ("Class".to_owned(), OwnedValue::from(0x5a020cu32)),
        ("Paired".to_owned(), OwnedValue::from(paired)),
        ("Connected".to_owned(), OwnedValue::from(connected)),
    ])
}

fn device(name: &str, paired: bool, connected: bool) -> HashMap<String, Properties> {
    HashMap::from([(DEVICE.to_owned(), device_properties(name, paired, connected))])
}

fn with(
    mut interfaces: HashMap<String, Properties>,
    name: &str,
    value: Option<OwnedValue>,
) -> HashMap<String, Properties> {
    let properties = interfaces.get_mut(DEVICE).unwrap();
    match value {
        Some(value) => properties.insert(name.to_owned(), value),
        None => properties.remove(name),
    };
    interfaces
}

fn objects(entries: Vec<(&str, HashMap<String, Properties>)>) -> Objects {
    entries.into_iter().map(|(path, interfaces)| (object_path(path), interfaces)).collect()
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| (*name).to_owned()).collect()
}

fn handler(waiters: &Waiters, service: &str) -> ProfileHandler {
    ProfileHandler {
        waiters: waiters.clone(),
        channels: Channels::default(),
        registrations: Registrations::default(),
        remote_service: service.to_owned(),
    }
}

fn waiter(waiters: &Waiters, service: &str, id: u64) -> mpsc::Receiver<Opened> {
    let (sender, receiver) = mpsc::channel();
    waiters.lock_or_panic().insert(waiter_key(DEVICE_PATH, service), Waiter { id, sender });
    receiver
}

fn fd_of(stream: UnixStream) -> OwnedFd {
    OwnedFd::from(std::os::fd::OwnedFd::from(stream))
}

/// The far end of a connection reads its end: it was shut down here.
fn was_shut_down(phone: &mut UnixStream) -> bool {
    phone.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    phone.read(&mut [0; 1]).unwrap() == 0
}

/// Runs `future` on this thread until it is ready.
fn block_on<F: Future>(future: F) -> F::Output {
    struct Unpark(thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => thread::park(),
        }
    }
}

#[test]
fn addresses_use_the_engine_form() {
    assert_eq!(engine_address("A4:5E:60:0B:1C:2D"), "a4-5e-60-0b-1c-2d");
}

#[test]
fn the_gateway_is_reached_through_a_hands_free_registration_naming_its_features() {
    let (uuid, options) = registration_for(HANDSFREE_AUDIO_GATEWAY);
    assert_eq!(uuid, HANDSFREE);
    assert_eq!(options.get("Features"), Some(&Value::from(0x0026u16)));
    assert_eq!(options.get("Role"), None);

    let (uuid, options) = registration_for(SIDE_CHANNEL);
    assert_eq!(uuid, SIDE_CHANNEL);
    assert_eq!(options.get("Role"), Some(&Value::from("client")));
    assert_eq!(options.get("Features"), None);

    for (_, options) in [registration_for(HANDSFREE_AUDIO_GATEWAY), registration_for(SIDE_CHANNEL)] {
        assert_eq!(options.get("AutoConnect"), Some(&Value::from(false)));
        assert_eq!(options.get("RequireAuthentication"), Some(&Value::from(true)));
    }
}

#[test]
fn a_connection_reaches_the_open_waiting_for_its_service() {
    let waiters = Waiters::default();
    let hands_free = waiter(&waiters, HANDSFREE_AUDIO_GATEWAY, 1);
    let side_channel = waiter(&waiters, SIDE_CHANNEL, 2);

    let (ours, _phone) = UnixStream::pair().unwrap();
    handler(&waiters, HANDSFREE_AUDIO_GATEWAY).new_connection(object_path(DEVICE_PATH), fd_of(ours), HashMap::new());

    assert!(matches!(hands_free.try_recv(), Ok(Ok(_))));
    assert!(side_channel.try_recv().is_err());
    assert!(waiters.lock_or_panic().contains_key(&waiter_key(DEVICE_PATH, SIDE_CHANNEL)));

    let (ours, _phone) = UnixStream::pair().unwrap();
    handler(&waiters, SIDE_CHANNEL).new_connection(object_path(DEVICE_PATH), fd_of(ours), HashMap::new());
    assert!(matches!(side_channel.try_recv(), Ok(Ok(_))));
    assert!(waiters.lock_or_panic().is_empty());
}

#[test]
fn a_connection_no_open_waits_for_is_shut_down() {
    let waiters = Waiters::default();
    let side_channel = waiter(&waiters, SIDE_CHANNEL, 1);

    let (ours, mut phone) = UnixStream::pair().unwrap();
    // A second descriptor of the same socket, like the one BlueZ keeps.
    let kept = ours.try_clone().unwrap();
    handler(&waiters, HANDSFREE_AUDIO_GATEWAY).new_connection(object_path(DEVICE_PATH), fd_of(ours), HashMap::new());

    assert!(was_shut_down(&mut phone));
    assert!(side_channel.try_recv().is_err());
    drop(kept);
}

#[test]
fn an_open_that_gives_up_shuts_down_a_connection_that_came_meanwhile() {
    let waiters = Waiters::default();
    let key = waiter_key(DEVICE_PATH, SIDE_CHANNEL);
    let receiver = waiter(&waiters, SIDE_CHANNEL, 7);

    // The connection arrives just as the wait ends.
    let (ours, mut phone) = UnixStream::pair().unwrap();
    let kept = ours.try_clone().unwrap();
    assert!(deliver(&waiters, &key, fd_of(ours)));
    stop_waiting(&waiters, &key, 7, &receiver);
    assert!(was_shut_down(&mut phone));

    // One that arrives later finds no waiter.
    let (ours, mut late) = UnixStream::pair().unwrap();
    assert!(!deliver(&waiters, &key, fd_of(ours)));
    assert!(was_shut_down(&mut late));
    drop(kept);
}

#[test]
fn an_open_gives_up_only_its_own_wait() {
    let waiters = Waiters::default();
    let key = waiter_key(DEVICE_PATH, SIDE_CHANNEL);
    let earlier = waiter(&waiters, SIDE_CHANNEL, 1);
    let _later = waiter(&waiters, SIDE_CHANNEL, 2);
    stop_waiting(&waiters, &key, 1, &earlier);
    assert_eq!(waiters.lock_or_panic().get(&key).map(|waiter| waiter.id), Some(2));
}

#[test]
fn a_disconnection_closes_the_channels_of_that_service_on_that_device() {
    let channels = Channels::default();
    let mut phones = Vec::new();
    for (index, (device, service)) in [
        (DEVICE_PATH, SIDE_CHANNEL),
        (DEVICE_PATH, HANDSFREE_AUDIO_GATEWAY),
        ("/org/bluez/hci0/dev_00_11_22_33_44_55", SIDE_CHANNEL),
    ]
    .into_iter()
    .enumerate()
    {
        let (ours, phone) = UnixStream::pair().unwrap();
        phones.push(phone);
        channels
            .lock_or_panic()
            .insert(index as u64, Channel { device: device.to_owned(), service: service.to_owned(), stream: ours });
    }
    let handler = ProfileHandler { channels: channels.clone(), ..handler(&Waiters::default(), SIDE_CHANNEL) };
    handler.request_disconnection(object_path(DEVICE_PATH));

    assert!(was_shut_down(&mut phones[0]));
    for phone in &mut phones[1..] {
        phone.set_nonblocking(true).unwrap();
        let still_open = matches!(phone.read(&mut [0; 1]), Err(e) if e.kind() == ErrorKind::WouldBlock);
        assert!(still_open);
    }
    assert_eq!(channels.lock_or_panic().len(), 3, "a closed channel stays until the engine closes it");
}

#[test]
fn a_released_registration_is_made_again_on_the_next_open() {
    let registrations = Registrations::default();
    registrations.lock_or_panic().extend([HANDSFREE_AUDIO_GATEWAY.to_owned(), SIDE_CHANNEL.to_owned()]);
    let handler = ProfileHandler { registrations: registrations.clone(), ..handler(&Waiters::default(), SIDE_CHANNEL) };
    handler.release();
    assert_eq!(*registrations.lock_or_panic(), [HANDSFREE_AUDIO_GATEWAY]);
}

#[test]
fn proximity_comes_from_the_adapter_and_the_paired_phone() {
    let names = names(&["Pixel 8"]);
    let state = |entries| proximity_of(&objects(entries), &names);

    assert_eq!(state(vec![(DEVICE_PATH, device("Pixel 8", true, true))]), ProximityState::BluetoothOff);
    assert_eq!(
        state(vec![(ADAPTER_PATH, adapter(false)), (DEVICE_PATH, device("Pixel 8", true, true))]),
        ProximityState::BluetoothOff
    );
    assert_eq!(
        state(vec![
            (ADAPTER_PATH, adapter(true)),
            ("/org/bluez/hci0/dev_A", device("Pixel 8", false, true)),
            ("/org/bluez/hci0/dev_B", device("Headphones", true, true)),
        ]),
        ProximityState::PhoneNotPaired
    );
    assert_eq!(
        state(vec![(ADAPTER_PATH, adapter(true)), (DEVICE_PATH, device(" pixel 8 ", true, true))]),
        ProximityState::Connected
    );
    assert_eq!(
        state(vec![(ADAPTER_PATH, adapter(true)), (DEVICE_PATH, device("Pixel 8", true, false))]),
        ProximityState::NotConnected
    );
}

#[test]
fn the_phone_is_a_bluetooth_classic_device() {
    let names = names(&["Pixel 8"]);
    let low_energy_only = with(device("Pixel 8", true, true), "Class", None);
    let objects = objects(vec![(ADAPTER_PATH, adapter(true)), (DEVICE_PATH, low_energy_only)]);
    assert_eq!(paired_phone_device(&objects, &names), None);
    assert_eq!(proximity_of(&objects, &names), ProximityState::PhoneNotPaired);
}

#[test]
fn the_same_paired_phone_is_picked_every_time() {
    let names = names(&["Pixel 8"]);
    let objects = objects(vec![
        (ADAPTER_PATH, adapter(true)),
        ("/org/bluez/hci0/dev_B", device("Pixel 8", true, false)),
        ("/org/bluez/hci0/dev_A", device("Pixel 8", true, true)),
    ]);
    let picked = paired_phone_device(&objects, &names).unwrap();
    assert!(bool_of(picked, "Connected"));
}

#[test]
fn a_powered_adapter_comes_first_then_the_lowest_path() {
    let both_on = objects(vec![("/org/bluez/hci1", adapter(true)), (ADAPTER_PATH, adapter(true))]);
    assert_eq!(chosen_adapter(&both_on).map(|(path, _)| path.as_str()), Some(ADAPTER_PATH));
    let first_off = objects(vec![("/org/bluez/hci1", adapter(true)), (ADAPTER_PATH, adapter(false))]);
    assert_eq!(chosen_adapter(&first_off).map(|(path, _)| path.as_str()), Some("/org/bluez/hci1"));
    assert_eq!(chosen_adapter(&objects(vec![(DEVICE_PATH, device("Pixel 8", true, true))])), None);
}

#[test]
fn devices_on_another_adapter_are_not_used() {
    let names = names(&["Pixel 8"]);
    let elsewhere = with(
        device("Pixel 8", true, true),
        "Adapter",
        Some(OwnedValue::from(ObjectPath::try_from(OTHER_ADAPTER_PATH).unwrap())),
    );
    let objects = objects(vec![
        (ADAPTER_PATH, adapter(true)),
        (OTHER_ADAPTER_PATH, adapter(false)),
        ("/org/bluez/hci1/dev_A4_5E_60_0B_1C_2D", elsewhere),
    ]);
    assert_eq!(proximity_of(&objects, &names), ProximityState::PhoneNotPaired);
}

#[test]
fn discovery_takes_a_classic_device_this_scan_saw() {
    let names = names(&["Pixel 8"]);
    let rssi = Some(OwnedValue::from(-60i16));
    let known: HashSet<String> =
        ["/org/bluez/hci0/dev_A", "/org/bluez/hci0/dev_B", "/org/bluez/hci0/dev_C"].map(str::to_owned).into();
    let found = |entries| {
        let objects = objects(entries);
        discovered_phone(&objects, ADAPTER_PATH, &known, &names).map(|(path, _)| path.to_string())
    };

    // Known before the scan and not seen by it, or seen only over Low Energy: not the phone.
    assert_eq!(found(vec![("/org/bluez/hci0/dev_A", device("Pixel 8", false, false))]), None);
    assert_eq!(
        found(vec![(
            "/org/bluez/hci0/dev_A",
            with(with(device("Pixel 8", false, false), "RSSI", rssi.clone()), "Class", None)
        )]),
        None
    );
    // Seen by this scan, or new during it.
    assert_eq!(
        found(vec![("/org/bluez/hci0/dev_B", with(device("Pixel 8", false, false), "RSSI", rssi.clone()))]).as_deref(),
        Some("/org/bluez/hci0/dev_B")
    );
    assert_eq!(
        found(vec![("/org/bluez/hci0/dev_N", device("pixel 8", false, false))]).as_deref(),
        Some("/org/bluez/hci0/dev_N")
    );
    // With several, the lowest path.
    assert_eq!(
        found(vec![
            ("/org/bluez/hci0/dev_Z", device("Pixel 8", false, false)),
            ("/org/bluez/hci0/dev_C", with(device("Pixel 8", false, false), "RSSI", rssi)),
        ])
        .as_deref(),
        Some("/org/bluez/hci0/dev_C")
    );
}

#[test]
fn names_match_ignoring_case_white_space_and_composition() {
    assert!(names_match(" pixel 8 ", "Pixel 8"));
    assert!(names_match("STRASSE", "Straße"));
    assert!(names_match("STRAẞE", "straße"));
    assert!(names_match("ΣΊΣΥΦΟΣ", "σίσυφος"));
    // Composed and decomposed é, and Hangul syllables and their jamo.
    assert!(names_match("Am\u{e9}lie", "AME\u{301}LIE"));
    assert!(names_match(
        "\u{ac24}\u{b7ed}\u{c2dc}",
        "\u{1100}\u{1162}\u{11af}\u{1105}\u{1165}\u{11a8}\u{1109}\u{1175}"
    ));
    assert!(!names_match("Pixel 8", "Pixel 8 Pro"));
    assert!(!names_match("ı", "i"), "the dotless i is its own letter");
    assert!(!names_match("  ", "  "), "an empty name matches nothing");
    assert!(!names_match("", "Pixel"));
}

#[test]
fn the_mirror_follows_bluez_signals() {
    let names = names(&["Pixel 8"]);
    let mut objects = mirrored(objects(vec![
        (ADAPTER_PATH, adapter(true)),
        ("/org/bluez", HashMap::from([(AGENT_MANAGER.to_owned(), Properties::new())])),
    ]));
    assert_eq!(objects.len(), 1, "only adapters and devices are kept");
    assert_eq!(proximity_of(&objects, &names), ProximityState::PhoneNotPaired);

    add_interfaces(&mut objects, object_path(DEVICE_PATH), device("Pixel 8", false, false));
    assert_eq!(proximity_of(&objects, &names), ProximityState::PhoneNotPaired);

    let paired = HashMap::from([("Paired".to_owned(), OwnedValue::from(true))]);
    change_properties(&mut objects, &object_path(DEVICE_PATH), DEVICE, paired, &[]);
    assert_eq!(proximity_of(&objects, &names), ProximityState::NotConnected);

    let connected = HashMap::from([
        ("Connected".to_owned(), OwnedValue::from(true)),
        ("RSSI".to_owned(), OwnedValue::from(-50i16)),
    ]);
    change_properties(&mut objects, &object_path(DEVICE_PATH), DEVICE, connected, &[]);
    assert_eq!(proximity_of(&objects, &names), ProximityState::Connected);

    change_properties(&mut objects, &object_path(DEVICE_PATH), DEVICE, Properties::new(), &["RSSI".to_owned()]);
    assert!(!objects[&object_path(DEVICE_PATH)][DEVICE].contains_key("RSSI"));

    let off = HashMap::from([("Powered".to_owned(), OwnedValue::from(false))]);
    change_properties(&mut objects, &object_path(ADAPTER_PATH), ADAPTER, off, &[]);
    assert_eq!(proximity_of(&objects, &names), ProximityState::BluetoothOff);

    // A change for an object the mirror does not have adds nothing.
    let unknown = HashMap::from([("Connected".to_owned(), OwnedValue::from(true))]);
    change_properties(&mut objects, &object_path("/org/bluez/hci0/dev_X"), DEVICE, unknown, &[]);
    assert!(!objects.contains_key(&object_path("/org/bluez/hci0/dev_X")));

    remove_interfaces(&mut objects, &object_path(DEVICE_PATH), &[DEVICE.to_owned()]);
    assert!(!objects.contains_key(&object_path(DEVICE_PATH)));
}

#[test]
fn a_bounded_call_ends_at_its_deadline() {
    let soon = Instant::now() + Duration::from_secs(5);
    assert_eq!(call_until(soon, "quick", || 7), Ok(Bounded::Finished(7)));

    let started = Instant::now();
    let deadline = started + Duration::from_millis(50);
    let outcome = call_until(deadline, "slow", || thread::sleep(Duration::from_secs(2)));
    assert_eq!(outcome, Ok(Bounded::DeadlinePassed));
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn the_agent_asks_the_user_and_answers_bluez() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let record = asked.clone();
    on_pairing_question(move |code| record.lock_or_panic().push(code));
    let agent = PairingAgent { device: DEVICE_PATH.to_owned() };

    // Another device is refused without asking.
    let other = block_on(agent.request_confirmation(object_path("/org/bluez/hci0/dev_X"), 1));
    assert!(matches!(other, Err(AgentError::Rejected(_))));
    assert!(asked.lock_or_panic().is_empty());

    // The user confirms the code.
    let confirming = thread::spawn(move || {
        let agent = PairingAgent { device: DEVICE_PATH.to_owned() };
        block_on(agent.request_confirmation(object_path(DEVICE_PATH), 42))
    });
    while asked.lock_or_panic().is_empty() {
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(*asked.lock_or_panic(), [Some("000042".to_owned())]);
    answer_pairing_question(true);
    assert!(confirming.join().unwrap().is_ok());

    // BlueZ cancels the question: it closes, and the request ends unanswered.
    let cancelled = thread::spawn(move || {
        let agent = PairingAgent { device: DEVICE_PATH.to_owned() };
        block_on(agent.request_confirmation(object_path(DEVICE_PATH), 123_456))
    });
    while asked.lock_or_panic().len() < 2 {
        thread::sleep(Duration::from_millis(5));
    }
    agent.cancel();
    assert!(matches!(cancelled.join().unwrap(), Err(AgentError::Canceled(_))));
    assert_eq!(asked.lock_or_panic()[1..], [Some("123456".to_owned()), None]);

    // An answer with no question open goes nowhere; requests other than a confirmation are refused.
    answer_pairing_question(false);
    assert!(matches!(agent.request_passkey(object_path(DEVICE_PATH)), Err(AgentError::Rejected(_))));
    assert!(matches!(
        agent.authorize_service(object_path(DEVICE_PATH), HANDSFREE.to_owned()),
        Err(AgentError::Rejected(_))
    ));
}
