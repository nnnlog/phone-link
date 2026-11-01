import Foundation
import PhoneLinkEngine

/// A button of a mirroring window's takeover.
public enum MirrorOverlayAction: Equatable, Sendable {
    /// Close the window.
    case close
    /// Dismiss the error and return to the phone screen.
    case dismiss
    /// Set up again (labelled Retry or Connect).
    case retry
    /// Turn on "allow apps", then set up again.
    case allowAndRetry
    /// Wake (or unlock) the phone's screen.
    case wake
    /// Ask the phone for its screen-capture consent again.
    case resendPermission
    /// Launch the paused app again, past the phone's launch checks.
    case resume
    /// Launch the app again.
    case reopen
    /// Go on past the phone's touch-confirmation warning.
    case continueTrueTouch
    /// Ask on the phone to allow audio (`openSettings`: open its settings instead).
    case audioPermission(openSettings: Bool)
    /// Open the window without the app's audio.
    case continueWithoutAudio
}

/// One button: its title and what it does; the first of a takeover is its default.
public struct MirrorOverlayButton: Equatable, Sendable {
    public let title: String
    public let action: MirrorOverlayAction

    public init(_ title: LocalizedStringResource, _ action: MirrorOverlayAction) {
        self.title = String(localized: title)
        self.action = action
    }
}

/// What a mirroring window shows over (or instead of) the phone's video, by the window's state.
public enum MirrorOverlay: Equatable, Sendable {
    /// The window waits for the phone: a spinner with a line under it, replaced by `slow` once the
    /// wait passes `slowAfter`.
    case loading(String, slow: String)
    /// A spinner with a line under it.
    case progress(String)
    /// A message with buttons, the first the default.
    case message(symbol: String, title: String, message: String, buttons: [MirrorOverlayButton])
    /// The nearby check's prompt (the phone status's).
    case nearby
    /// The phone asks to move the app to this Mac (`taskTransfer`: a task of another device).
    case moveToMac(taskTransfer: Bool)
    /// The user's layout switch waits for confirmation.
    case layoutSwitch
    /// The phone asks to confirm a touch: Continue (optionally never asking again) or Close.
    case trueTouch

    /// How long the window waits before its loading line says the wait is long.
    public static let slowAfter: Duration = .seconds(15)

    /// The overlay for `session` (`nil`: the video shows); `name` is the window's app or phone name.
    public static func of(_ session: MirrorSessionView, name: String, phoneName: String) -> MirrorOverlay? {
        switch session.state {
        case .loading:
            return .loading(
                session.phoneScreen ? String(localized: "Connecting to \(phoneName)…") : String(localized: "Opening \(name)…"),
                slow: String(localized: "This is taking longer than usual. Keep \(phoneName) unlocked and nearby.")
            )
        case .connected:
            return session.videoReady ? nil : .progress(String(localized: "Starting \(name)…"))
        case .connectionUnstable:
            return .progress(String(localized: "Reconnecting to \(phoneName)…"))
        case .nearbyConnectionNeeded:
            return .nearby
        case .screenCapturePermissionNeeded:
            return .message(
                symbol: "iphone",
                title: String(localized: "Check Your Phone"),
                message: String(localized: "Allow Link to Windows on \(phoneName) to share its screen with this Mac."),
                buttons: [MirrorOverlayButton("Ask Again", .resendPermission)]
            )
        case .error, .dismissibleError:
            return error(session.error, dismissible: session.state == .dismissibleError, name: name, phoneName: phoneName)
        case .restart:
            return session.restartLayoutSwitch ? .layoutSwitch : .moveToMac(taskTransfer: session.restartTaskTransfer)
        case .foldedCannotDraw:
            return .message(
                symbol: "iphone.gen3",
                title: String(localized: "Unfold Your Phone"),
                message: String(localized: "\(phoneName) can't show \(name) while it's folded."),
                buttons: []
            )
        case .locked:
            let message = session.secureLocked
                ? String(localized: "\(phoneName) is locked. Unlock it to continue.")
                : String(localized: "\(phoneName) is locked.")
            return .message(
                symbol: "lock",
                title: String(localized: "Your Phone Is Locked"),
                message: message,
                // A phone behind its PIN, pattern or biometrics unlocks only on the phone itself.
                buttons: session.secureLocked ? [] : [MirrorOverlayButton("Unlock", .wake)]
            )
        case .screenOff:
            return .message(
                symbol: "moon",
                title: String(localized: "Your Phone's Screen Is Off"),
                message: String(localized: "Wake \(phoneName) to continue."),
                buttons: [MirrorOverlayButton("Wake", .wake)]
            )
        case .suspended:
            return .message(
                symbol: "pause.circle",
                title: String(localized: "\(name) Is Paused"),
                message: String(localized: "\(phoneName) paused \(name) to open other apps."),
                buttons: [MirrorOverlayButton("Resume", .resume)]
            )
        case .closed:
            return .message(
                symbol: "xmark.app",
                title: String(localized: "\(name) Was Closed"),
                message: String(localized: "\(name) was closed on \(phoneName)."),
                buttons: [MirrorOverlayButton("Reopen", .reopen), MirrorOverlayButton("Close", .close)]
            )
        case .trueTouchWarning:
            return .trueTouch
        case .deviceOverheat:
            return .message(
                symbol: "thermometer.high",
                title: String(localized: "Your Phone Is Too Warm"),
                message: String(localized: "Let \(phoneName) cool down, then open \(name) again."),
                buttons: [MirrorOverlayButton("Close", .close)]
            )
        case .powerSavingMode:
            return .message(
                symbol: "battery.25",
                title: String(localized: "Your Phone Is Saving Power"),
                message: String(localized: "Turn off power saving on \(phoneName) to open \(name) on this Mac."),
                buttons: [MirrorOverlayButton("Open Again", .reopen)]
            )
        case .audioPermissionNeeded:
            return .message(
                symbol: "speaker.wave.2",
                title: String(localized: "Allow Audio on Your Phone"),
                message: String(localized: "To hear \(name) on this Mac, allow Link to Windows on \(phoneName) to share its audio."),
                buttons: [
                    MirrorOverlayButton("Send Request to Phone", .audioPermission(openSettings: false)),
                    MirrorOverlayButton("Open Phone Settings", .audioPermission(openSettings: true)),
                    MirrorOverlayButton("Continue Without Audio", .continueWithoutAudio),
                ]
            )
        case .audioPermissionAlwaysDenied:
            return .message(
                symbol: "speaker.slash",
                title: String(localized: "Audio Is Turned Off"),
                message: String(localized: "Link to Windows on \(phoneName) isn't allowed to share audio. Allow it in the phone's settings."),
                buttons: [
                    MirrorOverlayButton("Open Phone Settings", .audioPermission(openSettings: true)),
                    MirrorOverlayButton("Continue Without Audio", .continueWithoutAudio),
                ]
            )
        case .secureContent:
            return .message(
                symbol: "eye.slash",
                title: String(localized: "Protected Content"),
                message: String(localized: "\(name) is showing something that can't be shown on this Mac."),
                buttons: []
            )
        }
    }

