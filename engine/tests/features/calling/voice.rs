//! Call control on a computer that carries a call's voice, against a scripted phone and voice edge:
//! which calls the computer takes the audio of, the codec the phone selects, and the operations that
//! act on a call heard on the computer.

use std::time::Duration;

use crate::support::app_world;
use crate::support::calling_world;
use crate::support::common::eventually;

use app_world::*;
use calling_world::*;
use phonelink_engine::features::calling::control::testing::ADDRESS;
use phonelink_engine::features::calling::control::voice::VoiceCodec;
use phonelink_engine::features::calling::control::voice::testing::FakeVoice;
use phonelink_engine::features::calling::control::{CallRole, CallStatus, CallView, ControlError, ControlStatus};

fn status(world: &World) -> ControlStatus {
    world.calling.control().state(&phone()).unwrap().status
}

fn calls(world: &World) -> Vec<CallView> {
    world.calling.control().calls(&phone())
}

fn voice(world: &World) -> &FakeVoice {
    world.voice.as_ref().expect("a world with a voice edge")
}

/// History on, call control switched on, connected, on a computer that carries voice.
async fn ready() -> World {
    let world = voice_world();
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

/// A call dialed from the computer and answered by the other party, heard on the computer.
async fn talking_on_the_computer(world: &World) -> u64 {
    world.calling.control().dial("5550101").await.unwrap();
    world.ag.remote_answers();
    assert!(
        eventually(|| calls(world).first().is_some_and(|c| c.call.status == CallStatus::Talking && c.on_computer))
            .await,
        "the dialed call is not on the computer: {:?}",
        calls(world)
    );
    calls(world)[0].call.id
}

#[tokio::test(flavor = "multi_thread")]
async fn the_connection_announces_the_codecs_and_serves_the_phone() {
    let world = ready().await;

    assert_eq!(world.ag.commands()[..3], ["AT+BRSF=230", "AT+BAC=1,2", "AT+CIND=?"]);
    assert_eq!(voice(&world).attached().as_deref(), Some(ADDRESS));
    assert_eq!(voice(&world).expected(), None, "no call yet: the phone's link is declined");
    assert!(world.calling.control().state(&phone()).unwrap().can_take_audio);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_call_answered_from_the_computer_is_heard_on_it_with_the_codec_the_phone_selects() {
    let world = ready().await;
    world.ag.ring("+15550100");
    assert!(eventually(|| calls(&world).len() == 1).await);
    assert!(!voice(&world).phone_opens(VoiceCodec::Cvsd), "a ringing call's link is declined");

    let id = calls(&world)[0].call.id;
    world.calling.control().answer(id).await.unwrap();

    assert!(eventually(|| calls(&world).first().is_some_and(|c| c.on_computer)).await);
    assert_eq!(voice(&world).open(), Some(VoiceCodec::Msbc));
    assert_eq!(operations(&world), ["ATA", "AT+BCS=2"]);
    let view = calls(&world).remove(0);
    assert!(view.controls.mute && view.controls.hold && view.controls.keypad && view.controls.transfer);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_call_taken_on_the_phone_stays_there_until_it_is_transferred() {
    let world = ready().await;
    world.ag.ring("+15550100");
    assert!(eventually(|| calls(&world).len() == 1).await);
    // The user answers on the phone: the phone asks for the link, as it does for every active call.
    world.ag.answered_on_the_phone();
    assert!(eventually(|| calls(&world).first().is_some_and(|c| c.call.status == CallStatus::Talking)).await);
    assert!(!voice(&world).phone_opens(VoiceCodec::Cvsd));

    let view = calls(&world).remove(0);
    assert_eq!((view.call.role, view.on_computer), (CallRole::Untracked, false));
    assert!(view.controls.transfer && !view.controls.mute && !view.controls.hold && !view.controls.keypad);

    world.calling.control().transfer_to_computer(view.call.id).await.unwrap();
    let view = calls(&world).remove(0);
    assert_eq!((view.call.role, view.on_computer), (CallRole::Primary, true), "transferred calls join the window");
    assert_eq!(operations(&world), ["AT+BCC", "AT+BCS=2"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn transferring_to_the_phone_releases_the_link_and_later_links_are_declined() {
    let world = ready().await;
    let id = talking_on_the_computer(&world).await;

    world.calling.control().transfer_to_phone(id).unwrap();
    assert!(eventually(|| calls(&world).first().is_some_and(|c| !c.on_computer)).await);
    assert_eq!(voice(&world).open(), None);
    assert!(!voice(&world).phone_opens(VoiceCodec::Msbc), "the phone's link is declined until a transfer back");
    assert_eq!(calls(&world)[0].call.status, CallStatus::Talking, "the call goes on");

    world.calling.control().transfer_to_computer(id).await.unwrap();
    assert!(calls(&world)[0].on_computer);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_the_phone_releases_leaves_the_call_on_the_phone() {
    let world = ready().await;
    talking_on_the_computer(&world).await;

    voice(&world).phone_closes();
    assert!(eventually(|| calls(&world).first().is_some_and(|c| !c.on_computer)).await);
    assert_eq!(voice(&world).expected(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn mute_holds_until_another_call_or_the_end_of_the_link() {
    let world = ready().await;
    let id = talking_on_the_computer(&world).await;

    world.calling.control().set_muted(id, true).unwrap();
    assert!(voice(&world).muted() && calls(&world)[0].muted);
    world.calling.control().set_muted(id, false).unwrap();
    assert!(!voice(&world).muted() && !calls(&world)[0].muted);

    world.calling.control().set_muted(id, true).unwrap();
    world.calling.control().transfer_to_phone(id).unwrap();
    assert!(eventually(|| !voice(&world).muted()).await, "the mute ends with the link");
    assert_eq!(world.calling.control().set_muted(id, true), Err(ControlError::InvalidState));
}

#[tokio::test(flavor = "multi_thread")]
async fn hold_and_resume_send_the_hold_command_and_a_second_call_disables_them() {
    let world = ready().await;
    let id = talking_on_the_computer(&world).await;

    world.calling.control().hold(id).await.unwrap();
    assert!(eventually(|| calls(&world)[0].call.status == CallStatus::Held).await);
    world.calling.control().hold(id).await.unwrap();
    world.calling.control().resume(id).await.unwrap();
    assert!(eventually(|| calls(&world)[0].call.status == CallStatus::Talking).await);
    assert_eq!(operations(&world), ["ATD5550101;", "AT+BCS=2", "AT+CHLD=2", "AT+CHLD=2"]);

    world.ag.ring("+15550100");
    assert!(eventually(|| calls(&world).iter().any(|c| c.call.waiting)).await);
    let waiting = calls(&world).into_iter().find(|c| c.call.waiting).unwrap().call.id;
    world.calling.control().answer(waiting).await.unwrap();
    assert!(eventually(|| calls(&world).iter().all(|c| c.call.role != CallRole::Untracked)).await);
    assert_eq!(world.calling.control().hold(waiting).await, Err(ControlError::InvalidState));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_keypad_key_is_sent_and_its_tone_played() {
    let world = ready().await;
    let id = talking_on_the_computer(&world).await;

    world.calling.control().send_key(id, '5').await.unwrap();
    world.calling.control().send_key(id, '#').await.unwrap();
    assert_eq!(world.calling.control().send_key(id, 'A').await, Err(ControlError::InvalidState));

    assert_eq!(operations(&world)[2..], ["AT+VTS=5", "AT+VTS=#"]);
    assert_eq!(voice(&world).keys(), ['5', '#']);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_computer_that_cannot_take_audio_leaves_an_answered_call_on_the_phone() {
    let world = ready().await;
    voice(&world).set_can_carry(false);
    world.ag.ring("+15550100");
    assert!(eventually(|| calls(&world).len() == 1).await);
    let id = calls(&world)[0].call.id;

    world.calling.control().answer(id).await.unwrap();
    assert!(eventually(|| calls(&world).first().is_some_and(|c| c.call.status == CallStatus::Talking)).await);
    tokio::time::sleep(Duration::from_millis(100)).await;

    let view = calls(&world).remove(0);
    assert!(!view.on_computer && !view.controls.transfer);
    assert_eq!(voice(&world).declined(), 1, "the phone's link for the answered call");
    assert_eq!(world.calling.control().transfer_to_computer(id).await, Err(ControlError::InvalidState));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_wide_band_link_falls_back_to_narrow_band() {
    let world = ready().await;
    voice(&world).fail(VoiceCodec::Msbc);

    world.calling.control().dial("5550101").await.unwrap();
    assert!(eventually(|| calls(&world).first().is_some_and(|c| c.on_computer)).await);

    assert_eq!(voice(&world).open(), Some(VoiceCodec::Cvsd));
    assert_eq!(operations(&world), ["ATD5550101;", "AT+BCS=2", "AT+BAC=1", "AT+BCC", "AT+BCS=1"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_last_call_ending_gives_the_audio_back() {
    let world = ready().await;
    let id = talking_on_the_computer(&world).await;

    world.calling.control().end(id).await.unwrap();
    assert!(eventually(|| calls(&world).is_empty()).await);
    assert!(eventually(|| voice(&world).open().is_none() && voice(&world).expected().is_none()).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn ending_all_calls_ends_the_main_call_and_then_the_other() {
    let world = ready().await;
    let first = talking_on_the_computer(&world).await;
    world.ag.ring("+15550100");
    assert!(eventually(|| calls(&world).iter().any(|c| c.call.waiting)).await);
    let waiting = calls(&world).into_iter().find(|c| c.call.waiting).unwrap().call.id;
    world.calling.control().answer(waiting).await.unwrap();
    assert!(
        eventually(|| calls(&world).iter().any(|c| c.call.id == first && c.call.role == CallRole::Secondary)).await
    );

    world.calling.control().end_all().await.unwrap();
    assert!(eventually(|| calls(&world).is_empty()).await, "calls left: {:?}", calls(&world));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lost_connection_stops_serving_the_phone() {
    let world = ready().await;
    talking_on_the_computer(&world).await;

    world.ag.drop_channel();
    assert!(eventually(|| status(&world) == ControlStatus::NotConnected).await);
    assert_eq!(voice(&world).attached(), None);
    assert_eq!(voice(&world).open(), None);
}
