import Foundation
import PhoneLinkEngine
import Testing
import UserNotifications
@testable import PhoneLinkKit

/// The notifier's mappings; nothing here posts a notification or reaches the notification center.
struct UserNotifierTests {
    private static let reply = NotificationAction(
        id: "reply/0/0",
        title: "Reply",
        textInput: TextInput(buttonTitle: "Send", placeholder: "Message")
    )
    private static let button = NotificationAction(id: "disable-throttling", title: "Turn Off", textInput: nil)

    @Test func equalLayoutsShareACategory() {
        let copy = NotificationAction(id: "reply/0/0", title: "Reply", textInput: TextInput(buttonTitle: "Send", placeholder: "Message"))
        #expect(UserNotifier.categoryIdentifier(for: [Self.reply]) == UserNotifier.categoryIdentifier(for: [copy]))
        #expect(UserNotifier.categoryIdentifier(for: []) == UserNotifier.categoryIdentifier(for: []))
    }

    @Test func everyFieldAndTheOrderTellLayoutsApart() {
        let layouts: [[NotificationAction]] = [
            [],
            [Self.reply],
            [Self.button],
            [Self.reply, Self.button],
            [Self.button, Self.reply],
            [NotificationAction(id: "reply/0/0", title: "Answer", textInput: Self.reply.textInput)],
            [NotificationAction(id: "reply/1/0", title: "Reply", textInput: Self.reply.textInput)],
            [NotificationAction(id: "reply/0/0", title: "Reply", textInput: TextInput(buttonTitle: "Go", placeholder: "Message"))],
            [NotificationAction(id: "reply/0/0", title: "Reply", textInput: TextInput(buttonTitle: "Send", placeholder: ""))],
            [NotificationAction(id: "reply/0/0", title: "Reply", textInput: nil)],
            // The same characters split differently between fields.
            [NotificationAction(id: "ab", title: "c", textInput: nil)],
            [NotificationAction(id: "a", title: "bc", textInput: nil)],
        ]

        let identifiers = layouts.map(UserNotifier.categoryIdentifier(for:))
        #expect(Set(identifiers).count == layouts.count)
    }

    @Test func aCategoryHasOneActionPerEntryAndReportsDismissals() throws {
        let identifier = UserNotifier.categoryIdentifier(for: [Self.reply, Self.button])
        let category = UserNotifier.category(identifier: identifier, actions: [Self.reply, Self.button])

        #expect(category.identifier == identifier)
        #expect(category.options.contains(.customDismissAction))
        #expect(category.actions.map(\.identifier) == ["reply/0/0", "disable-throttling"])

        let reply = try #require(category.actions.first as? UNTextInputNotificationAction)
        #expect(reply.title == "Reply")
        #expect(reply.textInputButtonTitle == "Send")
        #expect(reply.textInputPlaceholder == "Message")

        let button = try #require(category.actions.last)
        #expect(!(button is UNTextInputNotificationAction))
        #expect(button.title == "Turn Off")
    }

    @Test func aLayoutWithoutActionsStillReportsDismissals() {
        let category = UserNotifier.category(identifier: UserNotifier.categoryIdentifier(for: []), actions: [])
        #expect(category.actions.isEmpty)
        #expect(category.options.contains(.customDismissAction))
    }

    @Test func responsesMapToTheEnginesForm() {
        #expect(UserNotifier.response(actionIdentifier: UNNotificationDefaultActionIdentifier, replyText: nil) == .activated)
        #expect(UserNotifier.response(actionIdentifier: UNNotificationDismissActionIdentifier, replyText: nil) == .dismissed)
        #expect(UserNotifier.response(actionIdentifier: "reply/0/0", replyText: "On my way") == .reply(actionId: "reply/0/0", text: "On my way"))
        #expect(UserNotifier.response(actionIdentifier: "reply/0/0", replyText: "") == .reply(actionId: "reply/0/0", text: ""))
        #expect(UserNotifier.response(actionIdentifier: "disable-throttling", replyText: nil) == .action(actionId: "disable-throttling"))
    }

    @Test func userInfoComesBackAsTheStringMapPosted() {
        let posted: [String: String] = ["kind": "notification", "key": "0|com.example|1|null|2"]
        #expect(UserNotifier.stringUserInfo(posted) == posted)

        let foreign: [AnyHashable: Any] = ["kind": "notification", "count": 3, 7: "seven"]
        #expect(UserNotifier.stringUserInfo(foreign) == ["kind": "notification"])
    }

    @Test func aSilentDeliveryStaysOutOfBannersWhileTheAppIsInFront() {
        #expect(UserNotifier.presentationOptions(for: .passive) == [.list])
        #expect(UserNotifier.presentationOptions(for: .active) == [.banner, .list, .sound])
    }

    @Test func theCentersAnswerIsAwaitedOnTheCallingThread() {
        #expect(UserNotifier.answer(Int.self) { done in done(1) } == 1)

        let late = UserNotifier.answer(Int.self) { done in
            DispatchQueue.global().asyncAfter(deadline: .now() + 0.05) { done(2) }
        }
        #expect(late == 2)

        let refusal = UserNotifier.answer((any Error)?.self) { done in done(CocoaError(.featureUnsupported)) }
        #expect(refusal??.localizedDescription != nil)
    }

    @Test func noAnswerInTimeIsNoAnswer() {
        #expect(UserNotifier.answer(Int.self, within: 0.05) { _ in } == nil)
    }

    @Test func authorizedAndProvisionalMayPost() {
        #expect(UserNotifier.allowsPosting(.authorized))
        #expect(UserNotifier.allowsPosting(.provisional))
        #expect(!UserNotifier.allowsPosting(.denied))
        #expect(!UserNotifier.allowsPosting(.notDetermined))
    }
}
