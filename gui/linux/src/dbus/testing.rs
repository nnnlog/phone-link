//! D-Bus errors as zbus reports them, for tests of code that tells them apart.

use std::io::ErrorKind;
use std::sync::Arc;

/// The error zbus reports for a method call answered with the D-Bus error `name`.
pub(crate) fn method_error(name: &str) -> zbus::Error {
    let call = zbus::Message::method_call("/", "Get").unwrap().destination(":1.2").unwrap().build(&()).unwrap();
    zbus::Error::from(zbus::Message::error(&call.header(), name).unwrap().build(&("detail",)).unwrap())
}

pub(crate) fn io_error(kind: ErrorKind) -> zbus::Error {
    zbus::Error::InputOutput(Arc::new(std::io::Error::new(kind, "test")))
}
