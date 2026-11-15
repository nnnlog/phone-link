use std::sync::Arc;
use std::thread;

use phonelink_engine::api::engine::Engine;
use phonelink_engine::edge::{EdgeError, LockState, Reachability, SessionLock};
use phonelink_engine::engine::SystemEvent;
use phonelink_remote_control::PcPowerControl;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{DynamicType, OwnedObjectPath};

const LOGIN1: &str = "org.freedesktop.login1";
const LOGIN1_MANAGER_PATH: &str = "/org/freedesktop/login1";
const LOGIN1_MANAGER: &str = "org.freedesktop.login1.Manager";
const LOGIN1_SESSION: &str = "org.freedesktop.login1.Session";
const AUTO_SESSION_PATH: &str = "/org/freedesktop/login1/session/auto";

const NETWORK_MANAGER: &str = "org.freedesktop.NetworkManager";
const NETWORK_MANAGER_PATH: &str = "/org/freedesktop/NetworkManager";
const NM_CONNECTIVITY_NONE: u32 = 1;

struct Endpoint {
    destination: &'static str,
    path: &'static str,
    interface: &'static str,
}

const KDE_SHUTDOWN: Endpoint =
    Endpoint { destination: "org.kde.Shutdown", path: "/Shutdown", interface: "org.kde.Shutdown" };
const XFCE_SESSION: Endpoint = Endpoint {
    destination: "org.xfce.SessionManager",
    path: "/org/xfce/SessionManager",
    interface: "org.xfce.Session.Manager",
};
const GNOME_SESSION: Endpoint = Endpoint {
    destination: "org.gnome.SessionManager",
    path: "/org/gnome/SessionManager",
    interface: "org.gnome.SessionManager",
};
const LOGIN1_MANAGER_ENDPOINT: Endpoint =
    Endpoint { destination: LOGIN1, path: LOGIN1_MANAGER_PATH, interface: LOGIN1_MANAGER };
const LOGIN1_SESSION_ENDPOINT: Endpoint =
    Endpoint { destination: LOGIN1, path: AUTO_SESSION_PATH, interface: LOGIN1_SESSION };

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desktop {
    Kde,
    Gnome,
    Xfce,
    Mate,
    Cinnamon,
    Other,
}

impl Desktop {
    pub fn detect() -> Self {
        Self::from_current_desktop(&std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default())
    }

    fn from_current_desktop(value: &str) -> Self {
        value
            .split(':')
            .find_map(|token| match token.to_ascii_lowercase().as_str() {
                "kde" => Some(Self::Kde),
                "gnome" | "gnome-classic" | "gnome-flashback" => Some(Self::Gnome),
                "xfce" => Some(Self::Xfce),
                "mate" => Some(Self::Mate),
                "cinnamon" | "x-cinnamon" => Some(Self::Cinnamon),
                _ => None,
            })
            .unwrap_or(Self::Other)
    }

    /// GNOME and KDE set logind's LockedHint; the other lockers do not.
    fn sets_locked_hint(self) -> bool {
        matches!(self, Self::Kde | Self::Gnome)
    }

    fn screensaver(self) -> Endpoint {
        match self {
            Self::Xfce => Endpoint {
                destination: "org.xfce.ScreenSaver",
                path: "/org/xfce/ScreenSaver",
                interface: "org.xfce.ScreenSaver",
            },
            Self::Mate => Endpoint {
                destination: "org.mate.ScreenSaver",
                path: "/org/mate/ScreenSaver",
                interface: "org.mate.ScreenSaver",
            },
            Self::Cinnamon => Endpoint {
                destination: "org.cinnamon.ScreenSaver",
                path: "/org/cinnamon/ScreenSaver",
                interface: "org.cinnamon.ScreenSaver",
            },
            Self::Gnome => Endpoint {
                destination: "org.gnome.ScreenSaver",
                path: "/org/gnome/ScreenSaver",
                interface: "org.gnome.ScreenSaver",
            },
            Self::Kde | Self::Other => Endpoint {
                destination: "org.freedesktop.ScreenSaver",
                path: "/org/freedesktop/ScreenSaver",
                interface: "org.freedesktop.ScreenSaver",
            },
        }
    }
}

