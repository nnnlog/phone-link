import PhoneLinkEngine

/// One informational line of the phone menu: an item of the phone's status the engine knows; an
/// item it does not know is not shown.
public struct DeviceStatusLine: Equatable, Identifiable, Sendable {
    public enum Item: Sendable {
        case battery, cellular, wifi, bluetooth, mobileData, metered
    }

    public let id: Item
    public let text: String
    public let symbol: String
    /// The symbol's variable value (signal bars), for symbols that draw one.
    public let variableValue: Double?

    init(_ id: Item, _ text: String, _ symbol: String, variableValue: Double? = nil) {
        self.id = id
        self.text = text
        self.symbol = symbol
        self.variableValue = variableValue
    }

    /// The lines of `status`, in the menu's order; an unknown item has none, and a phone that is not
    /// connected (or turned every feature off) has none at all.
    public static func lines(for status: PhoneStatus?) -> [DeviceStatusLine] {
        guard let status, status.live else { return [] }
        var lines: [DeviceStatusLine] = []
        if case let .level(percent, charging, powerSave) = status.battery {
            let text = switch (charging, powerSave) {
            case (false, false): String(localized: "\(percent)%")
            case (true, false): String(localized: "\(percent)% · Charging")
            case (false, true): String(localized: "\(percent)% · Battery Saver")
            case (true, true): String(localized: "\(percent)% · Charging · Battery Saver")
            }
            lines.append(DeviceStatusLine(.battery, text, charging ? "battery.100percent.bolt" : batterySymbol(percent)))
        }

        switch status.cellular {
        case .unknown:
            break
        case .airplaneMode:
            lines.append(DeviceStatusLine(.cellular, String(localized: "Airplane Mode"), "airplane"))
        case .noSim:
            lines.append(DeviceStatusLine(.cellular, String(localized: "No SIM"), "simcard"))
        case let .bars(level, _, carrier, fourBarGlyphs):
            let name = carrier.flatMap { $0.isEmpty ? nil : $0 } ?? String(localized: "Mobile Network")
            lines.append(DeviceStatusLine(.cellular, name, "cellularbars", variableValue: cellularBars(level, fourBarGlyphs: fourBarGlyphs)))
        }

        switch status.wifi {
        case .unknown:
            break
        case .notConnected:
            lines.append(DeviceStatusLine(.wifi, String(localized: "Wi-Fi Not Connected"), "wifi.slash"))
        case let .bars(level):
            lines.append(DeviceStatusLine(.wifi, String(localized: "Wi-Fi"), "wifi", variableValue: fraction(level, of: 4)))
        }

        // SF Symbols has no Bluetooth glyph; radio waves stand in for it.
        switch status.bluetooth {
        case .on:
            lines.append(DeviceStatusLine(.bluetooth, String(localized: "Bluetooth On"), "antenna.radiowaves.left.and.right"))
        case .off:
            lines.append(DeviceStatusLine(.bluetooth, String(localized: "Bluetooth Off"), "antenna.radiowaves.left.and.right.slash"))
        case .unknown:
            break
        }

        if let mobileData = status.mobileData {
            lines.append(DeviceStatusLine(.mobileData, mobileData ? String(localized: "Mobile Data On") : String(localized: "Mobile Data Off"), "arrow.up.arrow.down"))
        }
        if status.metered {
            lines.append(DeviceStatusLine(.metered, String(localized: "Metered Connection"), "gauge.with.dots.needle.33percent"))
        }

        return lines
    }

    /// The battery glyph nearest to `percent` (SF Symbols draws quarter steps).
    private static func batterySymbol(_ percent: Int32) -> String {
        switch percent {
        case ..<13: "battery.0percent"
        case ..<38: "battery.25percent"
        case ..<63: "battery.50percent"
        case ..<88: "battery.75percent"
        default: "battery.100percent"
        }
    }

    private static func fraction(_ level: Int32, of max: Int32) -> Double {
        guard max > 0 else { return 0 }
        return Swift.min(Swift.max(Double(level) / Double(max), 0), 1)
    }

