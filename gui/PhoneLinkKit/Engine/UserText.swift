import Foundation
import PhoneLinkEngine

/// Short, user-facing texts for what the engine reports (HIG Writing: say what happened and what to
/// do, without blame and without "we").
public enum UserText {
    /// An error thrown by an engine or feature call.
    public static func describe(_ error: Error) -> String {
        switch error {
        case let e as FeatureApiError:
            switch e {
            case .NotStarted: return String(localized: "Phone Link isn't connected to your account yet.")
            case .NoPhone: return String(localized: "No phone is linked to this account.")
            case .UnknownItem: return String(localized: "This item is no longer on your phone.")
            case let .NotAvailable(reason): return String(localized: "Not available: \(reason).")
            case let .InvalidInput(reason): return capitalized(reason) + "."
            case .Store: return String(localized: "Phone Link couldn't read its saved data.")
            case .Internal: return String(localized: "Something went wrong in Phone Link.")
            }
        case let e as BluetoothError:
            switch e {
            case .BluetoothOff: return String(localized: "Turn on Bluetooth on this Mac and try again.")
            case .PermissionNotGranted: return String(localized: "Allow Link to Windows to use Bluetooth on your phone, then try again.")
            case .NotDiscoverable: return String(localized: "Your phone didn't become visible over Bluetooth. Keep it nearby and try again.")
            case .PhoneNotFound: return String(localized: "This Mac didn't find your phone over Bluetooth. Keep the phone nearby and try again.")
            case let .PairingFailed(reason):
                return String(localized: "Pairing with your phone didn't finish (\(reason)). You can also pair it in System Settings › Bluetooth, then try again.")
            case .Internal: return String(localized: "Something went wrong in Phone Link.")
            }
        case let e as CallControlError:
            switch e {
            case let .Feature(error): return describe(error)
            case .NotReady: return String(localized: "This Mac isn't connected to your phone over Bluetooth.")
            case .NoSuchCall: return String(localized: "The call has ended.")
            case .InvalidState: return String(localized: "That isn't possible for this call right now.")
            case .EmptyNumber: return String(localized: "Enter a phone number.")
            case .DialFailed: return String(localized: "Your phone didn't place the call.")
            case .Refused: return String(localized: "Your phone didn't accept the request. Try it on your phone.")
            case let .Bluetooth(error): return describe(error)
            case .Internal: return String(localized: "Something went wrong in Phone Link.")
            }
        case let e as EngineError:
            switch e {
            case .Auth: return String(localized: "Signing in to your Microsoft account didn't finish.")
            case .NoPhone: return String(localized: "No phone is linked to this Microsoft account. Link one with Link to Windows on the phone.")
            case .Setup: return String(localized: "Phone Link couldn't start.")
            case .NotStarted: return String(localized: "Phone Link isn't connected to your account yet.")
            }
        default:
            return error.localizedDescription
        }
    }

    /// `nil` when the request succeeded; otherwise why it did not.
    public static func failure(_ outcome: RequestOutcome) -> String? {
        switch outcome {
        case .success, .accepted, .appLaunched:
            return nil
        case .denied, .permanentlyDenied:
            return String(localized: "Your phone denied the request.")
        case .needsPermission, .needsInitialPermission:
            return String(localized: "Allow access on your phone first.")
        case .timedOut:
            return String(localized: "Your phone didn't respond.")
        case .internetError:
            return String(localized: "This Mac isn't connected to the internet.")
        case .connectionFailed, .sendFailed:
            return String(localized: "Phone Link couldn't reach your phone.")
        case .allFeaturesDisabled:
            return String(localized: "Link to Windows is turned off on your phone.")
        case .meteredConnection, .meteredSyncDisabled:
            return String(localized: "Your phone doesn't sync on a metered connection.")
        case .phoneContractIncompatible:
            return String(localized: "Update Link to Windows on your phone.")
        case .pcContractIncompatible:
            return String(localized: "Update Phone Link on this Mac.")
        case .deviceNotSupported:
            return String(localized: "Your phone doesn't support this.")
        case .twoPhoneMode, .nonDefaultProfile:
            return String(localized: "Your phone can't do this in its current profile.")
        case .canceled:
            return String(localized: "The request was canceled.")
        case .failure, .resourceFailed, .invalidRequest, .databaseFailure, .unknown:
            return String(localized: "Your phone couldn't do this.")
        }
    }

    static func capitalized(_ s: String) -> String {
        guard let first = s.first else { return s }
        return first.uppercased() + s.dropFirst()
    }
}
