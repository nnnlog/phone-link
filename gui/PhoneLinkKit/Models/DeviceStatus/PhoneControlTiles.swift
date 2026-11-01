import PhoneLinkEngine

/// One control of the main window's phone area: what it is, the phone's state of it in a word, and
/// whether it can be used now. Unlike a menu item, a tile names its function and its state apart, so
/// the state stays readable at a glance ("Ringer" / "Vibrate").
public struct PhoneControlTile: Equatable, Identifiable, Sendable {
    public enum Kind: Sendable {
        case doNotDisturb, ringer, findPhone, hotspot, sendFiles, phoneScreen
    }

    public let id: Kind
    public let title: String
    /// The state in a word or two; `nil` when the control has no state to show (or it is unknown).
    public let state: String?
    public let symbol: String
    /// The control is on (Do Not Disturb on, the phone ringing): the tile is drawn highlighted.
    public let isOn: Bool
    /// A change is under way: the tile shows progress in place of its symbol.
    public let isBusy: Bool
    public let isEnabled: Bool

    public init(
        _ id: Kind,
        _ title: String,
        state: String? = nil,
        symbol: String,
        isOn: Bool = false,
        isBusy: Bool = false,
        isEnabled: Bool = true
    ) {
        self.id = id
        self.title = title
        self.state = state
        self.symbol = symbol
        self.isOn = isOn
        self.isBusy = isBusy
        self.isEnabled = isEnabled
    }
}

/// The phone's status bar as the phone area shows it: Wi-Fi, cellular, then the battery's
/// percentage and glyph. An item the engine does not know is left out.
public struct PhoneStatusSummary: Equatable, Sendable {
    public struct Glyph: Equatable, Sendable {
        public let symbol: String
        /// The symbol's variable value (signal bars), for symbols that draw one.
        public let variableValue: Double?
        /// What VoiceOver reads for the glyph.
        public let label: String
    }

    public let wifi: Glyph?
    public let cellular: Glyph?
    public let batteryPercent: Int32?
    public let battery: Glyph?
}

extension DeviceStatusModel {
    /// The phone's status bar; `nil` while the phone is not live or nothing is known.
    public var summary: PhoneStatusSummary? {
        let lines = lines
        guard !lines.isEmpty else { return nil }

        func glyph(_ item: DeviceStatusLine.Item) -> PhoneStatusSummary.Glyph? {
            lines.first { $0.id == item }.map { .init(symbol: $0.symbol, variableValue: $0.variableValue, label: $0.text) }
        }

        var percent: Int32?
        if case let .level(level, _, _) = status?.battery { percent = level }
        return PhoneStatusSummary(wifi: glyph(.wifi), cellular: glyph(.cellular), batteryPercent: percent, battery: glyph(.battery))
    }

    /// The Do Not Disturb tile; `nil` when the phone has no Do Not Disturb control.
    public var doNotDisturbTile: PhoneControlTile? {
        guard let status, let control = status.doNotDisturb else { return nil }
        let title = String(localized: "Do Not Disturb")
        let symbol = "moon.fill"
        guard status.live else { return PhoneControlTile(.doNotDisturb, title, symbol: symbol, isEnabled: false) }

        switch control.shown {
        case let .status(_, on):
            return PhoneControlTile(.doNotDisturb, title, state: on ? String(localized: "On", comment: "Control state") : String(localized: "Off", comment: "Control state"), symbol: symbol, isOn: on, isEnabled: control.enabled)
        case .unknown:
            return PhoneControlTile(.doNotDisturb, title, symbol: symbol, isEnabled: false)
        case .turningOn:
            return PhoneControlTile(.doNotDisturb, title, state: String(localized: "Turning On…"), symbol: symbol, isBusy: true, isEnabled: false)
        case .turningOff:
            return PhoneControlTile(.doNotDisturb, title, state: String(localized: "Turning Off…"), symbol: symbol, isBusy: true, isEnabled: false)
        }
    }

    /// The ringer tile, naming the current mode; a click moves to the next one. `nil` when the phone
    /// has no ringer control.
    public var ringerTile: PhoneControlTile? {
        guard let status, let control = status.ringer else { return nil }
        let title = String(localized: "Ringer")
        guard status.live else { return PhoneControlTile(.ringer, title, symbol: RingerMode.sound.symbol, isEnabled: false) }

        func pending(_ mode: RingerMode) -> PhoneControlTile {
            PhoneControlTile(.ringer, title, state: String(localized: "Switching…"), symbol: mode.symbol, isBusy: true, isEnabled: false)
        }

        switch control.shown {
        case let .mode(raw):
            // A mode outside 0/1/2 is still one the phone reported: shown as Sound, which the
            // ringer command switches it to.
            let mode = RingerMode(rawValue: raw) ?? .sound
            return PhoneControlTile(.ringer, title, state: mode.name, symbol: mode.symbol, isEnabled: control.enabled)
        case .unknown:
            return PhoneControlTile(.ringer, title, symbol: RingerMode.sound.symbol, isEnabled: false)
        case .turningVibrate:
            return pending(.vibrate)
        case .turningOff:
            return pending(.silent)
        case .turningOn:
            return pending(.sound)
        }
    }

    /// The find-my-phone tile: rings the phone; while it rings, on, with the seconds left, and a click
    /// stops it when stopping is offered. `nil` when the phone has no ring control.
    public var findPhoneTile: PhoneControlTile? {
        guard let status, let control = status.ringMyPhone else { return nil }
        let title = String(localized: "Find Phone")
        let symbol = "bell.and.waves.left.and.right"
        guard status.live else { return PhoneControlTile(.findPhone, title, symbol: symbol, isEnabled: false) }

        switch control.shown {
        case .unknown:
            return PhoneControlTile(.findPhone, title, symbol: symbol, isEnabled: false)
        case .off:
            return PhoneControlTile(.findPhone, title, symbol: symbol, isEnabled: control.enabled)
        case let .ringing(remainingSeconds, _):
            return PhoneControlTile(
                .findPhone,
                title,
                state: String(localized: "Ringing · \(remainingSeconds) s"),
                symbol: "bell.and.waves.left.and.right.fill",
                isOn: true,
                isEnabled: control.canStop && control.enabled
            )
        }
    }

    /// What a click on the find-my-phone tile does: ring, or stop the ringing.
    public func findPhone() {
        if case .ringing = status?.ringMyPhone?.shown {
            stopRinging()
        } else {
            ringPhone()
        }
    }
}

extension HotspotModel {
    /// The hotspot tile, on while this Mac is on the phone's hotspot; `nil` while the phone cannot
    /// share its connection.
    public var tile: PhoneControlTile? {
        switch item {
        case nil:
            nil
        case .connect:
            PhoneControlTile(.hotspot, String(localized: "Hotspot"), symbol: "personalhotspot")
        case .connected:
            PhoneControlTile(.hotspot, String(localized: "Hotspot"), state: String(localized: "Connected"), symbol: "personalhotspot", isOn: true)
        case let .working(work):
            PhoneControlTile(.hotspot, String(localized: "Hotspot"), state: work.state, symbol: "personalhotspot", isBusy: true, isEnabled: false)
        }
    }
}
