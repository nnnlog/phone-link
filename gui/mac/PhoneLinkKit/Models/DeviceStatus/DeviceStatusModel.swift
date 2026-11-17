import AppKit
import Observation
import os
import PhoneLinkEngine

private let logger = AppLog(category: "device-status")

/// Shows an alert of the phone menu and says whether the user chose its default button.
typealias DeviceStatusAlertPresenter = @MainActor (DeviceStatusAlert) async -> Bool

/// The phone's status and its Do Not Disturb, ringer and ring-my-phone controls, for the menu bar extra.
@MainActor
@Observable
public final class DeviceStatusModel {
    /// The selected phone's status; `nil` until the engine has started.
    public private(set) var status: PhoneStatus?

    /// Alerts waiting for the one on screen (alerts are modal, so they come one at a time).
    @ObservationIgnored private var queue: [DeviceStatusAlert] = []
    @ObservationIgnored private var showing: DeviceStatusAlert?
    /// Presents the queue until it is empty.
    @ObservationIgnored private(set) var presentation: Task<Void, Never>?

    private let api: any DeviceStatusApiProtocol
    private let present: DeviceStatusAlertPresenter
    private let openSettings: @MainActor () -> Void

    public convenience init(api: any DeviceStatusApiProtocol, hub: EngineEventHub) {
        self.init(api: api, hub: hub, present: Self.runAlert, openSettings: Self.openSettingsWindow)
    }

    init(
        api: any DeviceStatusApiProtocol,
        hub: EngineEventHub,
        present: @escaping DeviceStatusAlertPresenter,
        openSettings: @escaping @MainActor () -> Void
    ) {
        self.api = api
        self.present = present
        self.openSettings = openSettings

        hub.onEvent { [weak self] event in
            self?.handle(event)
        }
        hub.onStart { [weak self] in
            self?.reload()
        }
    }

    // MARK: - What the menu shows

    /// The status lines; empty while nothing is known (the menu then says the status is unavailable).
    public var lines: [DeviceStatusLine] {
        DeviceStatusLine.lines(for: status)
    }

    /// `nil` when the phone has no Do Not Disturb control.
    public var doNotDisturb: DeviceStatusDndItem? {
        guard let status, let control = status.doNotDisturb else { return nil }
        return DeviceStatusDndItem(control, live: status.live)
    }

    /// `nil` when the phone has no ringer control.
    public var ringer: DeviceStatusRingerItem? {
        guard let status, let control = status.ringer else { return nil }
        return DeviceStatusRingerItem(control, live: status.live)
    }

    /// `nil` when the phone has no ring-my-phone control.
    public var ringMyPhone: DeviceStatusRingItem? {
        guard let status, let control = status.ringMyPhone else { return nil }
        return DeviceStatusRingItem(control, live: status.live)
    }

    // MARK: - Loading

    public func reload() {
        do {
            status = try api.status()
        } catch let error as FeatureApiError where error == .NotStarted || error == .NoPhone {
            status = nil
        } catch {
            logger.error("phone status not read: \(error: error)")
        }
    }

    private func handle(_ event: EngineEvent) {
        switch event {
        case let .deviceStatus(change):
            switch change {
            case .changed:
                reload()
            case let .promptRequested(prompt):
                enqueue(.prompt(prompt))
            case let .promptWithdrawn(prompt):
                // The alert on screen stays (it is modal); a queued one is no longer shown.
                queue.removeAll { $0 == .prompt(prompt) }
            case .lowBatteryNotificationActivated:
                openSettings()
            }
        case .resync(feature: .deviceStatus), .phoneSession, .allFeaturesDisabled, .phoneChanged, .phoneUnlinked, .accountRemoved:
            reload()
        default:
            break
        }
    }

    // MARK: - Controls

    public func toggleDoNotDisturb() {
        activate(String(localized: "Couldn't Change Do Not Disturb"), "do not disturb not toggled") { try $0.toggleDoNotDisturb() }
    }

