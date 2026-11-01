//! The transition tables: the phone state's and a window's, by command.

use super::{PhoneCommand, PhoneState, SessionCommand, WindowState};

// The transition tables: every (state, command) pair not listed is refused.
pub(crate) fn phone_transition(from: PhoneState, command: PhoneCommand) -> Option<PhoneState> {
    match from {
        PhoneState::Disconnected => phone_from_disconnected(command),
        PhoneState::Connecting => phone_from_connecting(command),
        PhoneState::Connected => phone_from_connected(command),
        PhoneState::Locked => phone_from_locked(command),
        PhoneState::SecureLocked => phone_from_secure_locked(command),
        PhoneState::Unlocked => phone_from_unlocked(command),
        PhoneState::FoldedCannotDraw => phone_from_folded_cannot_draw(command),
        PhoneState::Error => phone_from_error(command),
        PhoneState::ScreenScrapePermissionNeeded => phone_from_screen_scrape_permission_needed(command),
        PhoneState::NearbyConnectionNeeded => phone_from_nearby_connection_needed(command),
        PhoneState::ConnectionUnstable => phone_from_connection_unstable(command),
        PhoneState::ScreenOff => phone_from_screen_off(command),
    }
}

/// The phone state's transitions from `PhoneState::Disconnected`, by command.
fn phone_from_disconnected(command: PhoneCommand) -> Option<PhoneState> {
    Some(match command {
        PhoneCommand::NearbyConnectionNeeded => PhoneState::NearbyConnectionNeeded,
        PhoneCommand::ScreenScrapePermissionNeeded => PhoneState::ScreenScrapePermissionNeeded,
        PhoneCommand::EnsureConnection => PhoneState::Connecting,
        PhoneCommand::OnConnectionError => PhoneState::Error,
        PhoneCommand::OnConnectionClosed => PhoneState::Disconnected,
        PhoneCommand::OnConnectionUnstable => PhoneState::Disconnected,
        PhoneCommand::OnConnectionRecovered => PhoneState::Disconnected,
        PhoneCommand::OnLocked => PhoneState::Locked,
        PhoneCommand::OnSecureLocked => PhoneState::SecureLocked,
        _ => return None,
    })
}

/// The phone state's transitions from `PhoneState::Connecting`, by command.
fn phone_from_connecting(command: PhoneCommand) -> Option<PhoneState> {
    Some(match command {
        PhoneCommand::EnsureConnection => PhoneState::Connecting,
        PhoneCommand::ScreenScrapePermissionNeeded => PhoneState::ScreenScrapePermissionNeeded,
        PhoneCommand::OnConnected => PhoneState::Connected,
        PhoneCommand::OnConnectionError => PhoneState::Error,
        PhoneCommand::OnConnectionClosed => PhoneState::Disconnected,
        PhoneCommand::OnUnlocked => PhoneState::Connecting,
        PhoneCommand::OnSecureLocked => PhoneState::SecureLocked,
        PhoneCommand::OnLocked => PhoneState::Locked,
        PhoneCommand::OnScreenOff => PhoneState::ScreenOff,
        PhoneCommand::OnFoldedCannotDraw => PhoneState::FoldedCannotDraw,
        PhoneCommand::OnConnectionUnstable => PhoneState::Connecting,
        PhoneCommand::OnConnectionRecovered => PhoneState::Connecting,
        PhoneCommand::StateMachineTimeout => PhoneState::Error,
        _ => return None,
    })
}

/// The phone state's transitions from `PhoneState::Connected`, by command.
fn phone_from_connected(command: PhoneCommand) -> Option<PhoneState> {
    Some(match command {
        PhoneCommand::OnConnected => PhoneState::Connected,
        PhoneCommand::OnConnectionClosed => PhoneState::Disconnected,
        PhoneCommand::OnLocked => PhoneState::Locked,
        PhoneCommand::OnSecureLocked => PhoneState::SecureLocked,
        PhoneCommand::OnScreenOff => PhoneState::ScreenOff,
        PhoneCommand::OnFoldedCannotDraw => PhoneState::FoldedCannotDraw,
        PhoneCommand::OnUnlocked => PhoneState::Unlocked,
        PhoneCommand::OnConnectionUnstable => PhoneState::ConnectionUnstable,
        _ => return None,
    })
}

/// The phone state's transitions from `PhoneState::Locked`, by command.
fn phone_from_locked(command: PhoneCommand) -> Option<PhoneState> {
    Some(match command {
        PhoneCommand::OnLocked => PhoneState::Locked,
        PhoneCommand::OnSecureLocked => PhoneState::SecureLocked,
        PhoneCommand::OnScreenOff => PhoneState::ScreenOff,
        PhoneCommand::OnUnlocked => PhoneState::Unlocked,
        PhoneCommand::OnConnectionClosed => PhoneState::Disconnected,
        PhoneCommand::OnFoldedCannotDraw => PhoneState::FoldedCannotDraw,
        PhoneCommand::OnConnectionUnstable => PhoneState::ConnectionUnstable,
        PhoneCommand::OnConnected => PhoneState::Locked,
        PhoneCommand::NearbyConnectionNeeded => PhoneState::NearbyConnectionNeeded,
        PhoneCommand::NearbyConnectionCompleted => PhoneState::Connecting,
        _ => return None,
    })
}

