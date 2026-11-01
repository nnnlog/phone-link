import AppKit
import Foundation
import Observation
import os
import PhoneLinkEngine

/// Instant hotspot for the menu: whether the selected phone can share its connection and whether
/// this Mac is on it, the connect that turns the phone's hotspot on and joins it, and the disconnect
/// that leaves it. There is no separate setup: the first
/// connect pairs with the phone over Bluetooth when no paired device is the phone and asks for the
/// phone's consent, as any Bluetooth feature pairs when the user first uses it. macOS has no way to
/// list the phone's hotspot in the system Wi-Fi menu, so the menu bar offers it instead.
///
/// The state is the engine's: read when the engine starts, whenever the phone record changes, and
/// whenever the Mac's Wi-Fi network changes (`reload()`, called by the app).
@MainActor
@Observable
public final class HotspotModel {
    /// What the menu offers.
    public enum Item: Equatable {
        case connect
        /// This Mac is on the phone's hotspot: offer disconnect.
        case connected
        case working(Work)
    }

    /// An action under way.
    public enum Work: Equatable, Sendable {
        case connecting, disconnecting

        /// The menu's text.
        public var text: String {
            switch self {
            case .connecting: String(localized: "Connecting to Phone Hotspot…")
            case .disconnecting: String(localized: "Disconnecting from Phone Hotspot…")
            }
        }

        /// The tile's state.
        public var state: String {
            switch self {
            case .connecting: String(localized: "Connecting…")
            case .disconnecting: String(localized: "Disconnecting…")
            }
        }
    }

    /// A finished action the app shows in an alert.
    public struct Outcome: Equatable, Identifiable {
        public let id = UUID()
        public let title: String
        public let message: String
        /// The network to join by hand, when the credentials arrived but the join did not finish.
        public let manualJoin: (ssid: String, passphrase: String)?

        public static func == (a: Outcome, b: Outcome) -> Bool { a.id == b.id }
    }

    public private(set) var state: HotspotPhoneState?
    public private(set) var working: Work?
    /// Called with every finished action that needs the user's attention.
    @ObservationIgnored public var present: ((Outcome) -> Void)?

    private let api: any HotspotApiProtocol
    @ObservationIgnored private var suspended = false

    private static let log = AppLog(category: "hotspot")

    public init(api: any HotspotApiProtocol, hub: EngineEventHub) {
        self.api = api

        hub.onEvent { [weak self] event in
            self?.handle(event)
        }
        hub.onStart { [weak self] in
            self?.suspended = false
            self?.reload()
        }
    }

    /// The menu item, or `nil` while the phone cannot share its connection.
    public var item: Item? {
        if let working { return .working(working) }
        guard let state, state.supported else { return nil }
        return state.joined ? .connected : .connect
    }

    public func reload() {
        guard !suspended else { return }
        do {
            state = try api.phoneState()
        } catch let error as FeatureApiError where error == .NotStarted || error == .NoPhone {
            state = nil
        } catch {
            Self.log.error("hotspot state not read: \(error: error)")
        }
    }

    public func connect() {
        run(.connecting) { [api] in
            _ = try await api.connect()
            return nil
        }
    }

    public func disconnect() {
        run(.disconnecting) { [api] in
            try await api.disconnect()
            return nil
        }
    }

    /// Connects while not on the phone's hotspot, disconnects while on it.
    public func toggle() {
        if item == .connected { disconnect() } else { connect() }
    }

    private func run(_ work: Work, _ action: @escaping @Sendable () async throws -> Outcome?) {
        guard working == nil, !suspended else { return }
        working = work

        Task { [weak self] in
            let outcome: Outcome?
            do {
                outcome = try await action()
            } catch {
                Self.log.error("hotspot \(work == .connecting ? "not connected" : "not disconnected", privacy: .public): \(error: error)")
                outcome = Self.outcome(for: error)
            }

            guard let self else { return }
            self.working = nil
            self.reload()
            if let outcome { self.present?(outcome) }
        }
    }