fn proxy<'a>(bus: &Connection, endpoint: &Endpoint) -> zbus::Result<Proxy<'a>> {
    Proxy::new(bus, endpoint.destination, endpoint.path, endpoint.interface)
}

fn call(
    bus: Option<&Connection>,
    endpoint: &Endpoint,
    method: &str,
    body: &(impl serde::Serialize + DynamicType),
) -> Result<(), EdgeError> {
    let bus = bus.ok_or_else(|| EdgeError::Unexpected { message: "no D-Bus connection".to_owned() })?;
    proxy(bus, endpoint)
        .and_then(|proxy| proxy.call::<_, _, ()>(method, body))
        .map_err(|e| EdgeError::Unexpected { message: format!("{}.{method}: {e}", endpoint.interface) })
}

pub struct DesktopLock {
    desktop: Desktop,
    system: Option<Connection>,
    session: Option<Connection>,
}

impl DesktopLock {
    pub fn new(desktop: Desktop) -> Self {
        Self { desktop, system: Connection::system().ok(), session: Connection::session().ok() }
    }
}

impl SessionLock for DesktopLock {
    fn lock_state(&self) -> LockState {
        if self.desktop.sets_locked_hint() {
            let locked = self
                .system
                .as_ref()
                .and_then(|bus| proxy(bus, &LOGIN1_SESSION_ENDPOINT).ok()?.get_property::<bool>("LockedHint").ok());
            return match locked {
                Some(true) => LockState::Locked,
                Some(false) => LockState::Unlocked,
                None => LockState::Unknown,
            };
        }
        let active = self
            .session
            .as_ref()
            .and_then(|bus| proxy(bus, &self.desktop.screensaver()).ok()?.call::<_, _, bool>("GetActive", &()).ok());
        if active == Some(true) { LockState::Locked } else { LockState::Unlocked }
    }
}

pub struct NetworkManagerReachability {
    bus: Option<Connection>,
}

impl NetworkManagerReachability {
    pub fn new() -> Self {
        Self { bus: Connection::system().ok() }
    }
}

impl Reachability for NetworkManagerReachability {
    fn is_internet_available(&self) -> bool {
        self.bus
            .as_ref()
            .and_then(|bus| {
                Proxy::new(bus, NETWORK_MANAGER, NETWORK_MANAGER_PATH, NETWORK_MANAGER)
                    .ok()?
                    .get_property::<u32>("Connectivity")
                    .ok()
            })
            .is_none_or(|connectivity| connectivity != NM_CONNECTIVITY_NONE)
    }
}

pub struct DesktopPowerControl {
    desktop: Desktop,
    system: Option<Connection>,
    session: Option<Connection>,
}

impl DesktopPowerControl {
    pub fn new(desktop: Desktop) -> Self {
        Self { desktop, system: Connection::system().ok(), session: Connection::session().ok() }
    }
}

impl PcPowerControl for DesktopPowerControl {
    fn lock_screen(&self) -> Result<(), EdgeError> {
        call(self.system.as_ref(), &LOGIN1_SESSION_ENDPOINT, "Lock", &())
    }

    fn sleep(&self) -> Result<(), EdgeError> {
        call(self.system.as_ref(), &LOGIN1_MANAGER_ENDPOINT, "Suspend", &(false,))
    }

    /// Each desktop's session manager lets apps save first. GNOME's `Shutdown` shows its dialog,
    /// which proceeds after 60 s; MATE and Cinnamon offer `RequestShutdown` without one.
    fn shut_down(&self) -> Result<(), EdgeError> {
        let session = self.session.as_ref();
        match self.desktop {
            Desktop::Kde => call(session, &KDE_SHUTDOWN, "logoutAndShutdown", &()),
            Desktop::Xfce => call(session, &XFCE_SESSION, "Shutdown", &(true,)),
            Desktop::Mate | Desktop::Cinnamon => call(session, &GNOME_SESSION, "RequestShutdown", &()),
            Desktop::Gnome => call(session, &GNOME_SESSION, "Shutdown", &()),
            Desktop::Other => call(self.system.as_ref(), &LOGIN1_MANAGER_ENDPOINT, "PowerOff", &(false,)),
        }
    }

