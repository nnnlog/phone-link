use std::io::ErrorKind;

use zbus::zvariant::OwnedValue;

use super::testing::{io_error, method_error};
use super::*;

#[test]
fn error_names_are_matched_in_calls_and_property_reads() {
    let gone = zbus::Error::FDO(Box::new(zbus::fdo::Error::UnknownObject("no such object".into())));
    assert!(dbus_error_is(&gone, &[UNKNOWN_OBJECT]));
    assert!(dbus_error_is(&method_error(UNKNOWN_OBJECT), &[UNKNOWN_OBJECT]));
    assert!(!dbus_error_is(&method_error(UNKNOWN_METHOD), &[UNKNOWN_OBJECT]));
    assert!(!dbus_error_is(&io_error(ErrorKind::BrokenPipe), &[UNKNOWN_OBJECT]));
}

#[test]
fn both_buses_answers_for_a_missing_owner_are_leaving_the_bus() {
    assert!(left_the_bus(&method_error(SERVICE_UNKNOWN)));
    assert!(left_the_bus(&zbus::Error::FDO(Box::new(zbus::fdo::Error::NameHasNoOwner("gone".into())))));
    assert!(!left_the_bus(&method_error("org.freedesktop.NetworkManager.PermissionDenied")));
}

#[test]
fn only_a_broken_connection_is_lost() {
    assert!(connection_lost(&io_error(ErrorKind::BrokenPipe)));
    assert!(connection_lost(&io_error(ErrorKind::ConnectionReset)));
    assert!(!connection_lost(&io_error(ErrorKind::TimedOut)));
    assert!(!connection_lost(&method_error(SERVICE_UNKNOWN)));
}

fn body(interface: &str, changed: Vec<(&str, OwnedValue)>, invalidated: &[&str]) -> PropertiesChangedBody {
    (
        interface.to_owned(),
        changed.into_iter().map(|(name, value)| (name.to_owned(), value)).collect(),
        invalidated.iter().map(|name| (*name).to_owned()).collect(),
    )
}

#[test]
fn a_property_change_gives_the_new_value() {
    let changed =
        body("org.freedesktop.NetworkManager", vec![("Connectivity", 4u32.into()), ("State", 70u32.into())], &[]);
    assert_eq!(
        property_change::<u32>(changed, "org.freedesktop.NetworkManager", "Connectivity").unwrap(),
        PropertyChange::Changed(4)
    );
    let locked = body("org.freedesktop.login1.Session", vec![("LockedHint", true.into())], &[]);
    assert_eq!(
        property_change::<bool>(locked, "org.freedesktop.login1.Session", "LockedHint").unwrap(),
        PropertyChange::Changed(true)
    );
}

#[test]
fn an_invalidated_property_has_to_be_read() {
    let invalidated = body("org.freedesktop.login1.Session", vec![], &["LockedHint"]);
    assert_eq!(
        property_change::<bool>(invalidated, "org.freedesktop.login1.Session", "LockedHint").unwrap(),
        PropertyChange::Invalidated
    );
}

#[test]
fn other_properties_and_interfaces_are_no_change() {
    let other_property = body("org.freedesktop.NetworkManager", vec![("State", 70u32.into())], &["Devices"]);
    assert_eq!(
        property_change::<u32>(other_property, "org.freedesktop.NetworkManager", "Connectivity").unwrap(),
        PropertyChange::Unchanged
    );
    let other_interface = body("org.freedesktop.NetworkManager.Device", vec![("Connectivity", 4u32.into())], &[]);
    assert_eq!(
        property_change::<u32>(other_interface, "org.freedesktop.NetworkManager", "Connectivity").unwrap(),
        PropertyChange::Unchanged
    );
}

#[test]
fn a_property_of_another_type_is_an_error() {
    let wrong = body("org.freedesktop.NetworkManager", vec![("Connectivity", true.into())], &[]);
    assert!(property_change::<u32>(wrong, "org.freedesktop.NetworkManager", "Connectivity").is_err());
}