/// The phone state's transitions from `PhoneState::SecureLocked`, by command.
fn phone_from_secure_locked(command: PhoneCommand) -> Option<PhoneState> {
    Some(match command {
        PhoneCommand::OnSecureLocked => PhoneState::SecureLocked,
        PhoneCommand::OnLocked => PhoneState::SecureLocked,
        PhoneCommand::OnScreenOff => PhoneState::ScreenOff,
        PhoneCommand::OnUnlocked => PhoneState::Unlocked,
        PhoneCommand::OnConnectionClosed => PhoneState::Disconnected,
        PhoneCommand::OnFoldedCannotDraw => PhoneState::FoldedCannotDraw,
        PhoneCommand::OnConnectionUnstable => PhoneState::ConnectionUnstable,
        PhoneCommand::OnConnected => PhoneState::SecureLocked,
        PhoneCommand::NearbyConnectionNeeded => PhoneState::NearbyConnectionNeeded,
        PhoneCommand::NearbyConnectionCompleted => PhoneState::Disconnected,
        _ => return None,
    })
}

/// The phone state's transitions from `PhoneState::Unlocked`, by command.
fn phone_from_unlocked(command: PhoneCommand) -> Option<PhoneState> {
    Some(match command {
        PhoneCommand::OnUnlocked => PhoneState::Unlocked,
        PhoneCommand::OnLocked => PhoneState::Locked,
        PhoneCommand::OnSecureLocked => PhoneState::SecureLocked,
        PhoneCommand::OnScreenOff => PhoneState::ScreenOff,
        PhoneCommand::OnConnectionClosed => PhoneState::Disconnected,
        PhoneCommand::OnFoldedCannotDraw => PhoneState::FoldedCannotDraw,
        PhoneCommand::OnConnectionUnstable => PhoneState::ConnectionUnstable,
        _ => return None,
    })
}

/// The phone state's transitions from `PhoneState::FoldedCannotDraw`, by command.
fn phone_from_folded_cannot_draw(command: PhoneCommand) -> Option<PhoneState> {
    Some(match command {
        PhoneCommand::OnFoldedCannotDraw => PhoneState::FoldedCannotDraw,
        PhoneCommand::OnLocked => PhoneState::Locked,
        PhoneCommand::OnScreenOff => PhoneState::ScreenOff,
        PhoneCommand::OnUnlocked => PhoneState::Unlocked,
        PhoneCommand::OnConnectionClosed => PhoneState::Disconnected,
        PhoneCommand::OnConnectionUnstable => PhoneState::ConnectionUnstable,
        PhoneCommand::OnConnected => PhoneState::FoldedCannotDraw,
        PhoneCommand::OnSecureLocked => PhoneState::SecureLocked,
        _ => return None,
    })
}

/// The phone state's transitions from `PhoneState::Error`, by command.
fn phone_from_error(command: PhoneCommand) -> Option<PhoneState> {
    Some(match command {
        PhoneCommand::EnsureConnection => PhoneState::Connecting,
        PhoneCommand::OnConnected => PhoneState::Connected,
        PhoneCommand::OnConnectionError => PhoneState::Error,
        PhoneCommand::OnConnectionClosed => PhoneState::Disconnected,
        PhoneCommand::RetryConnection => PhoneState::Disconnected,
        PhoneCommand::OnConnectionUnstable => PhoneState::Error,
        PhoneCommand::NearbyConnectionNeeded => PhoneState::NearbyConnectionNeeded,
        PhoneCommand::ScreenScrapePermissionNeeded => PhoneState::ScreenScrapePermissionNeeded,
        _ => return None,
    })
}

/// The phone state's transitions from `PhoneState::ScreenScrapePermissionNeeded`, by command.
fn phone_from_screen_scrape_permission_needed(command: PhoneCommand) -> Option<PhoneState> {
    Some(match command {
        PhoneCommand::ScreenScrapePermissionNeeded => PhoneState::ScreenScrapePermissionNeeded,
        PhoneCommand::EnsureConnection => PhoneState::Connecting,
        PhoneCommand::OnConnectionClosed => PhoneState::Disconnected,
        PhoneCommand::OnConnectionError => PhoneState::Error,
        _ => return None,
    })
}

/// The phone state's transitions from `PhoneState::NearbyConnectionNeeded`, by command.
fn phone_from_nearby_connection_needed(command: PhoneCommand) -> Option<PhoneState> {
    Some(match command {
        PhoneCommand::NearbyConnectionNeeded => PhoneState::NearbyConnectionNeeded,
        PhoneCommand::NearbyConnectionCompleted => PhoneState::Connecting,
        PhoneCommand::OnConnectionClosed => PhoneState::Disconnected,
        PhoneCommand::OnConnectionError => PhoneState::Error,
        PhoneCommand::ScreenScrapePermissionNeeded => PhoneState::ScreenScrapePermissionNeeded,
        PhoneCommand::OnSecureLocked => PhoneState::SecureLocked,
        PhoneCommand::OnLocked => PhoneState::Locked,
        _ => return None,
    })
}

/// The phone state's transitions from `PhoneState::ConnectionUnstable`, by command.
fn phone_from_connection_unstable(command: PhoneCommand) -> Option<PhoneState> {
    Some(match command {
        PhoneCommand::OnConnectionClosed => PhoneState::Disconnected,
        PhoneCommand::OnConnectionRecovered => PhoneState::Connected,
        PhoneCommand::OnConnectionError => PhoneState::Error,
        PhoneCommand::StateMachineTimeout => PhoneState::Error,
        _ => return None,
    })
}

/// The phone state's transitions from `PhoneState::ScreenOff`, by command.
fn phone_from_screen_off(command: PhoneCommand) -> Option<PhoneState> {
    Some(match command {
        PhoneCommand::OnSecureLocked => PhoneState::SecureLocked,
        PhoneCommand::OnLocked => PhoneState::Locked,
        PhoneCommand::OnScreenOff => PhoneState::ScreenOff,
        PhoneCommand::OnUnlocked => PhoneState::Unlocked,
        PhoneCommand::OnConnectionClosed => PhoneState::Disconnected,
        PhoneCommand::OnFoldedCannotDraw => PhoneState::FoldedCannotDraw,
        PhoneCommand::OnConnectionUnstable => PhoneState::ConnectionUnstable,
        PhoneCommand::OnConnected => PhoneState::ScreenOff,
        _ => return None,
    })
}