    /// The cellular level as bars: four when the engine says the phone allows them (its
    /// network-indicator API and a maximum of 4), else five — so level 4 of a maximum of 4 without
    /// that API is 4 of 5 bars. A level beyond the bar count shows none.
    private static func cellularBars(_ level: Int32, fourBarGlyphs: Bool) -> Double {
        let bars: Int32 = fourBarGlyphs ? 4 : 5
        guard (0...bars).contains(level) else { return 0 }
        return Double(level) / Double(bars)
    }
}

/// The Do Not Disturb item of the phone menu, with its confirmation timers.
public struct DeviceStatusDndItem: Equatable, Sendable {
    public let title: String
    /// Whether Do Not Disturb is on; `nil` while a change is pending (a line, not a toggle).
    public let isOn: Bool?
    public let isEnabled: Bool

    init(_ control: DndControlState, live: Bool) {
        switch control.shown {
        case let .status(_, on):
            title = String(localized: "Do Not Disturb")
            isOn = on
            isEnabled = control.enabled && live
        case .unknown:
            title = String(localized: "Do Not Disturb")
            isOn = false
            isEnabled = false
        case .turningOn:
            title = String(localized: "Turning On Do Not Disturb…")
            isOn = nil
            isEnabled = false
        case .turningOff:
            title = String(localized: "Turning Off Do Not Disturb…")
            isOn = nil
            isEnabled = false
        }
    }
}

/// The ringer item of the phone menu: one command that moves the ringer to its next mode, with a
/// second line naming that mode, since the command's effect is not visible in its title.
public struct DeviceStatusRingerItem: Equatable, Sendable {
    public let title: String
    /// What clicking does, while it can be clicked.
    public let hint: String?
    public let symbol: String
    public let isEnabled: Bool

    init(_ control: RingerControlState, live: Bool) {
        switch control.shown {
        case let .mode(raw):
            let enabled = control.enabled && live
            guard let mode = RingerMode(rawValue: raw) else {
                // A mode outside 0/1/2 is still a mode the phone reported: the control stays enabled
                // with Sound's symbol, and the ringer command switches it to Sound.
                self.init(
                    title: String(localized: "Ringer"),
                    hint: enabled ? String(localized: "Click to switch to \(RingerMode.sound.name)") : nil,
                    symbol: RingerMode.sound.symbol,
                    isEnabled: enabled
                )
                return
            }

            self.init(
                title: String(localized: "Ringer: \(mode.name)"),
                hint: enabled ? String(localized: "Click to switch to \(mode.next.name)") : nil,
                symbol: mode.symbol,
                isEnabled: enabled
            )
        case .unknown:
            self = .unknown
        case .turningVibrate:
            self.init(pendingTo: .vibrate)
        case .turningOff:
            self.init(pendingTo: .silent)
        case .turningOn:
            self.init(pendingTo: .sound)
        }
    }

    private init(title: String, hint: String?, symbol: String, isEnabled: Bool) {
        self.title = title
        self.hint = hint
        self.symbol = symbol
        self.isEnabled = isEnabled
    }

    private static let unknown = DeviceStatusRingerItem(title: String(localized: "Ringer"), hint: nil, symbol: RingerMode.sound.symbol, isEnabled: false)

    private init(pendingTo mode: RingerMode) {
        self.init(title: String(localized: "Ringer: Switching to \(mode.name)…"), hint: nil, symbol: mode.symbol, isEnabled: false)
    }
}

/// The ring-my-phone items of the phone menu: a Ring Phone command while the phone is not ringing;
/// while it rings, a line counting the seconds down and, when the engine offers it, Stop Ringing.
public struct DeviceStatusRingItem: Equatable, Sendable {
    /// The ring command's title, or while ringing the line naming the seconds left.
    public let title: String
    public let symbol: String
    /// The phone is ringing: `title` is a line, not a command.
    public let isRinging: Bool
    /// The ring command can be clicked.
    public let isEnabled: Bool
    /// The stop command's title, while ringing and stopping is offered.
    public let stopTitle: String?

