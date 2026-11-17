//! The session and power actions the phone asks for: lock, sleep, shut down and restart.

use phonelink_engine::edge::EdgeError;
use phonelink_remote_control::PcPowerControl;
use zbus::zvariant::DynamicType;

use super::desktop::Desktop;
use super::endpoint::{
    Endpoint, GNOME_SESSION, KDE_SHUTDOWN, LOGIN1_MANAGER_ENDPOINT, LOGIN1_SESSION_ENDPOINT, XFCE_SESSION,
    endpoint_proxy,
};
use crate::dbus::{Bus, LazyBus};

/// The session and power actions on the phone's request: logind locks and suspends; each desktop's
/// session manager shuts down and restarts, so that apps are asked to save first.
pub struct DesktopPowerControl {
    desktop: Desktop,
    system: LazyBus,
    session: LazyBus,
}

impl DesktopPowerControl {
    /// The connections are made on the first action. Their calls have no time limit: GNOME's
    /// session manager answers a shutdown only once the session ends or the user cancels.
    pub fn new(desktop: Desktop) -> Self {
        Self { desktop, system: LazyBus::new(Bus::System, None), session: LazyBus::new(Bus::Session, None) }
    }
}

/// Calls `method` of `endpoint` on `bus`, forgetting the connection when the call shows it was lost.
fn call(
    bus: &LazyBus,
    endpoint: &Endpoint,
    method: &str,
    body: &(impl serde::Serialize + DynamicType),
) -> Result<(), EdgeError> {
    bus.connection()
        .and_then(|connection| endpoint_proxy(&connection, endpoint)?.call::<_, _, ()>(method, body))
        .map_err(|e| {
            bus.after_error(&e);
            EdgeError::Unexpected { message: format!("{}.{method}: {e}", endpoint.interface) }
        })
}

impl PcPowerControl for DesktopPowerControl {
    fn lock_screen(&self) -> Result<(), EdgeError> {
        call(&self.system, &LOGIN1_SESSION_ENDPOINT, "Lock", &())
    }

    fn sleep(&self) -> Result<(), EdgeError> {
        call(&self.system, &LOGIN1_MANAGER_ENDPOINT, "Suspend", &(false,))
    }

    /// Each desktop's session manager lets apps save first. GNOME's `Shutdown` shows its dialog,
    /// which proceeds after 60 s; MATE and Cinnamon offer `RequestShutdown` without one.
    fn shut_down(&self) -> Result<(), EdgeError> {
        let session = &self.session;
        match self.desktop {
            Desktop::Kde => call(session, &KDE_SHUTDOWN, "logoutAndShutdown", &()),
            Desktop::Xfce => call(session, &XFCE_SESSION, "Shutdown", &(true,)),
            Desktop::Mate | Desktop::Cinnamon => call(session, &GNOME_SESSION, "RequestShutdown", &()),
            Desktop::Gnome => call(session, &GNOME_SESSION, "Shutdown", &()),
            Desktop::Other => call(&self.system, &LOGIN1_MANAGER_ENDPOINT, "PowerOff", &(false,)),
        }
    }

    fn restart(&self) -> Result<(), EdgeError> {
        let session = &self.session;
        match self.desktop {
            Desktop::Kde => call(session, &KDE_SHUTDOWN, "logoutAndReboot", &()),
            Desktop::Xfce => call(session, &XFCE_SESSION, "Restart", &(true,)),
            Desktop::Mate | Desktop::Cinnamon => call(session, &GNOME_SESSION, "RequestReboot", &()),
            Desktop::Gnome => call(session, &GNOME_SESSION, "Reboot", &()),
            Desktop::Other => call(&self.system, &LOGIN1_MANAGER_ENDPOINT, "Reboot", &(false,)),
        }
    }
}
