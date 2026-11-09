import AppKit
import CryptoKit
import Foundation
import os
import PhoneLinkEngine
import UniformTypeIdentifiers
import UserNotifications

private let logger = AppLog(category: "edges")

/// The engine's `SystemNotifier` over `UNUserNotificationCenter`: the presentation on macOS. The
/// engine decides whether and what to post; this class presents it and reports back what the user
/// did with it.
///
/// Actions come from one category per distinct action layout. Registering categories replaces the
/// app's whole set, so every category this run registered, and the category of every notification
/// still shown from an earlier run, stays in the set; a shown notification whose category is gone
/// loses its actions.
public final class UserNotifier: NSObject, SystemNotifier, UNUserNotificationCenterDelegate, @unchecked Sendable {
    private struct State {
        var responseHandler: (@Sendable (String, [String: String], NotificationResponse) -> Void)?
        /// The registered categories by identifier.
        var categories: [String: UNNotificationCategory] = [:]
    }

    /// How long the engine's thread waits for the center to answer a post or an authorization query.
    static let centerTimeout: TimeInterval = 2

    /// Everything mutable; the reason this class is `@unchecked Sendable`.
    private let state = OSAllocatedUnfairLock(uncheckedState: State())

    override public init() {}

    /// Receives what the user did with a posted notification, on any thread.
    public var responseHandler: (@Sendable (String, [String: String], NotificationResponse) -> Void)? {
        get { state.withLockUnchecked { $0.responseHandler } }
        set { state.withLockUnchecked { $0.responseHandler = newValue } }
    }

    /// Becomes the UNUserNotificationCenter delegate and asks for authorization (alert, sound, badge).
    public func activate() {
        let center = UNUserNotificationCenter.current()
        center.delegate = self
        center.requestAuthorization(options: [.alert, .sound, .badge]) { granted, error in
            if let error {
                logger.error("notification authorization not requested: \(error: error)")
            } else {
                logger.info("notification authorization answered; allowed \(granted)")
            }
        }

        keepShownCategories()
    }

    // MARK: - SystemNotifier

    public func post(notification: SystemNotification) throws {
        let content = UNMutableNotificationContent()
        content.title = notification.title
        content.subtitle = notification.subtitle ?? ""
        content.body = notification.body
        content.threadIdentifier = notification.threadId ?? ""
        content.userInfo = notification.userInfo
        content.interruptionLevel = notification.alert ? .active : .passive
        if notification.alert {
            content.sound = .default
        }

        content.categoryIdentifier = register(notification.actions)
        if let image = notification.image, let attachment = Self.attachment(image) {
            content.attachments = [attachment]
        }

        let request = UNNotificationRequest(identifier: notification.id, content: content, trigger: nil)
        // The engine records what it posted (the low-battery time, the last new-photo popup) only
        // when this returns, so it waits for the center's answer.
        let refusal = Self.answer(Error?.self) { done in
            UNUserNotificationCenter.current().add(request) { error in done(error) }
        }

        switch refusal {
        case .none:
            throw EdgeError.Unexpected(message: "the notification center did not answer a post within \(Self.centerTimeout) s")
        case let .some(.some(error)):
            throw EdgeError.Unexpected(message: "the notification center refused a post: \(error.localizedDescription)")
        case .some(.none):
            return
        }
    }

    public func withdraw(ids: [String]) throws {
        let center = UNUserNotificationCenter.current()
        center.removeDeliveredNotifications(withIdentifiers: ids)
        center.removePendingNotificationRequests(withIdentifiers: ids)
    }

    /// The status as the center reports it now: nothing announces a change the user makes in System
    /// Settings, least of all to a menu bar app in the background. No answer in time counts as not
    /// allowed, so nothing is posted (or recorded as posted) on a guess.
    public func isAuthorized() -> Bool {
        let status = Self.answer(UNAuthorizationStatus.self) { done in
            UNUserNotificationCenter.current().getNotificationSettings { settings in done(settings.authorizationStatus) }
        }

        guard let status else {
            logger.error("notification authorization not reported within \(Self.centerTimeout) s; nothing is posted")
            return false
        }
        return Self.allowsPosting(status)
    }

    /// Waits on the calling (engine) thread for an answer the center hands to its completion handler
    /// on a queue of its own; `nil` when none comes within `timeout`.
    static func answer<Value>(
        _: Value.Type,
        within timeout: TimeInterval = centerTimeout,
        _ ask: (@escaping @Sendable (Value) -> Void) -> Void
    ) -> Value? {
        let done = DispatchSemaphore(value: 0)
        let result = OSAllocatedUnfairLock<Value?>(uncheckedState: nil)
        ask { value in
            result.withLockUnchecked { $0 = value }
            done.signal()
        }

        guard done.wait(timeout: .now() + timeout) == .success else { return nil }
        return result.withLockUnchecked { $0 }
    }

    // MARK: - UNUserNotificationCenterDelegate

    public func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let request = response.notification.request
        let answer = Self.response(
            actionIdentifier: response.actionIdentifier,
            replyText: (response as? UNTextInputNotificationResponse)?.userText
        )