pub(crate) fn session_transition(from: WindowState, command: SessionCommand) -> Option<WindowState> {
    match from {
        WindowState::Loading => window_from_loading(command),
        WindowState::Connected => window_from_connected(command),
        WindowState::Closed => window_from_closed(command),
        WindowState::Suspended => window_from_suspended(command),
        WindowState::Locked => window_from_locked(command),
        WindowState::FoldedCannotDraw => window_from_folded_cannot_draw(command),
        WindowState::Error => window_from_error(command),
        WindowState::DismissibleError => window_from_dismissible_error(command),
        WindowState::TrueTouchWarning => window_from_true_touch_warning(command),
        WindowState::Restart => window_from_restart(command),
        WindowState::NearbyConnectionNeeded => window_from_nearby_connection_needed(command),
        WindowState::ScreenScrapePermissionNeeded => window_from_screen_scrape_permission_needed(command),
        WindowState::AudioStreamingPermissionNeeded => window_from_audio_streaming_permission_needed(command),
        WindowState::AudioStreamingPermissionAlwaysDenied => {
            window_from_audio_streaming_permission_always_denied(command)
        }
        WindowState::DeviceOverheat => window_from_device_overheat(command),
        WindowState::PowerSavingMode => window_from_power_saving_mode(command),
        WindowState::ConnectionUnstable => window_from_connection_unstable(command),
        WindowState::ScreenOff => window_from_screen_off(command),
        WindowState::SecureContent => window_from_secure_content(command),
    }
}

/// A window's transitions from `WindowState::Loading`, by command.
fn window_from_loading(command: SessionCommand) -> Option<WindowState> {
    Some(match command {
        SessionCommand::PhoneStateConnecting => WindowState::Loading,
        SessionCommand::PhoneStateConnected => WindowState::Connected,
        SessionCommand::PhoneStateConnectionUnstable => WindowState::ConnectionUnstable,
        SessionCommand::PhoneStateError => WindowState::Error,
        SessionCommand::PhoneStateFoldedCannotDraw => WindowState::FoldedCannotDraw,
        SessionCommand::PhoneStateLocked => WindowState::Locked,
        SessionCommand::PhoneStateUnlocked => WindowState::Loading,
        SessionCommand::PhoneStateScreenOff => WindowState::ScreenOff,
        SessionCommand::AppLaunchSuccess => WindowState::Connected,
        SessionCommand::AppLaunchError => WindowState::Error,
        SessionCommand::PeerConnectionClosed => WindowState::Closed,
        SessionCommand::PeerConnectionClosedSuspended => WindowState::Suspended,
        SessionCommand::PeerConnectionVideoReady => WindowState::Connected,
        SessionCommand::TrueTouchWarning => WindowState::TrueTouchWarning,
        SessionCommand::NearbyConnectionNeeded => WindowState::NearbyConnectionNeeded,
        SessionCommand::ScreenScrapePermissionNeeded => WindowState::ScreenScrapePermissionNeeded,
        SessionCommand::AudioStreamingPermissionNeeded => WindowState::AudioStreamingPermissionNeeded,
        SessionCommand::MoveToVirtualDisplay => WindowState::Restart,
        SessionCommand::RelaunchApp => WindowState::Loading,
        SessionCommand::RelaunchAll => WindowState::Loading,
        SessionCommand::DeviceOverheat => WindowState::DeviceOverheat,
        SessionCommand::PowerSavingMode => WindowState::PowerSavingMode,
        SessionCommand::StateMachineTimeout => WindowState::Error,
        SessionCommand::AudioStreamingPermissionFlowComplete => WindowState::Connected,
        SessionCommand::SecureContent => WindowState::SecureContent,
        SessionCommand::SwitchScreenLayout => WindowState::Restart,
        _ => return None,
    })
}

/// A window's transitions from `WindowState::Connected`, by command.
fn window_from_connected(command: SessionCommand) -> Option<WindowState> {
    Some(match command {
        SessionCommand::PhoneStateConnectionUnstable => WindowState::ConnectionUnstable,
        SessionCommand::PhoneStateError => WindowState::Error,
        SessionCommand::PhoneStateFoldedCannotDraw => WindowState::FoldedCannotDraw,
        SessionCommand::PhoneStateLocked => WindowState::Locked,
        SessionCommand::PhoneStateUnlocked => WindowState::Connected,
        SessionCommand::PhoneStateScreenOff => WindowState::ScreenOff,
        SessionCommand::AppLaunchSuccess => WindowState::Connected,
        SessionCommand::AppLaunchError => WindowState::Error,
        SessionCommand::PhoneScreenAppLaunchError => WindowState::DismissibleError,
        SessionCommand::PeerConnectionClosed => WindowState::Closed,
        SessionCommand::PeerConnectionClosedSuspended => WindowState::Suspended,
        SessionCommand::PeerConnectionVideoReady => WindowState::Connected,
        SessionCommand::MoveToVirtualDisplay => WindowState::Restart,
        SessionCommand::DeviceOverheat => WindowState::DeviceOverheat,
        SessionCommand::PowerSavingMode => WindowState::PowerSavingMode,
        SessionCommand::RelaunchAll => WindowState::Connected,
        SessionCommand::AudioStreamingPermissionNeeded => WindowState::AudioStreamingPermissionNeeded,
        SessionCommand::AudioStreamingPermissionFlowComplete => WindowState::Connected,
        SessionCommand::AudioStreamingPermissionAlwaysDenied => WindowState::AudioStreamingPermissionAlwaysDenied,
        SessionCommand::SecureContent => WindowState::SecureContent,
        SessionCommand::SwitchScreenLayout => WindowState::Restart,
        SessionCommand::PhoneStateConnecting => WindowState::Loading,
        _ => return None,
    })
}

