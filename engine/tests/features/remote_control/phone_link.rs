//! Remote control — the Phone Link contract: `remoteControl` content on `/legacy/phonecontent`, its
//! commands and answers, the value rules of `remoteControlType`, the metadata phase, the per-phone
//! toggle, and the App layer's answer while the Mac is locked. The Mac's actions only reach a
//! recording edge.

use crate::support::platform_world;
use crate::support::remote_control_world as rc;
use phonelink_engine::config::features::RemoteControlConfig;

use phonelink_engine::codec::valueset::{Value, ValueSet};
use phonelink_engine::edge::LockState;
use phonelink_engine::features::remote_control::PowerAction;
use platform_world::phone_id;
use rc::{RcWorld, item_with};

fn command(v: impl Into<Value>) -> ValueSet {
    ValueSet::new().with("remoteControlType", v.into())
}

fn result_only(code: i32) -> ValueSet {
    ValueSet::new().with("result", code)
}

#[tokio::test(start_paused = true)]
async fn each_command_acts_and_answers_result_0() {
    let w = RcWorld::connected().await;
    for (n, action) in
        [(0, PowerAction::LockScreen), (1, PowerAction::Sleep), (2, PowerAction::Restart), (3, PowerAction::ShutDown)]
    {
        let before = w.power.actions().len();
        assert_eq!(w.content(command(n)).await, result_only(0), "{n}");
        assert_eq!(w.power.actions()[before..], [action], "{n}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_failed_action_answers_result_1_without_sub_error() {
    let w = RcWorld::connected().await;
    w.power.set_failing(true);
    assert_eq!(w.content(command(3)).await, result_only(1));
    assert_eq!(w.power.actions(), [PowerAction::ShutDown]);
}

#[tokio::test(start_paused = true)]
async fn values_that_are_not_a_command_answer_result_1_without_acting() {
    let w = RcWorld::connected().await;
    for keys in [ValueSet::new(), command(4), command(-1), command("Lock"), command("shutdown")] {
        assert_eq!(w.content(keys.clone()).await, result_only(1), "{keys:?}");
    }

    // A command named as text, or another numeric type: refused as malformed content.
    for keys in [command("Sleep"), command(Value::Int64(1)), command(Value::UInt8(0)), command(true)] {
        assert_eq!(w.content(keys.clone()).await, result_only(1), "{keys:?}");
    }
    assert!(w.power.actions().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_metadata_phase_answers_empty_and_waits_for_the_command() {
    let w = RcWorld::connected().await;
    let answer = w.content(ValueSet::new().with("metadata", true)).await;
    assert!(answer.is_empty(), "{answer:?}");
    assert!(w.power.actions().is_empty());

    // The content message of the same transaction runs the command.
    assert_eq!(w.content(command(0)).await, result_only(0));
    assert_eq!(w.power.actions(), [PowerAction::LockScreen]);
}

#[tokio::test(start_paused = true)]
async fn the_toggle_off_answers_result_1_without_acting() {
    let w = RcWorld::connected().await;
    w.rc.set_phone_toggle(&phone_id(), false).unwrap();
    assert_eq!(w.content(command(1)).await, result_only(1));
    assert!(w.power.actions().is_empty());
}

#[tokio::test(start_paused = true)]
async fn neither_capability_nor_option_gates_the_processor() {
    let w = RcWorld::new(item_with(&[]), RemoteControlConfig { enabled: false, v2_enabled: false });
    w.sw.w.connect_phone().await;
    assert_eq!(w.content(command(2)).await, result_only(0));
    assert_eq!(w.power.actions(), [PowerAction::Restart]);
    assert_eq!(w.refresh.count(), 0);
}

#[tokio::test(start_paused = true)]
async fn the_locked_mac_answers_19_before_the_processor() {
    let w = RcWorld::connected().await;
    w.sw.w.lock.set(LockState::Locked);
    assert_eq!(w.content(command(0)).await.i32("result"), Some(19));
    assert!(w.power.actions().is_empty());
}
