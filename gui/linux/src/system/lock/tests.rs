use std::io::ErrorKind;

use super::*;
use crate::dbus::testing::{io_error, method_error};

#[test]
fn a_missing_screensaver_is_no_lock_service() {
    assert!(no_lock_service(&method_error(SERVICE_UNKNOWN)));
    assert!(no_lock_service(&method_error(UNKNOWN_INTERFACE)));
    assert!(!no_lock_service(&method_error("org.freedesktop.DBus.Error.AccessDenied")));
    assert!(!no_lock_service(&io_error(ErrorKind::TimedOut)));
}