    fn restart(&self) -> Result<(), EdgeError> {
        let session = self.session.as_ref();
        match self.desktop {
            Desktop::Kde => call(session, &KDE_SHUTDOWN, "logoutAndReboot", &()),
            Desktop::Xfce => call(session, &XFCE_SESSION, "Restart", &(true,)),
            Desktop::Mate | Desktop::Cinnamon => call(session, &GNOME_SESSION, "RequestReboot", &()),
            Desktop::Gnome => call(session, &GNOME_SESSION, "Reboot", &()),
            Desktop::Other => call(self.system.as_ref(), &LOGIN1_MANAGER_ENDPOINT, "Reboot", &(false,)),
        }
    }
}

pub fn watch_system_events(engine: Arc<Engine>, desktop: Desktop) {
    let system = match Connection::system() {
        Ok(bus) => bus,
        Err(e) => {
            eprintln!("no system bus; lock and sleep events are not watched: {e}");
            return;
        }
    };

    let lock_engine = engine.clone();
    if desktop.sets_locked_hint() {
        let bus = system.clone();
        thread::spawn(move || {
            if let Err(e) = watch_locked_hint(&bus, &lock_engine) {
                eprintln!("screen lock events stopped: {e}");
            }
        });
    } else if let Ok(session) = Connection::session() {
        thread::spawn(move || {
            if let Err(e) = watch_screensaver(&session, &desktop.screensaver(), &lock_engine) {
                eprintln!("screen lock events stopped: {e}");
            }
        });
    }

    thread::spawn(move || {
        if let Err(e) = watch_sleep(&system, &engine) {
            eprintln!("sleep events stopped: {e}");
        }
    });
}

fn lock_event(locked: bool) -> SystemEvent {
    if locked { SystemEvent::ScreenLocked } else { SystemEvent::ScreenUnlocked }
}

fn watch_locked_hint(bus: &Connection, engine: &Engine) -> zbus::Result<()> {
    let id: String = proxy(bus, &LOGIN1_SESSION_ENDPOINT)?.get_property("Id")?;
    let path: OwnedObjectPath = proxy(bus, &LOGIN1_MANAGER_ENDPOINT)?.call("GetSession", &(id,))?;
    let session = Proxy::new(bus, LOGIN1, path, LOGIN1_SESSION)?;

    let mut locked: bool = session.get_property("LockedHint")?;
    for change in session.receive_property_changed::<bool>("LockedHint") {
        let now = change.get()?;
        if now != locked {
            locked = now;
            engine.system_event(lock_event(locked));
        }
    }
    Ok(())
}

fn watch_screensaver(bus: &Connection, endpoint: &Endpoint, engine: &Engine) -> zbus::Result<()> {
    for message in proxy(bus, endpoint)?.receive_signal("ActiveChanged")? {
        let active: bool = message.body().deserialize()?;
        engine.system_event(lock_event(active));
    }
    Ok(())
}

fn watch_sleep(bus: &Connection, engine: &Engine) -> zbus::Result<()> {
    for message in proxy(bus, &LOGIN1_MANAGER_ENDPOINT)?.receive_signal("PrepareForSleep")? {
        let sleeping: bool = message.body().deserialize()?;
        engine.system_event(if sleeping { SystemEvent::WillSleep } else { SystemEvent::DidWake });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_comes_from_the_first_known_token() {
        assert_eq!(Desktop::from_current_desktop("KDE"), Desktop::Kde);
        assert_eq!(Desktop::from_current_desktop("ubuntu:GNOME"), Desktop::Gnome);
        assert_eq!(Desktop::from_current_desktop("X-Cinnamon"), Desktop::Cinnamon);
        assert_eq!(Desktop::from_current_desktop("sway:wlroots"), Desktop::Other);
        assert_eq!(Desktop::from_current_desktop(""), Desktop::Other);
    }
}
