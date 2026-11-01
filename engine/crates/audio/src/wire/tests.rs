use super::*;
use phonelink_core::codec::valueset::Value;

#[test]
fn command_body_and_options() {
    let req = command_request(AudioCommand::Pause, "0|com.example.player|1|null|10123", "PauseButtonPressed");

    assert_eq!(req.route, "/command/audio");
    assert_eq!(req.payload.keys().collect::<Vec<_>>(), ["correlationVector", "audioCommandValue", "key"]);
    assert_eq!(req.payload.get("audioCommandValue"), Some(&Value::Int32(2)));
    assert_eq!(
        req.payload.get("correlationVector"),
        Some(&Value::String(req.trace.as_ref().unwrap().correlation_id.clone()))
    );
    assert!(!req.options.user_initiated);
    assert_eq!(req.options.priority, 100);
}

#[test]
fn command_values() {
    let values: Vec<i32> = [AudioCommand::Play, AudioCommand::Pause, AudioCommand::Next, AudioCommand::Previous]
        .map(AudioCommand::value)
        .into();
    assert_eq!(values, [1, 2, 3, 4]);
}