    init(_ control: RingControlState, live: Bool) {
        symbol = "bell.and.waves.left.and.right"
        switch control.shown {
        case .unknown:
            title = String(localized: "Ring Phone")
            isRinging = false
            isEnabled = false
            stopTitle = nil
        case .off:
            title = String(localized: "Ring Phone")
            isRinging = false
            isEnabled = control.enabled && live
            stopTitle = nil
        case let .ringing(remainingSeconds, _):
            title = String(localized: "Ringing… \(remainingSeconds) s")
            isRinging = true
            isEnabled = false
            stopTitle = control.canStop && control.enabled && live ? String(localized: "Stop Ringing") : nil
        }
    }
}

/// The phone's ringer modes as the engine numbers them; the ringer command cycles them
/// sound → vibrate → silent → sound.
enum RingerMode: Int32 {
    case silent = 0
    case vibrate = 1
    case sound = 2

    var next: RingerMode {
        switch self {
        case .sound: .vibrate
        case .vibrate: .silent
        case .silent: .sound
        }
    }

    var name: String {
        switch self {
        case .sound: String(localized: "Sound", comment: "Ringer mode")
        case .vibrate: String(localized: "Vibrate", comment: "Ringer mode")
        case .silent: String(localized: "Silent", comment: "Ringer mode")
        }
    }

    var symbol: String {
        switch self {
        case .sound: "bell"
        case .vibrate: "iphone.radiowaves.left.and.right"
        case .silent: "bell.slash"
        }
    }
}

/// An alert the phone menu raises. A menu cannot host a SwiftUI alert, so the model queues these and
/// shows them one at a time as standard alerts.
public enum DeviceStatusAlert: Equatable, Sendable {
    /// A control needs something on the phone first; the default button runs the prompt's action.
    case prompt(StatusPrompt)
    /// A control the user clicked failed.
    case failure(title: String, message: String)

    public var title: String {
        switch self {
        case .prompt(.dndNotificationsPermission): String(localized: "Do Not Disturb Needs Notification Access")
        case .prompt(.ringerNotificationsPermission): String(localized: "Ringer Control Needs Notification Access")
        case .prompt(.dndRuleDisabled): String(localized: "Allow Do Not Disturb Changes on Your Phone")
        case .prompt(.dndTurnOffFailed): String(localized: "Couldn't Turn Off Do Not Disturb")
        case .prompt(.stopRinging): String(localized: "Stop Ringing Your Phone?")
        case let .failure(title, _): title
        }
    }

    public var message: String {
        switch self {
        case .prompt(.dndNotificationsPermission):
            String(localized: "To turn Do Not Disturb on or off from this Mac, allow notification access for Link to Windows on your phone.")
        case .prompt(.ringerNotificationsPermission):
            String(localized: "To change the ringer from this Mac, allow notification access for Link to Windows on your phone.")
        case .prompt(.dndRuleDisabled):
            String(localized: "Your phone doesn't let Link to Windows change Do Not Disturb. Open the setting on your phone to allow it.")
        case .prompt(.dndTurnOffFailed):
            String(localized: "Your phone didn't confirm the change. Open its Do Not Disturb settings to turn it off there.")
        case .prompt(.stopRinging):
            String(localized: "Your phone will stop ringing.")
        case let .failure(_, message):
            message
        }
    }

    /// The default button: the prompt's action, or OK for a failure.
    public var actionTitle: String {
        switch self {
        case .prompt(.dndNotificationsPermission), .prompt(.ringerNotificationsPermission): String(localized: "Allow on Phone")
        case .prompt(.dndRuleDisabled), .prompt(.dndTurnOffFailed): String(localized: "Open Settings on Phone")
        case .prompt(.stopRinging): String(localized: "Stop Ringing")
        case .failure: String(localized: "OK")
        }
    }

    /// A prompt can be declined; a failure only acknowledged.
    public var isPrompt: Bool {
        if case .prompt = self { return true }
        return false
    }
}