/// A window's transitions from `WindowState::Closed`, by command.
fn window_from_closed(command: SessionCommand) -> Option<WindowState> {
    Some(match command {
        SessionCommand::AudioStreamingPermissionNeeded => WindowState::AudioStreamingPermissionNeeded,
        SessionCommand::PhoneStateConnecting => WindowState::Closed,
        SessionCommand::PhoneStateConnected => WindowState::Closed,
        SessionCommand::PhoneStateConnectionUnstable => WindowState::ConnectionUnstable,
        SessionCommand::PhoneStateError => WindowState::Error,
        SessionCommand::PhoneStateFoldedCannotDraw => WindowState::FoldedCannotDraw,
        SessionCommand::PhoneStateLocked => WindowState::Locked,
        SessionCommand::PhoneStateUnlocked => WindowState::Closed,
        SessionCommand::PhoneStateScreenOff => WindowState::ScreenOff,
        SessionCommand::AppLaunchSuccess => WindowState::Connected,
        SessionCommand::AppLaunchError => WindowState::Error,
        SessionCommand::PeerConnectionClosed => WindowState::Error,
        SessionCommand::PeerConnectionClosedSuspended => WindowState::Suspended,
        SessionCommand::PeerConnectionVideoReady => WindowState::Connected,
        SessionCommand::RelaunchApp => WindowState::Loading,
        SessionCommand::RelaunchAll => WindowState::Closed,
        SessionCommand::DeviceOverheat => WindowState::DeviceOverheat,
        SessionCommand::PowerSavingMode => WindowState::PowerSavingMode,
        SessionCommand::TrueTouchWarning => WindowState::TrueTouchWarning,
        SessionCommand::SecureContent => WindowState::SecureContent,
        _ => return None,
    })
}

/// A window's transitions from `WindowState::Suspended`, by command.
fn window_from_suspended(command: SessionCommand) -> Option<WindowState> {
    Some(match command {
        SessionCommand::PhoneStateConnecting => WindowState::Closed,
        SessionCommand::PhoneStateConnected => WindowState::Closed,
        SessionCommand::PhoneStateConnectionUnstable => WindowState::ConnectionUnstable,
        SessionCommand::PhoneStateError => WindowState::Error,
        SessionCommand::PhoneStateFoldedCannotDraw => WindowState::FoldedCannotDraw,
        SessionCommand::PhoneStateLocked => WindowState::Locked,
        SessionCommand::PhoneStateUnlocked => WindowState::Connected,
        SessionCommand::PhoneStateScreenOff => WindowState::ScreenOff,
        SessionCommand::AppLaunchSuccess => WindowState::Connected,
        SessionCommand::AppLaunchError => WindowState::Error,
        SessionCommand::PeerConnectionClosed => WindowState::Error,
        SessionCommand::PeerConnectionClosedSuspended => WindowState::Suspended,
        SessionCommand::PeerConnectionVideoReady => WindowState::Connected,
        SessionCommand::RelaunchApp => WindowState::Loading,
        SessionCommand::RelaunchAll => WindowState::Suspended,
        SessionCommand::DeviceOverheat => WindowState::DeviceOverheat,
        SessionCommand::PowerSavingMode => WindowState::PowerSavingMode,
        SessionCommand::SecureContent => WindowState::SecureContent,
        _ => return None,
    })
}

/// A window's transitions from `WindowState::Locked`, by command.
fn window_from_locked(command: SessionCommand) -> Option<WindowState> {
    Some(match command {
        SessionCommand::PhoneStateConnecting => WindowState::Locked,
        SessionCommand::PhoneStateConnectionUnstable => WindowState::ConnectionUnstable,
        SessionCommand::PhoneStateError => WindowState::Error,
        SessionCommand::PhoneStateFoldedCannotDraw => WindowState::FoldedCannotDraw,
        SessionCommand::PhoneStateLocked => WindowState::Locked,
        SessionCommand::PhoneStateUnlocked => WindowState::Connected,
        SessionCommand::PhoneStateScreenOff => WindowState::ScreenOff,
        SessionCommand::AppLaunchSuccess => WindowState::Locked,
        SessionCommand::AppLaunchError => WindowState::Error,
        SessionCommand::PhoneScreenAppLaunchError => WindowState::DismissibleError,
        SessionCommand::PeerConnectionClosed => WindowState::Closed,
        SessionCommand::PeerConnectionClosedSuspended => WindowState::Locked,
        SessionCommand::PeerConnectionVideoReady => WindowState::Locked,
        SessionCommand::MoveToVirtualDisplay => WindowState::Locked,
        SessionCommand::DeviceOverheat => WindowState::Locked,
        SessionCommand::PowerSavingMode => WindowState::PowerSavingMode,
        SessionCommand::RelaunchApp => WindowState::Locked,
        SessionCommand::RelaunchAll => WindowState::Locked,
        SessionCommand::SecureContent => WindowState::Locked,
        SessionCommand::SwitchScreenLayout => WindowState::Locked,
        SessionCommand::NearbyConnectionNeeded => WindowState::NearbyConnectionNeeded,
        _ => return None,
    })
}

