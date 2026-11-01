use super::*;

#[test]
fn command_values() {
    let with = |v: Value| ValueSet::new().with(TYPE_KEY, v);

    assert_eq!(command(&ValueSet::new()), Command::Unrecognized);
    assert_eq!(command(&with(Value::Int32(0))), Command::Act(PowerAction::LockScreen));
    assert_eq!(command(&with(Value::Int32(1))), Command::Act(PowerAction::Sleep));
    assert_eq!(command(&with(Value::Int32(2))), Command::Act(PowerAction::Restart));
    assert_eq!(command(&with(Value::Int32(3))), Command::Act(PowerAction::ShutDown));
    assert_eq!(command(&with(Value::Int32(4))), Command::Unrecognized);
    assert_eq!(command(&with(Value::Int32(-1))), Command::Unrecognized);
    assert_eq!(command(&with(Value::String("Lock".into()))), Command::Unrecognized);
    assert!(matches!(command(&with(Value::String("Sleep".into()))), Command::Malformed(_)));
    assert!(matches!(command(&with(Value::Int64(1))), Command::Malformed(_)));
    assert!(matches!(command(&with(Value::UInt8(1))), Command::Malformed(_)));
    assert!(matches!(command(&with(Value::Bool(true))), Command::Malformed(_)));
}
