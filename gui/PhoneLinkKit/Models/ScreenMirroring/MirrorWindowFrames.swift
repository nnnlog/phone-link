import Foundation

/// Where mirroring windows keep their frames between windows of the same app, per layout: the
/// Expanded layout has a frame of its own, the Default and Compact layouts share one. A frame is saved
/// under the layout the window shows (`saveLayout`) and read once, when a window of the app opens
/// (`openingFrame`), or on a layout switch for the layout switched to (`frame`).
public struct MirrorWindowFrames {
    /// One kept frame of an app.
    public enum Slot: Equatable, Sendable {
        /// The Default and Compact layouts.
        case standard
        /// The Expanded layout.
        case expanded
    }

    /// The screen layouts as the phone numbers them.
    public static let defaultLayout: Int32 = 0
    public static let expandedLayout: Int32 = 2

    private let defaults: UserDefaults

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    /// The slot of screen layout `layout`.
    public static func slot(_ layout: Int32) -> Slot {
        layout == expandedLayout ? .expanded : .standard
    }

    /// The layout a window's frame is saved under: the layout the window shows, else (none known yet,
    /// or the Default layout) the app's last one, `last`.
    public static func saveLayout(current: Int32?, last: Int32?) -> Int32 {
        if let current, current != defaultLayout { return current }
        return last ?? defaultLayout
    }

    /// The frame kept in `slot` for `package`; `nil` when none is, or it does not read as a frame.
    public func frame(package: String, slot: Slot) -> CGRect? {
        guard let text = defaults.string(forKey: Self.key(package, slot)) else { return nil }
        let frame = NSRectFromString(text)
        return frame.width > 0 && frame.height > 0 ? frame : nil
    }

    /// The frame a new window of `package` opens with: the one kept for the app's last layout, else —
    /// when that is the Expanded layout and it has none — the Default one.
    public func openingFrame(package: String, lastLayout: Int32) -> CGRect? {
        let slot = Self.slot(lastLayout)
        if let frame = frame(package: package, slot: slot) { return frame }
        return slot == .expanded ? frame(package: package, slot: .standard) : nil
    }

    public func save(_ frame: CGRect, package: String, slot: Slot) {
        defaults.set(NSStringFromRect(frame), forKey: Self.key(package, slot))
    }

    private static func key(_ package: String, _ slot: Slot) -> String {
        switch slot {
        case .standard: "MirrorWindowFrame.\(package)"
        case .expanded: "MirrorWindowFrame.\(package).expanded"
        }
    }
}
