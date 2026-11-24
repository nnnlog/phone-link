//! One copy of the app per user session: the first copy owns the name `phonelink.linux.App` on the
//! session bus and shows its window when another copy asks; a later copy asks it to and quits. A copy
//! started with a link hands it to the first copy, which opens it once its window can.

use std::sync::{Mutex, OnceLock};

use phonelink_core::sync::MutexExt;

use zbus::blocking::{Connection, Proxy, connection};
use zbus::fdo::RequestNameFlags;

const NAME: &str = "phonelink.linux.App";
const PATH: &str = "/phonelink/linux/App";

type Show = Box<dyn Fn() + Send + Sync>;
type Open = Box<dyn Fn(String) + Send + Sync>;

/// What the first copy does when another copy asks it to show itself; set once the window can show.
static SHOW: OnceLock<Show> = OnceLock::new();
/// What the first copy does with a link; set once the window can open one.
static OPEN: OnceLock<Open> = OnceLock::new();
/// The link that came before the window could open one.
static WAITING_LINK: Mutex<Option<String>> = Mutex::new(None);

/// The object the first copy serves at [`PATH`].
struct App;

#[zbus::interface(name = "phonelink.linux.App")]
impl App {
    fn show(&self) {
        if let Some(show) = SHOW.get() {
            show();
        }
    }

    fn open(&self, link: String) {
        open_link(link);
    }
}

/// Opens `link` in this copy: now, or once the window can.
pub fn open_link(link: String) {
    match OPEN.get() {
        Some(open) => open(link),
        None => *WAITING_LINK.lock_or_recover() = Some(link),
    }
}

/// Which copy this is.
pub enum Claim {
    /// The first copy, with the connection that holds the name while it runs; `None` when the session
    /// bus was not reached, so no other copy is kept out.
    First(Option<Connection>),
    /// Another copy runs and was asked to show itself.
    Running,
}

/// Claims the name, or asks the copy that holds it to open `link`, or without one to show its window.
pub fn claim(link: Option<&str>) -> Claim {
    let first = connection::Builder::session().and_then(|builder| builder.serve_at(PATH, App)?.build()).and_then(
        |connection| {
            connection.request_name_with_flags(NAME, RequestNameFlags::DoNotQueue.into())?;
            Ok(connection)
        },
    );

    match first {
        Ok(connection) => Claim::First(Some(connection)),
        Err(zbus::Error::NameTaken) => {
            let shown = Connection::session().and_then(|bus| {
                let first = Proxy::new(&bus, NAME, PATH, NAME)?;
                match link {
                    Some(link) => first.call::<_, _, ()>("Open", &(link,)),
                    None => first.call::<_, _, ()>("Show", &()),
                }
            });
            match shown {
                Ok(()) => Claim::Running,
                // A second copy would share the first one's sign-in, data and device identity: this one
                // quits even though the first did not show its window.
                Err(e) => {
                    tracing::error!(error = %e, "Phone Link is already running but did not answer; this copy quits");
                    Claim::Running
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "Phone Link does not guard against a second copy");
            Claim::First(None)
        }
    }
}

/// Sets what showing the window does, for a later copy's request.
pub fn on_show(show: impl Fn() + Send + Sync + 'static) {
    let _ = SHOW.set(Box::new(show));
}

/// Sets what opening a link does, and opens the one that came before.
pub fn on_open(open: impl Fn(String) + Send + Sync + 'static) {
    let _ = OPEN.set(Box::new(open));
    let waiting = WAITING_LINK.lock_or_recover().take();
    if let Some(link) = waiting {
        open_link(link);
    }
}