    /// An error's takeover: what happened, and the one button the error offers.
    static func error(_ error: MirrorError?, dismissible: Bool, name: String, phoneName: String) -> MirrorOverlay {
        let kind = error?.kind ?? .generic
        let (title, message) = MirrorText.error(kind, name: name, phoneName: phoneName)

        let button: MirrorOverlayButton
        switch error?.action ?? .retry {
        case .close:
            button = dismissible ? MirrorOverlayButton("Dismiss", .dismiss) : MirrorOverlayButton("Close", .close)
        case .retry:
            button = MirrorOverlayButton("Try Again", .retry)
        case .connect:
            button = MirrorOverlayButton("Connect", .retry)
        case .allowAndRetry:
            button = MirrorOverlayButton("Allow and Try Again", .allowAndRetry)
        case .relaunch:
            button = MirrorOverlayButton("Open Again", .reopen)
        }

        return .message(symbol: "exclamationmark.triangle", title: title, message: message, buttons: [button])
    }
}

/// Mirroring's user-facing texts (HIG Writing: say what happened and what to do).
public enum MirrorText {
    /// An error's title and message.
    public static func error(_ kind: MirrorErrorKind, name: String, phoneName: String) -> (String, String) {
        switch kind {
        case .appNotAvailable:
            (String(localized: "App Not Available"), String(localized: "\(name) isn't available on \(phoneName)."))
        case .overheated:
            (String(localized: "Your Phone Is Too Warm"), String(localized: "Let \(phoneName) cool down, then try again."))
        case .powerSaving:
            (String(localized: "Your Phone Is Saving Power"), String(localized: "Turn off power saving on \(phoneName), then try again."))
        case .launchFailed:
            (String(localized: "Couldn't Open \(name)"), String(localized: "\(phoneName) couldn't open \(name)."))
        case .permissionOff:
            (String(localized: "Apps Are Turned Off"), String(localized: "Phone Link isn't allowed to open apps from \(phoneName) on this Mac."))
        case .disconnected:
            (String(localized: "Disconnected"), String(localized: "The connection to \(phoneName) ended."))
        case .timedOut:
            (String(localized: "Your Phone Didn't Respond"), String(localized: "\(phoneName) didn't answer in time. Keep it nearby and unlocked, then connect again."))
        case .cannotConnectDirectly:
            (String(localized: "Couldn't Connect"), String(localized: "This Mac couldn't connect to \(phoneName). Check that both are online, then try again."))
        case .phoneUnreachable:
            (String(localized: "Your Phone Can't Be Reached"), String(localized: "Check that \(phoneName) is on and online, then connect again."))
        case .meteredConnection:
            (String(localized: "Your Phone Is on Mobile Data"), String(localized: "\(phoneName) doesn't share its screen over a metered connection. Connect it to Wi-Fi, then try again."))
        case .featuresDisabled:
            (String(localized: "Link to Windows Is Turned Off"), String(localized: "Turn on Link to Windows on \(phoneName), then connect again."))
        case .updateNeeded:
            (String(localized: "Update Link to Windows"), String(localized: "Update Link to Windows on \(phoneName), then connect again."))
        case .serviceIssue:
            (String(localized: "Service Unavailable"), String(localized: "The service that connects to \(phoneName) didn't respond. Try again later."))
        case .generic:
            (String(localized: "Something Went Wrong"), String(localized: "\(phoneName)'s screen couldn't be shown."))
        }
    }
}
