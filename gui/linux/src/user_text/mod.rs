//! Short, user-facing texts for what the engine reports: say what happened and what to do. The
//! English texts are the macOS app's where they fit (so its Korean applies), with "this computer" for
//! "this Mac".

use phonelink_engine::api::FeatureApiError;
use phonelink_engine::api::RequestOutcome;
use phonelink_engine::api::bluetooth::BluetoothError;
use phonelink_engine::api::calling::CallControlError;
use phonelink_engine::api::hotspot::HotspotError;
use phonelink_engine::engine::EngineError;

use crate::l10n::{fill, translate, translated};

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// Why work failed when the reason is an OS or library error: such a text is no sentence for the window,
/// so the caller logs it and shows this.
pub fn something_went_wrong() -> String {
    translated("Something went wrong in Phone Link.")
}

/// An error of a feature call.
pub fn feature_error(error: &FeatureApiError) -> String {
    match error {
        FeatureApiError::NotStarted => translated("Phone Link isn't connected to your account yet."),
        FeatureApiError::NoPhone => translated("No phone is linked to this account."),
        FeatureApiError::UnknownItem { .. } => translated("This item is no longer on your phone."),
        FeatureApiError::NotAvailable { reason } => fill(translate("Not available: %@."), std::slice::from_ref(reason)),
        FeatureApiError::InvalidInput { reason } => capitalized(reason) + ".",
        FeatureApiError::Store { .. } => translated("Phone Link couldn't read its saved data."),
        FeatureApiError::Internal { .. } => translated("Something went wrong in Phone Link."),
    }
}

/// An error of a Bluetooth pairing or connection.
pub fn bluetooth_error(error: &BluetoothError) -> String {
    match error {
        BluetoothError::BluetoothOff => translated("Turn on Bluetooth on this computer and try again."),
        BluetoothError::PermissionNotGranted => {
            translated("Allow Link to Windows to use Bluetooth on your phone, then try again.")
        }
        BluetoothError::NotDiscoverable => {
            translated("Your phone didn't become visible over Bluetooth. Keep it nearby and try again.")
        }
        BluetoothError::PhoneNotFound => {
            translated("This computer didn't find your phone over Bluetooth. Keep the phone nearby and try again.")
        }
        BluetoothError::PairingFailed { reason } => fill(
            translate(
                "Pairing with your phone didn't finish (%@). You can also pair it in your system's Bluetooth settings, then try again.",
            ),
            std::slice::from_ref(reason),
        ),
        BluetoothError::Internal { .. } => translated("Something went wrong in Phone Link."),
    }
}

/// An error of a call-control command.
pub fn call_error(error: &CallControlError) -> String {
    match error {
        CallControlError::Feature { error } => feature_error(error),
        CallControlError::NotReady => translated("This computer isn't connected to your phone over Bluetooth."),
        CallControlError::NoSuchCall => translated("The call has ended."),
        CallControlError::InvalidState => translated("That isn't possible for this call right now."),
        CallControlError::EmptyNumber => translated("Enter a phone number."),
        CallControlError::DialFailed { .. } => translated("Your phone didn't place the call."),
        CallControlError::Refused { .. } => translated("Your phone didn't accept the request. Try it on your phone."),
        CallControlError::Bluetooth { error } => bluetooth_error(error),
        CallControlError::Internal { .. } => translated("Something went wrong in Phone Link."),
    }
}

