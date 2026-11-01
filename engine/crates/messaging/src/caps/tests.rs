use super::*;

#[test]
fn before_any_report() {
    assert_eq!(default_at_load(true, 3.99), 0, "the pairing run");
    assert_eq!(default_at_load(false, 3.99), 0x2, "a later run");
    assert_eq!(default_at_load(false, 3.89), 0);
    assert_eq!(default_at_load(false, 3.9), 0x2);
    assert_eq!(effective(None, 0x2), 0x2, "nothing stored: the run's default");
    assert_eq!(effective(Some(0x12), 0), 0x12, "a stored value wins");
}

#[test]
fn predicates() {
    let p = Predicates::new(0xFF, 3.99);
    assert!(p.rcs_receive() && p.rcs_send() && p.mark_conversation_as_read());
    assert!(p.participants() && p.send() && p.requires_proximity() && p.mms_auto_download_on_device());

    let old = Predicates::new(0xFF, 3.8);
    assert!(!old.rcs_receive() && !old.rcs_send() && !old.mark_conversation_as_read());
    assert!(old.participants(), "participants does not depend on the contract version");

    let no_receive = Predicates::new(caps::RCS_SEND, 3.99);
    assert!(!no_receive.rcs_send(), "RCS send needs RCS receive");
    assert!(!Predicates::new(0, 3.99).rcs_receive());
}

#[test]
fn settings_and_connect_force_rcs_receive() {
    let s = PhoneSettings { capabilities: 0x20, rcs_send_available: true, ..Default::default() };
    assert_eq!(s.stored_capabilities(), 0x20 | 0x2 | 0x4);

    let s = PhoneSettings {
        capabilities: 0,
        mms_auto_download_available: true,
        mark_as_read_available: true,
        ..Default::default()
    };
    assert_eq!(s.stored_capabilities(), 0x1 | 0x2 | 0x8);

    assert_eq!(from_connect(0x30), 0x32);
    assert_eq!(from_connect(-1), u32::MAX);
}