/// A window's transitions from `WindowState::FoldedCannotDraw`, by command.
fn window_from_folded_cannot_draw(command: SessionCommand) -> Option<WindowState> {
    Some(match command {
        SessionCommand::PhoneStateConnectionUnstable => WindowState::ConnectionUnstable,
        SessionCommand::PhoneStateError => WindowState::Error,
        SessionCommand::PhoneStateFoldedCannotDraw => WindowState::FoldedCannotDraw,
        SessionCommand::PhoneStateLocked => WindowState::Locked,
        SessionCommand::PhoneStateUnlocked => WindowState::Connected,
        SessionCommand::PhoneStateScreenOff => WindowState::ScreenOff,
        SessionCommand::AppLaunchSuccess => WindowState::FoldedCannotDraw,
        SessionCommand::AppLaunchError => WindowState::Error,
        SessionCommand::PhoneScreenAppLaunchError => WindowState::FoldedCannotDraw,
        SessionCommand::PeerConnectionClosed => WindowState::Closed,
        SessionCommand::PeerConnectionClosedSuspended => WindowState::FoldedCannotDraw,
        SessionCommand::PeerConnectionVideoReady => WindowState::FoldedCannotDraw,
        SessionCommand::MoveToVirtualDisplay => WindowState::FoldedCannotDraw,
        SessionCommand::DeviceOverheat => WindowState::DeviceOverheat,
        SessionCommand::PowerSavingMode => WindowState::PowerSavingMode,
        SessionCommand::RelaunchAll => WindowState::FoldedCannotDraw,
        SessionCommand::SecureContent => WindowState::SecureContent,
        SessionCommand::SwitchScreenLayout => WindowState::FoldedCannotDraw,
        _ => return None,
    })
}

/// A window's transitions from `WindowState::Error`, by command.
fn window_from_error(command: SessionCommand) -> Option<WindowState> {
    Some(match command {
        SessionCommand::PhoneStateConnecting => WindowState::Loading,
        SessionCommand::PhoneStateConnected => WindowState::Connected,
        SessionCommand::PhoneStateConnectionUnstable => WindowState::ConnectionUnstable,
        SessionCommand::PhoneStateError => WindowState::Error,
        SessionCommand::PhoneStateFoldedCannotDraw => WindowState::Error,
        SessionCommand::PhoneStateLocked => WindowState::Error,
        SessionCommand::PhoneStateUnlocked => WindowState::Error,
        SessionCommand::PhoneStateScreenOff => WindowState::Error,
        SessionCommand::AppLaunchSuccess => WindowState::Connected,
        SessionCommand::AppLaunchError => WindowState::Error,
        SessionCommand::PhoneScreenAppLaunchError => WindowState::DismissibleError,
        SessionCommand::PeerConnectionClosed => WindowState::Error,
        SessionCommand::PeerConnectionClosedSuspended => WindowState::Suspended,
        SessionCommand::PeerConnectionVideoReady => WindowState::Connected,
        SessionCommand::RelaunchApp => WindowState::Loading,
        SessionCommand::RelaunchAll => WindowState::Loading,
        SessionCommand::MoveToVirtualDisplay => WindowState::Error,
        SessionCommand::NearbyConnectionNeeded => WindowState::NearbyConnectionNeeded,
        SessionCommand::ScreenScrapePermissionNeeded => WindowState::ScreenScrapePermissionNeeded,
        SessionCommand::DeviceOverheat => WindowState::DeviceOverheat,
        SessionCommand::PowerSavingMode => WindowState::PowerSavingMode,
        SessionCommand::StateMachineTimeout => WindowState::Error,
        SessionCommand::AudioStreamingPermissionNeeded => WindowState::AudioStreamingPermissionNeeded,
        SessionCommand::AudioStreamingPermissionAlwaysDenied => WindowState::AudioStreamingPermissionAlwaysDenied,
        SessionCommand::SecureContent => WindowState::SecureContent,
        SessionCommand::SwitchScreenLayout => WindowState::Error,
        _ => return None,
    })
}

/// A window's transitions from `WindowState::DismissibleError`, by command.
fn window_from_dismissible_error(command: SessionCommand) -> Option<WindowState> {
    Some(match command {
        SessionCommand::PhoneStateConnecting => WindowState::Loading,
        SessionCommand::PhoneStateConnected => WindowState::Connected,
        SessionCommand::PhoneStateConnectionUnstable => WindowState::ConnectionUnstable,
        SessionCommand::PhoneStateError => WindowState::Error,
        SessionCommand::PhoneStateFoldedCannotDraw => WindowState::Error,
        SessionCommand::PhoneStateLocked => WindowState::DismissibleError,
        SessionCommand::PhoneStateUnlocked => WindowState::DismissibleError,
        SessionCommand::PhoneStateScreenOff => WindowState::DismissibleError,
        SessionCommand::AppLaunchSuccess => WindowState::Connected,
        SessionCommand::DismissError => WindowState::Connected,
        SessionCommand::AppLaunchError => WindowState::Error,
        SessionCommand::PhoneScreenAppLaunchError => WindowState::DismissibleError,
        SessionCommand::PeerConnectionClosed => WindowState::DismissibleError,
        SessionCommand::PeerConnectionClosedSuspended => WindowState::Suspended,
        SessionCommand::PeerConnectionVideoReady => WindowState::Connected,
        SessionCommand::RelaunchApp => WindowState::DismissibleError,
        SessionCommand::RelaunchAll => WindowState::DismissibleError,
        SessionCommand::NearbyConnectionNeeded => WindowState::NearbyConnectionNeeded,
        SessionCommand::ScreenScrapePermissionNeeded => WindowState::ScreenScrapePermissionNeeded,
        SessionCommand::StateMachineTimeout => WindowState::Error,
        SessionCommand::AudioStreamingPermissionNeeded => WindowState::AudioStreamingPermissionNeeded,
        SessionCommand::AudioStreamingPermissionAlwaysDenied => WindowState::AudioStreamingPermissionAlwaysDenied,
        SessionCommand::SecureContent => WindowState::SecureContent,
        _ => return None,
    })
}