        let handler = state.withLockUnchecked { $0.responseHandler }
        handler?(request.identifier, Self.stringUserInfo(request.content.userInfo), answer)
        completionHandler()
    }

    /// Everything posted is shown, also while the app is in front, the way the engine chose: an alert
    /// as a banner with its sound, a silent delivery in the list only.
    public func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([.banner, .list, .sound])
    }

    /// How a notification is shown while the app is in front: `post` marks a silent delivery
    /// `passive`, which must not pop up as a banner there either.
    static func presentationOptions(for level: UNNotificationInterruptionLevel) -> UNNotificationPresentationOptions {
        level == .passive ? [.list] : [.banner, .list, .sound]
    }

    // MARK: - Authorization

    /// Authorized or provisional (quiet delivery) may post; the ephemeral status exists only for
    /// iOS App Clips.
    static func allowsPosting(_ status: UNAuthorizationStatus) -> Bool {
        switch status {
        case .authorized, .provisional:
            true
        case .notDetermined, .denied:
            false
        @unknown default:
            false
        }
    }

    // MARK: - Categories

    /// The identifier of the category for `actions`, registered with the center the first time
    /// the layout appears. The center is called under the lock so that a later registration never
    /// overtakes an earlier one with a smaller set.
    private func register(_ actions: [NotificationAction]) -> String {
        let identifier = Self.categoryIdentifier(for: actions)
        state.withLockUnchecked { state in
            guard state.categories[identifier] == nil else { return }
            state.categories[identifier] = Self.category(identifier: identifier, actions: actions)
            UNUserNotificationCenter.current().setNotificationCategories(Set(state.categories.values))
        }
        return identifier
    }

    /// Adds to the set the registered categories of notifications still shown from an earlier run.
    private func keepShownCategories() {
        UNUserNotificationCenter.current().getDeliveredNotifications { [weak self] delivered in
            guard let self else { return }
            let shown = Set(delivered.map(\.request.content.categoryIdentifier))
            UNUserNotificationCenter.current().getNotificationCategories { [weak self] categories in
                self?.adopt(categories.filter { shown.contains($0.identifier) })
            }
        }
    }

    private func adopt(_ categories: Set<UNNotificationCategory>) {
        state.withLockUnchecked { state in
            let new = categories.filter { state.categories[$0.identifier] == nil }
            guard !new.isEmpty else { return }
            for category in new {
                state.categories[category.identifier] = category
            }
            UNUserNotificationCenter.current().setNotificationCategories(Set(state.categories.values))
        }
    }

    /// A layout's category identifier: the SHA-256 of every field of its actions, so equal layouts
    /// share one category within a run and across runs. Each field is written with its length (an
    /// absent one as a single marker byte), so no two layouts serialize alike.
    static func categoryIdentifier(for actions: [NotificationAction]) -> String {
        var hash = SHA256()
        for action in actions {
            for field in [action.id, action.title, action.textInput?.buttonTitle, action.textInput?.placeholder] {
                guard let field else {
                    hash.update(data: [0])
                    continue
                }

                let bytes = Data(field.utf8)
                hash.update(data: [1])
                withUnsafeBytes(of: UInt64(bytes.count).bigEndian) { hash.update(bufferPointer: $0) }
                hash.update(data: bytes)
            }
        }
        return "actions-" + hash.finalize().map { String(format: "%02x", $0) }.joined()
    }

    /// One action per entry in order — a text-input action for an entry with `textInput` — and the
    /// custom dismiss action, so that dismissals reach the delegate.
    static func category(identifier: String, actions: [NotificationAction]) -> UNNotificationCategory {
        let buttons = actions.map { action -> UNNotificationAction in
            guard let input = action.textInput else {
                return UNNotificationAction(identifier: action.id, title: action.title, options: [])
            }
            return UNTextInputNotificationAction(
                identifier: action.id,
                title: action.title,
                options: [],
                textInputButtonTitle: input.buttonTitle,
                textInputPlaceholder: input.placeholder
            )
        }

        return UNNotificationCategory(identifier: identifier, actions: buttons, intentIdentifiers: [], options: [.customDismissAction])
    }

    // MARK: - Responses

    /// The engine's form of a response: the notification clicked, dismissed, a text-input action
    /// with what the user typed, or a button.
    static func response(actionIdentifier: String, replyText: String?) -> NotificationResponse {
        switch actionIdentifier {
        case UNNotificationDefaultActionIdentifier:
            return .activated
        case UNNotificationDismissActionIdentifier:
            return .dismissed
        default:
            if let replyText {
                return .reply(actionId: actionIdentifier, text: replyText)
            }
            return .action(actionId: actionIdentifier)
        }
    }

    /// The map the engine posted. `post` sets only strings, so anything else did not come from the
    /// engine: it is logged and left out.
    static func stringUserInfo(_ userInfo: [AnyHashable: Any]) -> [String: String] {
        var map: [String: String] = [:]
        for (key, value) in userInfo {
            guard let key = key as? String, let value = value as? String else {
                logger.notice("notification userInfo entry not a string; it is left out")
                continue
            }
            map[key] = value
        }
        return map
    }

    // MARK: - Images

    /// The image as an attachment: written to a temporary file named for the type ImageIO detects,
    /// which the center moves into its own store. `nil` — the notification is posted without it —
    /// when ImageIO recognizes no image type or the center refuses the file.
    private static func attachment(_ image: Data) -> UNNotificationAttachment? {
        guard let type = EdgeImageIO.type(of: image), let fileExtension = type.preferredFilenameExtension else {
            logger.notice("notification image in no format ImageIO recognizes; posted without it")
            return nil
        }

        let url = FileManager.default.temporaryDirectory.appendingPathComponent("\(UUID().uuidString).\(fileExtension)")
        do {
            try image.write(to: url)
            return try UNNotificationAttachment(
                identifier: "image",
                url: url,
                options: [UNNotificationAttachmentOptionsTypeHintKey: type.identifier]
            )
        } catch {
            try? FileManager.default.removeItem(at: url)
            logger.notice("notification image not attached: \(error: error); posted without it")
            return nil
        }
    }
}
