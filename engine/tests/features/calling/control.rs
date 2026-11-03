//! Call control against a scripted phone: the gates and the connection states, the setup's
//! triggers, the calls from the phone's call list with their contacts, and each operation's AT
//! command.

use crate::support::app_world;
use crate::support::calling_world;
use crate::support::common::eventually;

use app_world::*;
use calling_world::*;
use phonelink_engine::features::bluetooth::RfcommError;
use phonelink_engine::features::calling::control::{
    AG_SERVICE_UUID, CallRole, CallStatus, CallView, ControlError, ControlStatus, DialFailure,
};

fn status(world: &World) -> ControlStatus {
    world.calling.control().state(&phone()).unwrap().status
}

fn calls(world: &World) -> Vec<CallView> {
    world.calling.control().calls(&phone())
}

/// History on, call control switched on, connected.
async fn ready() -> World {
    let world = world();
    world.grant();
    world.calling.control().set_enabled(&phone(), true).unwrap();
    assert!(eventually(|| status(&world) == ControlStatus::Ready).await, "not ready: {:?}", status(&world));
    world
}

/// The commands after the service-level connection and the first call list.
fn operations(world: &World) -> Vec<String> {
    let all = world.ag.commands();
    let first_list = all.iter().position(|c| c == "AT+CLCC").expect("the first call list");
    all[first_list + 1..].iter().filter(|c| *c != "AT+CLCC").cloned().collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn switching_call_control_on_connects_to_the_audio_gateway() {
    let world = ready().await;

    assert_eq!(world.ag.opened(), [AG_SERVICE_UUID]);
    assert_eq!(
        world.ag.commands(),
        ["AT+BRSF=102", "AT+CIND=?", "AT+CIND?", "AT+CMER=3,0,0,1", "AT+CHLD=?", "AT+CLIP=1", "AT+CCWA=1", "AT+CLCC"]
    );
    let state = world.calling.control().state(&phone()).unwrap();
    assert!(state.offered && state.pc_consent && state.enabled);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_call_history_on_the_switch_connects_nothing_and_turning_history_off_disconnects() {
    let world = world();
    world.calling.control().set_enabled(&phone(), true).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(status(&world), ControlStatus::Off);
    assert!(world.ag.opened().is_empty());

    world.grant();
    assert!(eventually(|| status(&world) == ControlStatus::Ready).await);

    world.calling.set_pc_consent(&phone(), false).unwrap();
    assert!(eventually(|| status(&world) == ControlStatus::Off).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn bluetooth_off_and_an_unpaired_phone_are_states_and_pairing_connects() {
    let world = world();
    world.grant();
    world.ag.set_radio(false);
    world.calling.control().set_enabled(&phone(), true).unwrap();
    assert!(eventually(|| status(&world) == ControlStatus::BluetoothOff).await);

    world.ag.set_radio(true);
    world.ag.set_paired(false);
    assert_eq!(world.calling.control().connect().await, ControlStatus::NotPaired);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_service_record_is_read_again_once() {
    let world = world();
    world.grant();
    world.ag.fail_opens(vec![RfcommError::ServiceNotFound]);
    world.calling.control().set_enabled(&phone(), true).unwrap();

    assert!(eventually(|| status(&world) == ControlStatus::Ready).await);
    assert_eq!(world.ag.opened().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_connection_is_tried_again_when_the_app_comes_forward() {
    let world = world();
    world.grant();
    // The record changes of the setup have set up (nothing, call control being off) by now.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    world.ag.fail_opens(vec![RfcommError::Unreachable]);
    world.calling.control().set_enabled(&phone(), true).unwrap();
    assert!(eventually(|| status(&world) == ControlStatus::ConnectionFailed).await);

    world.calling.foreground();
    assert!(eventually(|| status(&world) == ControlStatus::Ready).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lost_channel_ends_the_calls_until_the_connection_is_set_up_again() {
    let world = ready().await;
    world.ag.ring("+15550100");
    assert!(eventually(|| calls(&world).len() == 1).await);

    world.ag.drop_channel();
    assert!(eventually(|| status(&world) == ControlStatus::NotConnected).await);
    assert!(calls(&world).is_empty());

    world.calling.foreground();
    assert!(eventually(|| status(&world) == ControlStatus::Ready).await);
    assert!(eventually(|| calls(&world).len() == 1).await, "the ringing call is listed again");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_incoming_call_shows_its_contact_and_answering_tracks_it() {
    let world = ready().await;
    world.ag.ring("+15550100");
    assert!(eventually(|| calls(&world).len() == 1).await);

    let view = calls(&world).remove(0);
    assert_eq!(
        (view.call.status, view.call.role, view.call.waiting),
        (CallStatus::Incoming, CallRole::Untracked, false)
    );
    assert_eq!(view.contact.and_then(|c| c.display_name).as_deref(), Some("Ada"));

    world.calling.control().answer(view.call.id).await.unwrap();
    assert_eq!(operations(&world), ["ATA"]);
    assert!(eventually(|| calls(&world).first().is_some_and(|c| c.call.status == CallStatus::Talking)).await);
    let call = calls(&world).remove(0).call;
    assert_eq!(call.role, CallRole::Primary);
    assert!(call.talking_since_ms.is_some());

    world.calling.control().end(call.id).await.unwrap();
    assert_eq!(operations(&world), ["ATA", "AT+CHUP"]);
    assert!(eventually(|| calls(&world).is_empty()).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn declining_hangs_up_a_ringing_call_and_rejects_a_waiting_one() {
    let world = ready().await;
    world.ag.ring("+15550100");
    assert!(eventually(|| calls(&world).len() == 1).await);
    let id = calls(&world)[0].call.id;
    world.calling.control().decline(id).await.unwrap();
    assert!(eventually(|| calls(&world).is_empty()).await);

    world.calling.control().dial("5550101").await.unwrap();
    world.ag.remote_answers();
    world.ag.ring("+15550102");
    assert!(eventually(|| calls(&world).iter().any(|c| c.call.waiting)).await);
    let waiting = calls(&world).into_iter().find(|c| c.call.waiting).unwrap().call;
    world.calling.control().decline(waiting.id).await.unwrap();

    assert_eq!(operations(&world), ["AT+CHUP", "ATD5550101;", "AT+CHLD=0"]);
    assert!(eventually(|| calls(&world).len() == 1).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dial_is_normalized_and_the_dialed_call_is_tracked() {
    let world = ready().await;
    world.calling.control().dial("+1 (555) 010-01AB").await.unwrap();

    assert_eq!(operations(&world), ["ATD+155501001 22;".replace(' ', "")]);
    assert!(eventually(|| calls(&world).first().is_some_and(|c| c.call.role == CallRole::Primary)).await);
    assert_eq!(calls(&world)[0].call.status, CallStatus::Dialing);

    assert_eq!(world.calling.control().dial(" ()-").await, Err(ControlError::EmptyNumber));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_dial_is_worded_from_the_phone_status() {
    let world = ready().await;
    world.ag.refuse("ATD");
    assert_eq!(world.calling.control().dial("5550101").await, Err(ControlError::DialFailed(DialFailure::Failed)));

    world.h.app.update_record_for(&phone(), |r| r.status_keys.network_signal_strength = Some(-1));
    assert_eq!(world.calling.control().dial("5550101").await, Err(ControlError::DialFailed(DialFailure::NoNetwork)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_waiting_call_answered_holds_the_first_and_ending_the_held_one_resumes_it_first() {
    let world = ready().await;
    world.calling.control().dial("5550101").await.unwrap();
    world.ag.remote_answers();
    world.ag.ring("+15550100");
    assert!(eventually(|| calls(&world).iter().any(|c| c.call.waiting)).await);
    let first = calls(&world).into_iter().find(|c| !c.call.incoming).unwrap().call;
    let waiting = calls(&world).into_iter().find(|c| c.call.waiting).unwrap().call;

    world.calling.control().answer(waiting.id).await.unwrap();
    assert!(
        eventually(|| {
            let calls = calls(&world);
            calls.iter().any(|c| {
                c.call.id == first.id && c.call.status == CallStatus::Held && c.call.role == CallRole::Secondary
            }) && calls.iter().any(|c| c.call.id == waiting.id && c.call.role == CallRole::Primary)
        })
        .await
    );

    world.calling.control().end(first.id).await.unwrap();
    assert_eq!(operations(&world), ["ATD5550101;", "AT+CHLD=2", "AT+CHLD=2", "AT+CHUP", "AT+CHLD=2"]);
    assert!(
        eventually(|| {
            let calls = calls(&world);
            calls.len() == 1 && calls[0].call.id == waiting.id && calls[0].call.status == CallStatus::Talking
        })
        .await,
        "the other call is resumed and primary"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn operations_need_a_connection_and_a_call_in_the_right_state() {
    let world = world();
    assert_eq!(world.calling.control().dial("1").await, Err(ControlError::NotReady));

    let world = ready().await;
    assert_eq!(world.calling.control().answer(99).await, Err(ControlError::NoSuchCall));
    world.calling.control().dial("5550101").await.unwrap();
    assert!(eventually(|| calls(&world).len() == 1).await);
    let id = calls(&world)[0].call.id;
    assert_eq!(world.calling.control().answer(id).await, Err(ControlError::InvalidState));
    assert_eq!(world.calling.control().swap().await, Err(ControlError::InvalidState));
}