/// A window's transitions from `WindowState::TrueTouchWarning`, by command.
fn window_from_true_touch_warning(command: SessionCommand) -> Option<WindowState> {
    Some(match command {
        SessionCommand::PhoneStateConnectionUnstable => WindowState::ConnectionUnstable,
        SessionCommand::PhoneStateLocked => WindowState::Locked,
        SessionCommand::PhoneStateUnlocked => WindowState::TrueTouchWarning,
        SessionCommand::PhoneStateScreenOff => WindowState::ScreenOff,
        SessionCommand::AppLaunchSuccess => WindowState::Connected,
        SessionCommand::PhoneStateError => WindowState::Error,
        SessionCommand::DeviceOverheat => WindowState::DeviceOverheat,
        SessionCommand::PowerSavingMode => WindowState::PowerSavingMode,
        SessionCommand::RelaunchApp => WindowState::TrueTouchWarning,
        SessionCommand::RelaunchAll => WindowState::TrueTouchWarning,
        SessionCommand::SecureContent => WindowState::SecureContent,
        _ => return None,
    })
}

/// A window's transitions from `WindowState::Restart`, by command.
fn window_from_restart(command: SessionCommand) -> Option<WindowState> {
    Some(match command {
        SessionCommand::PeerConnectionVideoReady => WindowState::Restart,
        SessionCommand::PhoneStateConnected => WindowState::Connected,
        SessionCommand::PhoneStateConnectionUnstable => WindowState::ConnectionUnstable,
        SessionCommand::PhoneStateLocked => WindowState::Locked,
        SessionCommand::PhoneStateFoldedCannotDraw => WindowState::FoldedCannotDraw,
        SessionCommand::PhoneStateUnlocked => WindowState::Restart,
        SessionCommand::PhoneStateScreenOff => WindowState::ScreenOff,
        SessionCommand::MoveToVirtualDisplay => WindowState::Restart,
        SessionCommand::AppLaunchSuccess => WindowState::Connected,
        SessionCommand::PhoneStateError => WindowState::Error,
        SessionCommand::DeviceOverheat => WindowState::DeviceOverheat,
        SessionCommand::PowerSavingMode => WindowState::PowerSavingMode,
        SessionCommand::PeerConnectionClosed => WindowState::Closed,
        SessionCommand::SecureContent => WindowState::SecureContent,
        SessionCommand::RelaunchAll => WindowState::Restart,
        SessionCommand::SwitchScreenLayout => WindowState::Restart,
        SessionCommand::PhoneStateConnecting => WindowState::Loading,
        _ => return None,
    })
}

/// A window's transitions from `WindowState::NearbyConnectionNeeded`, by command.
fn window_from_nearby_connection_needed(command: SessionCommand) -> Option<WindowState> {
    Some(match command {
        SessionCommand::NearbyConnectionNeeded => WindowState::NearbyConnectionNeeded,
        SessionCommand::PhoneStateError => WindowState::Error,
        SessionCommand::PhoneStateConnecting => WindowState::Loading,
        SessionCommand::PhoneStateConnected => WindowState::Connected,
        SessionCommand::PhoneStateConnectionUnstable => WindowState::ConnectionUnstable,
        SessionCommand::AppLaunchSuccess => WindowState::Connected,
        SessionCommand::PhoneStateFoldedCannotDraw => WindowState::NearbyConnectionNeeded,
        SessionCommand::PhoneStateLocked => WindowState::Locked,
        SessionCommand::PhoneStateUnlocked => WindowState::NearbyConnectionNeeded,
        SessionCommand::PhoneStateScreenOff => WindowState::NearbyConnectionNeeded,
        SessionCommand::AppLaunchError => WindowState::Error,
        SessionCommand::PeerConnectionClosed => WindowState::Closed,
        SessionCommand::PeerConnectionClosedSuspended => WindowState::Suspended,
        SessionCommand::PeerConnectionVideoReady => WindowState::Connected,
        SessionCommand::DeviceOverheat => WindowState::DeviceOverheat,
        SessionCommand::PowerSavingMode => WindowState::PowerSavingMode,
        SessionCommand::RelaunchApp => WindowState::NearbyConnectionNeeded,
        SessionCommand::RelaunchAll => WindowState::NearbyConnectionNeeded,
        SessionCommand::AudioStreamingPermissionNeeded => WindowState::AudioStreamingPermissionNeeded,
        SessionCommand::SecureContent => WindowState::SecureContent,
        _ => return None,
    })
}

/// A window's transitions from `WindowState::ScreenScrapePermissionNeeded`, by command.
fn window_from_screen_scrape_permission_needed(command: SessionCommand) -> Option<WindowState> {
    Some(match command {
        SessionCommand::ScreenScrapePermissionNeeded => WindowState::ScreenScrapePermissionNeeded,
        SessionCommand::PhoneStateError => WindowState::Error,
        SessionCommand::PhoneStateConnecting => WindowState::Loading,
        SessionCommand::PhoneStateConnected => WindowState::Connected,
        SessionCommand::PhoneStateConnectionUnstable => WindowState::ConnectionUnstable,
        SessionCommand::AppLaunchSuccess => WindowState::Connected,
        SessionCommand::PhoneStateFoldedCannotDraw => WindowState::ScreenScrapePermissionNeeded,
        SessionCommand::PhoneStateLocked => WindowState::ScreenScrapePermissionNeeded,
        SessionCommand::PhoneStateUnlocked => WindowState::ScreenScrapePermissionNeeded,
        SessionCommand::PhoneStateScreenOff => WindowState::ScreenScrapePermissionNeeded,
        SessionCommand::AppLaunchError => WindowState::Error,
        SessionCommand::PeerConnectionClosed => WindowState::Closed,
        SessionCommand::PeerConnectionClosedSuspended => WindowState::Suspended,
        SessionCommand::PeerConnectionVideoReady => WindowState::Connected,
        SessionCommand::DeviceOverheat => WindowState::DeviceOverheat,
        SessionCommand::PowerSavingMode => WindowState::PowerSavingMode,
        SessionCommand::RelaunchApp => WindowState::ScreenScrapePermissionNeeded,
        SessionCommand::RelaunchAll => WindowState::ScreenScrapePermissionNeeded,
        SessionCommand::AudioStreamingPermissionNeeded => WindowState::AudioStreamingPermissionNeeded,
        SessionCommand::SecureContent => WindowState::SecureContent,
        SessionCommand::AudioStreamingPermissionFlowComplete => WindowState::ScreenScrapePermissionNeeded,
        _ => return None,
    })
}

