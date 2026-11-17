//! The D-Bus plumbing the edges share: connections to the system and session buses, proxies that read
//! properties without zbus's cache, the bus's own answers (who owns a name, which error a reply is), and
//! the `PropertiesChanged` signal.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::sync::Mutex;
use std::time::Duration;

use phonelink_core::sync::MutexExt;
use zbus::DBusError;
use zbus::blocking::fdo::DBusProxy;
use zbus::blocking::proxy::SignalIterator;
use zbus::blocking::{Connection, Proxy};
use zbus::names::BusName;
use zbus::proxy::CacheProperties;
use zbus::zvariant::{ObjectPath, OwnedValue};

#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;

pub(crate) const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

pub(crate) const SERVICE_UNKNOWN: &str = "org.freedesktop.DBus.Error.ServiceUnknown";
pub(crate) const NAME_HAS_NO_OWNER: &str = "org.freedesktop.DBus.Error.NameHasNoOwner";
pub(crate) const UNKNOWN_METHOD: &str = "org.freedesktop.DBus.Error.UnknownMethod";
pub(crate) const UNKNOWN_INTERFACE: &str = "org.freedesktop.DBus.Error.UnknownInterface";
/// What a service answers for an object path where it has no object, such as one it just removed.
pub(crate) const UNKNOWN_OBJECT: &str = "org.freedesktop.DBus.Error.UnknownObject";

/// Which message bus a connection goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bus {
    System,
    Session,
}

/// A new connection to `bus`, whose method calls fail after `method_timeout` when one is given.
pub(crate) fn connect(bus: Bus, method_timeout: Option<Duration>) -> zbus::Result<Connection> {
    let builder = match bus {
        Bus::System => zbus::blocking::connection::Builder::system()?,
        Bus::Session => zbus::blocking::connection::Builder::session()?,
    };
    match method_timeout {
        Some(timeout) => builder.method_timeout(timeout),
        None => builder,
    }
    .build()
}

/// A connection to one message bus, made on first use: a failed connect is tried again on the next
/// use, and a connection that was lost is replaced on the use after the failure that showed it.
pub(crate) struct LazyBus {
    bus: Bus,
    method_timeout: Option<Duration>,
    connection: Mutex<Option<Connection>>,
}

impl LazyBus {
    pub(crate) fn new(bus: Bus, method_timeout: Option<Duration>) -> Self {
        Self { bus, method_timeout, connection: Mutex::new(None) }
    }

    pub(crate) fn connection(&self) -> zbus::Result<Connection> {
        let mut connection = self.connection.lock_or_recover();
        if let Some(connection) = connection.as_ref() {
            return Ok(connection.clone());
        }
        let made = connect(self.bus, self.method_timeout)?;
        Ok(connection.insert(made).clone())
    }

    /// Forgets the connection when `error` shows it was lost, so the next use connects again.
    pub(crate) fn after_error(&self, error: &zbus::Error) {
        if connection_lost(error) {
            *self.connection.lock_or_recover() = None;
        }
    }
}

/// A proxy that reads every property from the service instead of zbus's cache: a cached proxy would
/// subscribe to change signals and fetch every property for one read.
pub(crate) fn uncached<'a, D, P>(
    bus: &Connection,
    destination: D,
    path: P,
    interface: &'a str,
) -> zbus::Result<Proxy<'a>>
where
    D: TryInto<BusName<'a>>,
    D::Error: Into<zbus::Error>,
    P: TryInto<ObjectPath<'a>>,
    P::Error: Into<zbus::Error>,
{
    zbus::blocking::proxy::Builder::new(bus)
        .destination(destination)?
        .path(path)?
        .interface(interface)?
        .cache_properties(CacheProperties::No)
        .build()
}

/// The unique name of the connection that owns `name` now; `None` while none does. Asking the bus,
/// rather than calling the name, never starts a D-Bus-activatable service that is installed but not
/// running, and calls to the unique name cannot start one either.
pub(crate) fn name_owner(bus: &Connection, name: &'static str) -> zbus::Result<Option<BusName<'static>>> {
    match DBusProxy::new(bus)?.get_name_owner(BusName::try_from(name)?) {
        Ok(owner) => Ok(Some(BusName::from(owner))),
        Err(zbus::fdo::Error::NameHasNoOwner(_)) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// `error` is a D-Bus error reply named one of `names`, whichever way zbus reports it: a method
/// call's as `MethodError`, a property read's as one of the standard `fdo` errors.
pub(crate) fn dbus_error_is(error: &zbus::Error, names: &[&str]) -> bool {
    match error {
        zbus::Error::MethodError(name, _, _) => names.contains(&name.as_str()),
        zbus::Error::FDO(error) => names.contains(&error.name().as_str()),
        _ => false,
    }
}

/// The bus answered that nothing owns the name a call went to: the service is not on the bus, or
/// left it. dbus-daemon and dbus-broker name this answer differently.
pub(crate) fn left_the_bus(error: &zbus::Error) -> bool {
    dbus_error_is(error, &[SERVICE_UNKNOWN, NAME_HAS_NO_OWNER])
}

/// The connection itself failed (its socket closed or broke), as opposed to a call that failed or
/// timed out on a working connection: only a new connection helps.
pub(crate) fn connection_lost(error: &zbus::Error) -> bool {
    matches!(error, zbus::Error::InputOutput(io) if io.kind() != ErrorKind::TimedOut)
}

/// The `PropertiesChanged` signals for the properties of `interface` on the object `path` of
/// `destination`. They keep coming from a service that leaves the bus and comes back under a new
/// connection.
pub(crate) fn property_changes<P>(
    bus: &Connection,
    destination: &'static str,
    path: P,
    interface: &'static str,
) -> zbus::Result<SignalIterator<'static>>
where
    P: TryInto<ObjectPath<'static>>,
    P::Error: Into<zbus::Error>,
{
    uncached(bus, destination, path, PROPERTIES)?.receive_signal_with_args("PropertiesChanged", &[(0, interface)])
}

/// The body of `org.freedesktop.DBus.Properties.PropertiesChanged`: the interface, its changed
/// properties with their new values, and its invalidated properties.
pub(crate) type PropertiesChangedBody = (String, HashMap<String, OwnedValue>, Vec<String>);

/// What a `PropertiesChanged` signal says about one property.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PropertyChange<T> {
    /// It changed to this value.
    Changed(T),
    /// It changed, and its value has to be read.
    Invalidated,
    /// The signal is about other properties, or another interface.
    Unchanged,
}

/// What `body` says about the property `name` of `interface`.
pub(crate) fn property_change<T>(
    body: PropertiesChangedBody,
    interface: &str,
    name: &str,
) -> zbus::Result<PropertyChange<T>>
where
    T: TryFrom<OwnedValue>,
    T::Error: Into<zbus::Error>,
{
    let (changed_interface, mut changed, invalidated) = body;
    if changed_interface != interface {
        return Ok(PropertyChange::Unchanged);
    }
    if let Some(value) = changed.remove(name) {
        return T::try_from(value).map(PropertyChange::Changed).map_err(Into::into);
    }
    Ok(if invalidated.iter().any(|property| property == name) {
        PropertyChange::Invalidated
    } else {
        PropertyChange::Unchanged
    })
}
