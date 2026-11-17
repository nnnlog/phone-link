//! The D-Bus objects of logind and of the desktops' session managers that the session and power
//! actions and the watchers call.

use zbus::blocking::{Connection, Proxy};

use crate::dbus::uncached;

pub(super) const LOGIN1: &str = "org.freedesktop.login1";
const LOGIN1_MANAGER_PATH: &str = "/org/freedesktop/login1";
const LOGIN1_MANAGER: &str = "org.freedesktop.login1.Manager";
pub(super) const LOGIN1_SESSION: &str = "org.freedesktop.login1.Session";
const AUTO_SESSION_PATH: &str = "/org/freedesktop/login1/session/auto";

/// A D-Bus object and the interface of it that is called.
pub(super) struct Endpoint {
    pub(super) destination: &'static str,
    pub(super) path: &'static str,
    pub(super) interface: &'static str,
}

pub(super) const KDE_SHUTDOWN: Endpoint =
    Endpoint { destination: "org.kde.Shutdown", path: "/Shutdown", interface: "org.kde.Shutdown" };
pub(super) const XFCE_SESSION: Endpoint = Endpoint {
    destination: "org.xfce.SessionManager",
    path: "/org/xfce/SessionManager",
    interface: "org.xfce.Session.Manager",
};
pub(super) const GNOME_SESSION: Endpoint = Endpoint {
    destination: "org.gnome.SessionManager",
    path: "/org/gnome/SessionManager",
    interface: "org.gnome.SessionManager",
};
pub(super) const LOGIN1_MANAGER_ENDPOINT: Endpoint =
    Endpoint { destination: LOGIN1, path: LOGIN1_MANAGER_PATH, interface: LOGIN1_MANAGER };
pub(super) const LOGIN1_SESSION_ENDPOINT: Endpoint =
    Endpoint { destination: LOGIN1, path: AUTO_SESSION_PATH, interface: LOGIN1_SESSION };

/// A proxy of `endpoint` on `bus` that reads its properties from the service.
pub(super) fn endpoint_proxy(bus: &Connection, endpoint: &Endpoint) -> zbus::Result<Proxy<'static>> {
    uncached(bus, endpoint.destination, endpoint.path, endpoint.interface)
}
