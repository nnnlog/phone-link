//! Which desktop the session runs, as `XDG_CURRENT_DESKTOP` names it, and where its screen lock is
//! read.

use super::endpoint::Endpoint;

#[cfg(test)]
mod tests;

/// The desktop the session runs, for its screen lock and its session manager.
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

    /// GNOME and KDE set logind's `LockedHint`; the other lockers do not.
    pub(super) fn sets_locked_hint(self) -> bool {
        matches!(self, Self::Kde | Self::Gnome)
    }

    /// The screensaver whose `GetActive` says whether the screen is locked, where logind's `LockedHint`
    /// is not set.
    pub(super) fn screensaver(self) -> Endpoint {
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