/// A window's transitions from `WindowState::AudioStreamingPermissionNeeded`, by command.
fn window_from_audio_streaming_permission_needed(command: SessionCommand) -> Option<WindowState> {
    Some(match command {
        SessionCommand::AudioStreamingPermissionNeeded => WindowState::AudioStreamingPermissionNeeded,
        SessionCommand::PhoneStateError => WindowState::Error,
        SessionCommand::PhoneStateConnecting => WindowState::Loading,
        SessionCommand::AudioStreamingPermissionFlowComplete => WindowState::Connected,
        SessionCommand::PhoneStateConnectionUnstable => WindowState::ConnectionUnstable,
        SessionCommand::PhoneStateFoldedCannotDraw => WindowState::AudioStreamingPermissionNeeded,
        SessionCommand::PhoneStateLocked => WindowState::AudioStreamingPermissionNeeded,
        SessionCommand::PhoneStateUnlocked => WindowState::AudioStreamingPermissionNeeded,
        SessionCommand::PhoneStateScreenOff => WindowState::AudioStreamingPermissionNeeded,
        SessionCommand::AppLaunchError => WindowState::Error,
        SessionCommand::PeerConnectionClosed => WindowState::Closed,
        SessionCommand::PeerConnectionClosedSuspended => WindowState::Suspended,
        SessionCommand::PeerConnectionVideoReady => WindowState::AudioStreamingPermissionNeeded,
        SessionCommand::DeviceOverheat => WindowState::DeviceOverheat,
        SessionCommand::PowerSavingMode => WindowState::PowerSavingMode,
        SessionCommand::RelaunchApp => WindowState::AudioStreamingPermissionNeeded,
        SessionCommand::RelaunchAll => WindowState::AudioStreamingPermissionNeeded,
        SessionCommand::SecureContent => WindowState::SecureContent,
        _ => return None,
    })
}

/// A window's transitions from `WindowState::AudioStreamingPermissionAlwaysDenied`, by command.
fn window_from_audio_streaming_permission_always_denied(command: SessionCommand) -> Option<WindowState> {
    Some(match command {
        SessionCommand::AudioStreamingPermissionNeeded => WindowState::AudioStreamingPermissionNeeded,
        SessionCommand::AudioStreamingPermissionAlwaysDenied => WindowState::AudioStreamingPermissionAlwaysDenied,
        SessionCommand::PhoneStateError => WindowState::Error,
        SessionCommand::PhoneStateConnecting => WindowState::Loading,
        SessionCommand::AudioStreamingPermissionFlowComplete => WindowState::Connected,
        SessionCommand::PhoneStateConnectionUnstable => WindowState::ConnectionUnstable,
        SessionCommand::PhoneStateFoldedCannotDraw => WindowState::AudioStreamingPermissionAlwaysDenied,
        SessionCommand::PhoneStateLocked => WindowState::AudioStreamingPermissionAlwaysDenied,
        SessionCommand::PhoneStateUnlocked => WindowState::AudioStreamingPermissionAlwaysDenied,
        SessionCommand::PhoneStateScreenOff => WindowState::AudioStreamingPermissionAlwaysDenied,
        SessionCommand::AppLaunchError => WindowState::Error,
        SessionCommand::PeerConnectionClosed => WindowState::Closed,
        SessionCommand::PeerConnectionClosedSuspended => WindowState::Suspended,
        SessionCommand::PeerConnectionVideoReady => WindowState::AudioStreamingPermissionAlwaysDenied,
        SessionCommand::DeviceOverheat => WindowState::DeviceOverheat,
        SessionCommand::PowerSavingMode => WindowState::PowerSavingMode,
        SessionCommand::RelaunchApp => WindowState::AudioStreamingPermissionAlwaysDenied,
        SessionCommand::RelaunchAll => WindowState::AudioStreamingPermissionAlwaysDenied,
        SessionCommand::SecureContent => WindowState::SecureContent,
        _ => return None,
    })
}

/// A window's transitions from `WindowState::DeviceOverheat`, by command.
fn window_from_device_overheat(command: SessionCommand) -> Option<WindowState> {
    Some(match command {
        SessionCommand::PhoneStateConnecting => WindowState::Loading,
        SessionCommand::PhoneStateConnected => WindowState::Connected,
        SessionCommand::PhoneStateConnectionUnstable => WindowState::ConnectionUnstable,
        SessionCommand::PhoneStateError => WindowState::Error,
        SessionCommand::PhoneStateFoldedCannotDraw => WindowState::FoldedCannotDraw,
        SessionCommand::PhoneStateLocked => WindowState::Locked,
        SessionCommand::PhoneStateUnlocked => WindowState::DeviceOverheat,
        SessionCommand::PhoneStateScreenOff => WindowState::ScreenOff,
        SessionCommand::AppLaunchSuccess => WindowState::Connected,
        SessionCommand::AppLaunchError => WindowState::Error,
        SessionCommand::PeerConnectionClosed => WindowState::Error,
        SessionCommand::PeerConnectionClosedSuspended => WindowState::Suspended,
        SessionCommand::PeerConnectionVideoReady => WindowState::Connected,
        SessionCommand::RelaunchApp => WindowState::Loading,
        SessionCommand::RelaunchAll => WindowState::Loading,
        SessionCommand::PowerSavingMode => WindowState::PowerSavingMode,
        SessionCommand::SecureContent => WindowState::SecureContent,
        _ => return None,
    })
}

