use super::*;

#[test]
fn a_lock_change_is_told_after_the_first_state() {
    assert_eq!(lock_change(None, true), None);
    assert_eq!(lock_change(None, false), None);
    assert_eq!(lock_change(Some(false), true), Some(SystemEvent::ScreenLocked));
    assert_eq!(lock_change(Some(true), false), Some(SystemEvent::ScreenUnlocked));
    assert_eq!(lock_change(Some(true), true), None);
}
