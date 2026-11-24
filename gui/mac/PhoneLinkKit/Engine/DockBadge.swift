import PhoneLinkEngine

/// The number on the app's Dock icon: what waits in the sidebar's sections, each thing counted once.
///
/// A notification that tells of a message or of a call is left out while Messages or Calls counts
/// that message or call itself, as Phone Link on Windows leaves it out of its taskbar number.
public enum DockBadge {
    /// What the sections hold.
    public struct Sections: Equatable, Sendable {
        /// Unread messages, as the sidebar counts them; `nil` while Messages shows no conversations.
        public var messages: Int?
        /// Missed calls not yet seen, as the sidebar counts them; `nil` while Calls shows no history.
        public var calls: Int?
        /// Where each notification of the list opens.
        public var notifications: [NotificationOpenTarget]

        public init(messages: Int?, calls: Int?, notifications: [NotificationOpenTarget]) {
            self.messages = messages
            self.calls = calls
            self.notifications = notifications
        }
    }

    public static func count(_ sections: Sections) -> Int {
        let notifications = sections.notifications.count { target in
            switch target {
            case .notifications: true
            case .messaging: sections.messages == nil
            case .calls: sections.calls == nil
            }
        }
        return (sections.messages ?? 0) + (sections.calls ?? 0) + notifications
    }

    /// The Dock tile's label: the count, or none when nothing waits.
    public static func label(_ sections: Sections) -> String? {
        let count = count(sections)
        return count > 0 ? String(count) : nil
    }
}
