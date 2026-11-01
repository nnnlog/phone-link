use super::*;

#[test]
fn configuration_keys() {
    let off = configuration(&Predicates::new(0, 3.99));
    assert_eq!(
        off.keys().collect::<Vec<_>>(),
        ["MessagingSync", "MessagingMassText", "MessagingSend", "MessagingRcsReceive", "MessagingRcsSend"]
    );
    assert_eq!(off.bool("MessagingRcsReceive"), Some(false));

    let on = configuration(&Predicates::new(0x2 | 0x4 | 0x10, 3.99));
    assert_eq!(on.bool("MessagingSend"), Some(true));
    assert_eq!(on.bool("MessagingRcsSend"), Some(true));
    assert_eq!(on.i32_array("MessagingRcsUnsupportedMessageTypes"), Some([10, 9].as_slice()));
}

#[test]
fn permission_table() {
    assert_eq!(permission_from_outcome(&AppOutcome::Success), Some(true));
    assert_eq!(permission_from_outcome(&AppOutcome::Accepted), Some(true));
    assert_eq!(permission_from_outcome(&AppOutcome::Denied), Some(false));
    assert_eq!(permission_from_outcome(&AppOutcome::PermanentlyDenied), Some(false));
    assert_eq!(permission_from_outcome(&AppOutcome::NeedsPermission), Some(false));
    assert_eq!(permission_from_outcome(&AppOutcome::NeedsInitialPermission), None);
    assert_eq!(permission_from_outcome(&AppOutcome::TimedOut), None);
    assert_eq!(permission_from_outcome(&AppOutcome::Failure), None);
}