    /// Moves the ringer to its next mode (sound → vibrate → silent → sound).
    public func cycleRingerMode() {
        activate(String(localized: "Couldn't Change the Ringer"), "ringer mode not changed") { try $0.cycleRingerMode() }
    }

    /// Rings the phone; the menu counts the seconds down while it rings.
    public func ringPhone() {
        activate(String(localized: "Couldn't Ring Your Phone"), "phone not rung") { try $0.ringMyPhone() }
    }

    /// Stops the ringing phone: the first stop of a ring asks for confirmation (a prompt).
    public func stopRinging() {
        activate(String(localized: "Couldn't Stop Ringing"), "ringing not stopped") { try $0.stopRinging() }
    }

    private func activate(_ failureTitle: String, _ subject: StaticString, _ control: (any DeviceStatusApiProtocol) throws -> StatusControlOutcome) {
        do {
            switch try control(api) {
            case .sent:
                break
            case let .prompt(prompt):
                enqueue(.prompt(prompt))
            case .notAvailable:
                logger.debug("phone control not available; nothing sent")
            }
        } catch let error as FeatureApiError where error == .NotStarted {
            logger.debug("phone control dropped; the engine has not started")
        } catch {
            logger.error("\(subject.description, privacy: .public): \(error: error)")
            enqueue(.failure(title: failureTitle, message: UserText.describe(error)))
        }

        // The pending state shows at once.
        reload()
    }

    // MARK: - Alerts

    private func enqueue(_ alert: DeviceStatusAlert) {
        guard alert != showing, !queue.contains(alert) else { return }
        queue.append(alert)

        guard presentation == nil else { return }
        presentation = Task {
            await presentQueue()
        }
    }

    private func presentQueue() async {
        while !queue.isEmpty {
            let alert = queue.removeFirst()
            showing = alert
            let accepted = await present(alert)
            showing = nil
            if accepted, case let .prompt(prompt) = alert {
                accept(prompt)
            }
        }
        presentation = nil
    }

    private func accept(_ prompt: StatusPrompt) {
        do {
            try api.acceptPrompt(prompt: prompt)
        } catch let error as FeatureApiError where error == .NotStarted {
            logger.debug("accepted prompt dropped; the engine has not started")
        } catch {
            logger.error("accepted prompt not sent: \(error: error)")
            queue.append(.failure(title: String(localized: "Couldn't Send Request to Phone"), message: UserText.describe(error)))
        }
    }

    /// A standard alert for an app without a key window: the app comes to the front first.
    private static func runAlert(_ alert: DeviceStatusAlert) async -> Bool {
        let panel = NSAlert()
        panel.messageText = alert.title
        panel.informativeText = alert.message
        panel.addButton(withTitle: alert.actionTitle)
        if alert.isPrompt {
            panel.addButton(withTitle: String(localized: "Cancel"))
        }

        NSApp.activate()
        return panel.runModal() == .alertFirstButtonReturn && alert.isPrompt
    }

    /// Opens the Settings window on the Notifications pane, which holds the low-battery switch, from
    /// outside any view. SwiftUI ignores `showSettingsWindow:` sent from code since macOS 14 and offers
    /// `openSettings` only to views, so this runs the app menu's own Settings item.
    private static func openSettingsWindow() {
        UserDefaults.standard.set(SettingsPane.notifications.rawValue, forKey: PreferenceKeys.settingsPane)
        NSApp.activate()

        guard let appMenu = NSApp.mainMenu?.items.first?.submenu,
              let index = appMenu.items.firstIndex(where: { $0.keyEquivalent == "," && $0.keyEquivalentModifierMask.contains(.command) })
        else {
            preconditionFailure("the app menu has no Settings item (the app declares a Settings scene)")
        }
        appMenu.performActionForItem(at: index)
    }
}