/// Why joining or leaving the phone's hotspot did not finish.
pub fn hotspot_error(error: &HotspotError) -> String {
    match error {
        HotspotError::Feature { error } => feature_error(error),
        HotspotError::Bluetooth { error } => bluetooth_error(error),
        HotspotError::Busy => translated("Phone Link is already working on it."),
        HotspotError::NotSupported => translated("Your phone can't share its connection with this computer."),
        HotspotError::ConsentTimedOut => translated(
            "Your phone didn't answer. Allow instant hotspot in the notification on your phone, then try again.",
        ),
        HotspotError::NoAnswer => translated(
            "Your phone didn't respond. Make sure this computer is in your phone's list of paired Bluetooth devices, then try again.",
        ),
        HotspotError::NotTrusted { .. } => {
            translated("Your phone didn't accept this computer. Reconnect Phone Link and try again.")
        }
        HotspotError::Unreachable { .. } => {
            translated("This computer couldn't reach your phone over Bluetooth. Keep the phone nearby and try again.")
        }
        HotspotError::SideChannelNotOpen => translated(
            "This computer couldn't find instant hotspot on your phone over Bluetooth. Make sure instant hotspot is on in Link to Windows. If it is, remove your phone in your system's Bluetooth settings and try again: Phone Link pairs with it again.",
        ),
        HotspotError::Unavailable { .. } => translated("Your phone can't share its connection right now."),
        HotspotError::NotVisible { ssid, .. } => {
            fill(translate("Join \u{201c}%@\u{201d} from the Wi-Fi menu when it appears."), std::slice::from_ref(ssid))
        }
        HotspotError::JoinFailed { ssid, .. } => {
            fill(translate("Join \u{201c}%@\u{201d} from the Wi-Fi menu."), std::slice::from_ref(ssid))
        }
        HotspotError::PhoneError { .. } => translated("Your phone couldn't turn on its hotspot."),
        HotspotError::NoCredentials | HotspotError::InvalidCredentials { .. } => {
            translated("Your phone sent hotspot details this computer can't use.")
        }
        _ => translated("Your phone couldn't do this."),
    }
}

/// Why the engine did not start, for the failure screen.
pub fn engine_error(error: &EngineError) -> String {
    match error {
        EngineError::Auth { .. } => translated("Signing in to your Microsoft account didn't finish."),
        EngineError::NoPhone => {
            translated("No phone is linked to this Microsoft account. Link one with Link to Windows on the phone.")
        }
        EngineError::Setup { message } if message.contains("store failure") => translated(
            "Phone Link couldn't open its saved sign-in. Make sure a keyring (GNOME Keyring, KWallet or KeePassXC) is running and unlocked, then try again.",
        ),
        EngineError::Setup { .. } => translated("Phone Link couldn't start."),
        EngineError::NotStarted => translated("Phone Link isn't connected to your account yet."),
    }
}

/// `None` when the request succeeded; otherwise why it did not.
pub fn outcome_failure(outcome: &RequestOutcome) -> Option<String> {
    let text = match outcome {
        RequestOutcome::Success | RequestOutcome::Accepted | RequestOutcome::AppLaunched => return None,
        RequestOutcome::Denied | RequestOutcome::PermanentlyDenied => "Your phone denied the request.",
        RequestOutcome::NeedsPermission | RequestOutcome::NeedsInitialPermission => "Allow access on your phone first.",
        RequestOutcome::TimedOut => "Your phone didn't respond.",
        RequestOutcome::InternetError => "This computer isn't connected to the internet.",
        RequestOutcome::ConnectionFailed { .. } | RequestOutcome::SendFailed { .. } => {
            "Phone Link couldn't reach your phone."
        }
        RequestOutcome::AllFeaturesDisabled => "Link to Windows is turned off on your phone.",
        RequestOutcome::MeteredConnection | RequestOutcome::MeteredSyncDisabled => {
            "Your phone doesn't sync on a metered connection."
        }
        RequestOutcome::PhoneContractIncompatible => "Update Link to Windows on your phone.",
        RequestOutcome::PcContractIncompatible => "Update Phone Link on this computer.",
        RequestOutcome::DeviceNotSupported => "Your phone doesn't support this.",
        RequestOutcome::TwoPhoneMode | RequestOutcome::NonDefaultProfile => {
            "Your phone can't do this in its current profile."
        }
        RequestOutcome::Canceled => "The request was canceled.",
        RequestOutcome::Failure
        | RequestOutcome::ResourceFailed { .. }
        | RequestOutcome::InvalidRequest
        | RequestOutcome::DatabaseFailure
        | RequestOutcome::Unknown => "Your phone couldn't do this.",
    };
    Some(translated(text))
}

#[cfg(test)]
mod tests;
