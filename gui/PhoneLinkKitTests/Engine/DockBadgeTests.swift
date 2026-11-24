import PhoneLinkEngine
import Testing
@testable import PhoneLinkKit

struct DockBadgeTests {
    @Test func theBadgeIsTheSumOfWhatWaitsCountedOnce() {
        // (unread messages, unseen missed calls, notifications' open targets, label)
        let cases: [(Int?, Int?, [NotificationOpenTarget], String?)] = [
            (nil, nil, [], nil),
            (0, 0, [], nil),
            (3, 1, [.notifications, .notifications], "6"),
            // A message's or a call's own notification is not counted beside the message or the call.
            (2, 1, [.messaging, .messaging, .calls, .notifications], "4"),
            // While Messages or Calls counts nothing itself, the notification is all that tells of it.
            (nil, 1, [.messaging, .calls], "2"),
            (2, nil, [.messaging, .calls], "3"),
        ]

        for (messages, calls, notifications, label) in cases {
            let sections = DockBadge.Sections(messages: messages, calls: calls, notifications: notifications)
            #expect(DockBadge.label(sections) == label, "messages \(String(describing: messages)), calls \(String(describing: calls)), \(notifications)")
        }
    }
}