/// A window's transitions from `WindowState::PowerSavingMode`, by command.
fn window_from_power_saving_mode(command: SessionCommand) -> Option<WindowState> {
    Some(match command {
        SessionCommand::PhoneStateConnecting => WindowState::Loading,
        SessionCommand::PhoneStateConnected => WindowState::Connected,
        SessionCommand::PhoneStateConnectionUnstable => WindowState::ConnectionUnstable,
        SessionCommand::PhoneStateError => WindowState::Error,
        SessionCommand::PhoneStateFoldedCannotDraw => WindowState::FoldedCannotDraw,
        SessionCommand::PhoneStateLocked => WindowState::Locked,
        SessionCommand::PhoneStateUnlocked => WindowState::PowerSavingMode,
        SessionCommand::PhoneStateScreenOff => WindowState::ScreenOff,
        SessionCommand::AppLaunchSuccess => WindowState::Connected,
        SessionCommand::AppLaunchError => WindowState::Error,
        SessionCommand::PeerConnectionClosed => WindowState::Error,
        SessionCommand::PeerConnectionClosedSuspended => WindowState::Suspended,
        SessionCommand::PeerConnectionVideoReady => WindowState::Connected,
        SessionCommand::RelaunchApp => WindowState::Loading,
        SessionCommand::RelaunchAll => WindowState::Loading,
        SessionCommand::DeviceOverheat => WindowState::PowerSavingMode,
        SessionCommand::SecureContent => WindowState::SecureContent,
        _ => return None,
    })
}

/// A window's transitions from `WindowState::ConnectionUnstable`, by command.
fn window_from_connection_unstable(command: SessionCommand) -> Option<WindowState> {
    Some(match command {
        SessionCommand::PhoneStateConnected => WindowState::Connected,
        SessionCommand::PhoneStateError => WindowState::Error,
        SessionCommand::StateMachineTimeout => WindowState::Error,
        SessionCommand::RelaunchAll => WindowState::ConnectionUnstable,
        _ => return None,
    })
}

/// A window's transitions from `WindowState::ScreenOff`, by command.
fn window_from_screen_off(command: SessionCommand) -> Option<WindowState> {
    Some(match command {
        SessionCommand::PhoneStateConnectionUnstable => WindowState::ConnectionUnstable,
        SessionCommand::PhoneStateError => WindowState::Error,
        SessionCommand::PhoneStateFoldedCannotDraw => WindowState::FoldedCannotDraw,
        SessionCommand::PhoneStateLocked => WindowState::Locked,
        SessionCommand::PhoneStateUnlocked => WindowState::Connected,
        SessionCommand::PhoneStateScreenOff => WindowState::ScreenOff,
        SessionCommand::AppLaunchSuccess => WindowState::ScreenOff,
        SessionCommand::AppLaunchError => WindowState::Error,
        SessionCommand::PhoneScreenAppLaunchError => WindowState::DismissibleError,
        SessionCommand::PeerConnectionClosed => WindowState::Closed,
        SessionCommand::PeerConnectionClosedSuspended => WindowState::ScreenOff,
        SessionCommand::PeerConnectionVideoReady => WindowState::ScreenOff,
        SessionCommand::MoveToVirtualDisplay => WindowState::ScreenOff,
        SessionCommand::DeviceOverheat => WindowState::DeviceOverheat,
        SessionCommand::PowerSavingMode => WindowState::PowerSavingMode,
        SessionCommand::RelaunchAll => WindowState::ScreenOff,
        SessionCommand::SecureContent => WindowState::SecureContent,
        SessionCommand::SwitchScreenLayout => WindowState::ScreenOff,
        _ => return None,
    })
}

/// A window's transitions from `WindowState::SecureContent`, by command.
fn window_from_secure_content(command: SessionCommand) -> Option<WindowState> {
    Some(match command {
        SessionCommand::PhoneStateConnecting => WindowState::Loading,
        SessionCommand::PhoneStateConnected => WindowState::Connected,
        SessionCommand::PhoneStateConnectionUnstable => WindowState::ConnectionUnstable,
        SessionCommand::PhoneStateError => WindowState::Error,
        SessionCommand::PhoneStateFoldedCannotDraw => WindowState::FoldedCannotDraw,
        SessionCommand::PhoneStateLocked => WindowState::Locked,
        SessionCommand::PhoneStateUnlocked => WindowState::SecureContent,
        SessionCommand::PhoneStateScreenOff => WindowState::ScreenOff,
        SessionCommand::AppLaunchSuccess => WindowState::Connected,
        SessionCommand::AppLaunchError => WindowState::Error,
        SessionCommand::PeerConnectionClosed => WindowState::Error,
        SessionCommand::PeerConnectionClosedSuspended => WindowState::Suspended,
        SessionCommand::PeerConnectionVideoReady => WindowState::Connected,
        SessionCommand::RelaunchApp => WindowState::Loading,
        SessionCommand::RelaunchAll => WindowState::Loading,
        SessionCommand::PowerSavingMode => WindowState::PowerSavingMode,
        _ => return None,
    })
}
