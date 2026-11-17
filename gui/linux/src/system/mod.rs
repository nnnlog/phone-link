//! The desktop session's services for the engine: the screen lock's state and changes, sleep and
//! wake, internet reachability from `NetworkManager`, the session and power actions the phone asks for,
//! and opening or showing a received file.

mod desktop;
mod endpoint;
mod events;
mod lock;
mod open;
mod power;
mod reachability;
mod supervise;

use std::time::Duration;

pub use desktop::Desktop;
pub use events::watch_system_events;
pub use lock::DesktopLock;
pub use open::{open_path, reveal_path};
pub use power::DesktopPowerControl;
pub use reachability::NetworkManagerReachability;

/// How long a call may take on the connections the lock state, the reachability flag and the
/// watchers use: the engine reads the lock state and the flag as short local calls, the lock state on
/// every message from the phone.
const METHOD_TIMEOUT: Duration = Duration::from_secs(1);
