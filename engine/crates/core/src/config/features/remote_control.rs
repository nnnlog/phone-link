//! The options and timers of the `remote_control` feature, at their defaults.

/// The Mac's remote-control options, at their defaults (both on). They are configuration, not user
/// settings: the user's switch is the per-phone toggle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteControlConfig {
    /// Off: the Mac advertises no remote-control capability and answers every Cross Device request
    /// `DISABLED`; the toggle is not offered.
    pub enabled: bool,
    /// Off: the Mac advertises `REMOTE_CONTROL` without `REMOTE_CONTROL_V2`.
    pub v2_enabled: bool,
}

impl Default for RemoteControlConfig {
    fn default() -> Self {
        Self { enabled: true, v2_enabled: true }
    }
}
