import Foundation
import Observation
import os
import PhoneLinkEngine

/// The selected phone's remote-control setting: whether it may lock, sleep, restart or shut down
/// this Mac. The Mac acts on the phone's request without asking, so the switch is the user's only
/// control; it is on until the user turns it off, and offered only while it applies.
///
/// Every value is the engine's: read when the engine starts and on every remote-control event, and
/// read back after a change.
@MainActor
@Observable
public final class RemoteControlModel {
    /// `nil` before the engine starts and while the phone is not in the account's device list.
    public private(set) var state: RemoteControlPhoneState?
    /// The last change of the user's that failed; the view shows it and clears it.
    public var failure: SettingsFailure?

    private let api: any RemoteControlApiProtocol
    /// The account was removed or the phone left it: nothing is read or changed until the next start.
    @ObservationIgnored private var suspended = false

    private static let log = AppLog(category: "remote-control")

    public init(api: any RemoteControlApiProtocol, hub: EngineEventHub) {
        self.api = api

        hub.onEvent { [weak self] event in
            self?.handle(event)
        }
        hub.onStart { [weak self] in
            self?.suspended = false
            self?.reload()
        }
    }

    /// "Allow this phone to control this Mac": the toggle, shown on only while it applies.
    public var allowsRemoteControl: Bool {
        get { state.map { $0.toggle && $0.applicable } ?? false }
        set { setToggle(newValue) }
    }

    /// The toggle applies — the phone supports remote control and it is on on this Mac — so the
    /// settings offer it only then.
    public var toggleApplies: Bool {
        state?.applicable == true
    }

    /// The line under the toggle.
    public var detail: String {
        guard let state else { return String(localized: "Your phone can lock, sleep, restart or shut down this Mac.") }
        if !state.capable {
            return String(localized: "This phone doesn't support controlling this Mac.")
        }
        if !state.applicable {
            return String(localized: "Remote control is turned off on this Mac.")
        }
        return state.v2Capable
            ? String(localized: "Your phone can lock, sleep, restart or shut down this Mac without asking first.")
            : String(localized: "Your phone can lock this Mac without asking first.")
    }

    public func reload() {
        guard !suspended else { return }
        do {
            state = try api.phoneState()
        } catch let error as FeatureApiError where error == .NotStarted || error == .NoPhone {
            state = nil
        } catch {
            Self.log.error("remote-control state not read: \(error: error)")
        }
    }

    private func setToggle(_ enabled: Bool) {
        guard !suspended else { return }
        do {
            try api.setPhoneToggle(enabled: enabled)
        } catch let error as FeatureApiError where error == .NotStarted {
            state = nil
            return
        } catch {
            Self.log.error("remote-control setting not changed: \(error: error)")
            failure = SettingsFailure(title: String(localized: "Couldn't Change the Setting"), message: UserText.describe(error))
        }

        reload()
    }

    private func handle(_ event: EngineEvent) {
        switch event {
        case .accountRemoved, .phoneUnlinked:
            suspended = true
            state = nil
        case .remoteControl, .phoneChanged:
            reload()
        default:
            break
        }
    }
}