    private func handle(_ event: EngineEvent) {
        switch event {
        case .accountRemoved, .phoneUnlinked:
            suspended = true
            state = nil
        case .phoneChanged, .phoneSession:
            reload()
        default:
            break
        }
    }

    /// The alert for a failed connect or disconnect.
    static func outcome(for error: Error) -> Outcome {
        let fail = { (message: String) in Outcome(title: String(localized: "Couldn't Use Your Phone's Hotspot"), message: message, manualJoin: nil) }
        guard let error = error as? HotspotError else { return fail(UserText.describe(error)) }

        switch error {
        case let .Feature(error): return fail(UserText.describe(error))
        case .Busy: return fail(String(localized: "Phone Link is already working on it."))
        case .NotSupported: return fail(String(localized: "Your phone can't share its connection with this Mac."))
        case let .Bluetooth(error): return fail(UserText.describe(error))
        case .ConsentTimedOut: return fail(String(localized: "Your phone didn't answer. Allow instant hotspot in the notification on your phone, then try again."))
        case .NotTrusted: return fail(String(localized: "Your phone didn't accept this Mac. Reconnect Phone Link and try again."))
        case .Unreachable: return fail(String(localized: "This Mac couldn't reach your phone over Bluetooth. Keep the phone nearby and try again."))
        case .SideChannelNotOpen:
            return fail(String(localized: "This Mac couldn't find instant hotspot on your phone over Bluetooth. Make sure instant hotspot is on in Link to Windows. If it is, remove your phone in System Settings › Bluetooth and try again: Phone Link pairs with it again."))
        case .NoAnswer: return fail(String(localized: "Your phone didn't respond. Make sure this Mac is in your phone's list of paired Bluetooth devices, then try again."))
        case .PhoneError: return fail(String(localized: "Your phone couldn't turn on its hotspot."))
        case let .Unavailable(reason): return fail(Self.text(reason))
        case .NoCredentials, .InvalidCredentials: return fail(String(localized: "Your phone sent hotspot details this Mac can't use."))
        case let .NotVisible(ssid, passphrase):
            return Outcome(
                title: String(localized: "Your Phone's Hotspot Isn't Visible Yet"),
                message: String(localized: "Join “\(ssid)” from the Wi-Fi menu when it appears."),
                manualJoin: (ssid, passphrase)
            )
        case let .JoinFailed(ssid, passphrase, reason):
            return Outcome(
                title: String(localized: "Couldn't Join Your Phone's Hotspot"),
                message: String(localized: "Join “\(ssid)” from the Wi-Fi menu. (\(reason))"),
                manualJoin: (ssid, passphrase)
            )
        case .LeaveFailed: return fail(String(localized: "This Mac couldn't leave your phone's hotspot. Choose another network from the Wi-Fi menu."))
        case .Internal: return fail(String(localized: "Something went wrong in Phone Link."))
        }
    }

    private static func text(_ reason: HotspotUnavailable) -> String {
        switch reason {
        case .airplaneMode: return String(localized: "Turn off airplane mode on your phone.")
        case .blockedByPolicy: return String(localized: "Your phone's policy doesn't allow its hotspot.")
        case .dataPlan: return String(localized: "Your phone's data plan doesn't allow a hotspot.")
        case .dataSaver: return String(localized: "Turn off data saver on your phone.")
        case .passwordNotSet: return String(localized: "Set a password for your phone's hotspot first.")
        case .simNotReady: return String(localized: "Your phone's SIM isn't ready.")
        case .smartViewInUse: return String(localized: "Stop Smart View on your phone first.")
        case .wifiDirectInUse: return String(localized: "Stop Wi-Fi Direct on your phone first.")
        case .wirelessDexInUse: return String(localized: "Stop wireless DeX on your phone first.")
        case .linkToWindowsOff: return String(localized: "Turn on Link to Windows on your phone.")
        case .bluetoothPermission: return String(localized: "Allow Link to Windows to use Bluetooth on your phone.")
        case .other: return String(localized: "Your phone can't share its connection right now.")
        }
    }
}
