import Foundation
import Observation
import os
import PhoneLinkEngine

private let logger = AppLog(category: "notifications")

/// What the notification list column shows. The setup states come first, in priority order: the
/// Mac's switch is asked about only once the phone has given access (or cannot), so a new user is
/// first sent to the phone.
public enum NotificationsContent: Equatable, Sendable {
    /// Nothing loaded yet.
    case loading
    /// The phone has no notification listener for Link to Windows.
    case unavailable
    /// Link to Windows on the phone (Android 13 or later) may not send notifications; the user allows
    /// it on the phone, through the link the post-notification sheet shows.
    case needsPostNotificationPermission
    /// "Show notifications from my phone" is off on this Mac.
    case turnedOff
    /// The phone answered the access request "device not supported" (a low-memory phone).
    case lowMemoryPhone
    /// The phone has not given Link to Windows notification access.
    case needsAccess
    /// The list is empty while a full sync is in flight.
    case syncing
    case empty
    case list
    /// The state or the list could not be read (the reason); Try Again reads them again.
    case failed(String)
}

/// Why something the user did failed, for an alert.
public struct NotificationsFailure: Equatable, Sendable {
    public let title: String
    public let message: String

    public init(title: String, message: String) {
        self.title = title
        self.message = message
    }
}

/// A part of the notification list: the pinned notifications, or the unpinned ones posted on one day.
public struct NotificationSection: Identifiable, Equatable, Sendable {
    public enum ID: Hashable, Sendable {
        case pinned
        /// The start of the day, in the calendar the sections were made with.
        case day(Date)
    }

    public let id: ID
    /// "Pinned", "Today", "Yesterday", the weekday within the last week, else the date.
    public let title: String
    /// In the list's order, newest first.
    public let notifications: [PhoneNotification]
}

/// The phone's notifications for the main window: the list, the selected item, the per-app commands
/// of a row's context menu, and the setup states before there is a list.
@MainActor
@Observable
public final class NotificationsModel {
    /// The phone's notifications, newest first.
    public private(set) var notifications: [PhoneNotification] = []
    public private(set) var state: NotificationsState?
    /// The selected notification's key.
    public var selection: String?
    /// A reply field to focus once its notification shows (the row's Reply… command).
    public var replyFocusRequest: String?
    /// Why the last action the user started failed.
    public var lastFailure: NotificationsFailure?
    public private(set) var isClearing = false
    public private(set) var isRequestingAccess = false
    /// The post-notification sheet while it is open; it closes by itself once the stage ends.
    public var postNotificationHelp: PostNotificationHelp?

    /// The listed apps' settings, by package: a row's context menu offers turning the app on or off
    /// and muting it from these.
    private var appSettings: [String: AppNotificationSetting] = [:]
    /// Actions and replies queued from this window that have not finished. A finished one queued
    /// elsewhere — a banner's, whose failure the engine announces itself, or one from an earlier
    /// run — is not the user's action here.
    @ObservationIgnored private var pendingActions: [PendingAction: Int] = [:]
    /// Why the state or the list could not be read, until a read succeeds.
    private var stateFailure: String?
    private var listFailure: String?
    /// The undo managers holding a Turn Off of this model's: their actions name one phone's apps, so
    /// they are dropped when another phone may be selected (a new start, the phone unlinked, the
    /// account removed).
    @ObservationIgnored private let undoManagers = NSHashTable<UndoManager>.weakObjects()

    private let api: any NotificationsApiProtocol
    private let navigator: AppNavigator

    public init(api: any NotificationsApiProtocol, hub: EngineEventHub, navigator: AppNavigator) {
        self.api = api
        self.navigator = navigator

        hub.onEvent { [weak self] event in
            self?.handle(event)
        }
        hub.onStart { [weak self] in
            self?.dropUndoActions()
            self?.reload()
        }
    }

    // MARK: - What the views show

    public var content: NotificationsContent {
        if let failure = stateFailure ?? listFailure { return .failed(failure) }
        guard let state else { return .loading }
        if !state.capable { return .unavailable }
        if state.postNotificationNeeded { return .needsPostNotificationPermission }
        if !state.pcPermission && (state.devicePermission || state.lowRamDevice) { return .turnedOff }
        if state.lowRamDevice { return .lowMemoryPhone }
        if !state.devicePermission { return .needsAccess }
        if notifications.isEmpty { return state.syncing ? .syncing : .empty }
        return .list
    }

    /// The sidebar badge: the number of notifications the list shows.
    public var badgeCount: Int {
        content == .list ? notifications.count : 0
    }

    public var selectedNotification: PhoneNotification? {
        guard content == .list, let selection else { return nil }
        return notifications.first { $0.key == selection }
    }

    /// Clear All has something to clear: it leaves pinned notifications.
    public var canClearAll: Bool {
        !isClearing && notifications.contains { !$0.pinned }
    }

    /// The per-app commands apply: only a phone that reports its app list keeps per-app settings.
    public var appCommandsAvailable: Bool {
        state?.appListSupported == true
    }

    /// The app of `notification` shows its notifications in the list (its phone-side switch).
    public func isAppOn(_ notification: PhoneNotification) -> Bool {
        appSettings[notification.packageName]?.flags.feed ?? true
    }

    public func isAppMuted(_ notification: PhoneNotification, now: Date = .now) -> Bool {
        guard let until = appSettings[notification.packageName]?.mutedUntilMs else { return false }
        return until > Self.unixMs(now)
    }

    /// Muting applies to an app that is on, while every banner is not already muted.
    public func canMuteApp(_ notification: PhoneNotification, now: Date = .now) -> Bool {
        guard isAppOn(notification) else { return false }
        guard let allUntil = state?.mutedAllUntilMs else { return true }
        return allUntil <= Self.unixMs(now)
    }

    /// The app's name as the phone reports it, else its package name.
    public static func appName(of notification: PhoneNotification) -> String {
        if let name = notification.appName, !name.isEmpty { return name }
        return notification.packageName
    }

    /// A row's text: the body, or a conversation's last message.
    public static func preview(of notification: PhoneNotification) -> String {
        if !notification.body.isEmpty { return notification.body }
        for line in notification.conversation.reversed() {
            if case let .message(text) = line { return text }
        }
        return ""
    }

    /// The list as its sections: Pinned when any notification is pinned, then one section per day,
    /// newest day first. A section header names the day, so a row needs only the time of day.
    public func sections(now: Date = .now, calendar: Calendar = .current) -> [NotificationSection] {
        Self.sections(of: notifications, now: now, calendar: calendar)
    }

    /// `list` in sections (see `sections(now:calendar:)`). A notification dated after today (the
    /// phone's clock is ahead) counts as today's, so there is one Today.
    public static func sections(of list: [PhoneNotification], now: Date, calendar: Calendar) -> [NotificationSection] {
        var sections: [NotificationSection] = []
        let pinned = list.filter(\.pinned)
        if !pinned.isEmpty {
            sections.append(NotificationSection(id: .pinned, title: String(localized: "Pinned"), notifications: pinned))
        }

        let today = calendar.startOfDay(for: now)
        var days: [Date: [PhoneNotification]] = [:]
        for notification in list where !notification.pinned {
            let posted = calendar.startOfDay(for: NotificationTimeText.date(ofUnixMs: notification.timeMs))
            days[min(posted, today), default: []].append(notification)
        }

        for day in days.keys.sorted(by: >) {
            guard let notifications = days[day] else { preconditionFailure("a key of the dictionary has a value") }
            sections.append(NotificationSection(id: .day(day), title: dayTitle(day, today: today, calendar: calendar), notifications: notifications))
        }

        return sections
    }

    /// A day section's header: "Today", "Yesterday", the weekday within the last week, else the date.
    private static func dayTitle(_ day: Date, today: Date, calendar: Calendar) -> String {
        guard let days = calendar.dateComponents([.day], from: day, to: today).day else {
            preconditionFailure("a day difference always has a day component")
        }

        func style(_ date: Date.FormatStyle.DateStyle) -> Date.FormatStyle {
            Date.FormatStyle(date: date, time: .omitted, locale: calendar.locale ?? .current, calendar: calendar, timeZone: calendar.timeZone)
        }

        switch days {
        case 0:
            return String(localized: "Today")
        case 1:
            return String(localized: "Yesterday")
        case 2...6:
            return day.formatted(style(.omitted).weekday(.wide))
        default:
            return day.formatted(style(.abbreviated))
        }
    }

    public static func canSend(_ reply: String) -> Bool {
        !reply.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    /// A messaging notification: a click on it opens Messages, so its row and its detail offer
    /// Open in Messages.
    public static func opensInMessages(_ notification: PhoneNotification) -> Bool {
        notification.openTarget == .messaging
    }

    // MARK: - Loading

    /// Reads the state and the list again.
    public func reload() {
        reloadState()
        reloadList()
    }

    private func handle(_ event: EngineEvent) {
        switch event {
        case let .notifications(change):
            switch change {
            case .listChanged:
                reloadList()
            case .stateChanged:
                reloadState()
            case .settingsChanged:
                // The list hides an app whose switch went off; the context menu reads its mute.
                reloadList()
            case let .actionFinished(key, reply, outcome):
                actionFinished(key: key, reply: reply, outcome: outcome)
            case let .openRequested(key, target):
                open(key, target)
            }
        case .phoneUnlinked, .accountRemoved:
            dropUndoActions()
            reload()
        case .resync(feature: .notifications), .phoneSession, .phoneChanged:
            reload()
        default:
            break
        }
    }

    private func reloadState() {
        defer { closePostNotificationHelpIfDone() }
        do {
            state = try api.state()
            stateFailure = nil
        } catch {
            if Self.nothingToShow(error) {
                state = nil
                stateFailure = nil
            } else {
                logger.error("notifications state not read: \(error: error)")
                stateFailure = UserText.describe(error)
            }
        }
    }

    private func reloadList() {
        defer { closePostNotificationHelpIfDone() }
        let list: [PhoneNotification]
        do {
            list = try api.list()
            listFailure = nil
        } catch {
            if Self.nothingToShow(error) {
                replaceList(with: [])
                appSettings = [:]
                listFailure = nil
            } else {
                logger.error("notification list not read: \(error: error)")
                listFailure = UserText.describe(error)
            }
            return
        }

        replaceList(with: list)
        reloadAppSettings()
    }

    /// Replaces the list, keeping the selection on an item that is still listed: when the selected one
    /// went, the item now at its place is selected, as a deleted mail is followed by the next. Actions
    /// queued for a notification that left the list are forgotten: the engine drops them unsent.
    private func replaceList(with list: [PhoneNotification]) {
        let oldIndex = selection.flatMap { key in notifications.firstIndex { $0.key == key } }
        notifications = list

        let listed = Set(list.map(\.key))
        pendingActions = pendingActions.filter { listed.contains($0.key.key) }

        guard let key = selection, !list.contains(where: { $0.key == key }) else { return }
        if let oldIndex, !list.isEmpty {
            selection = list[min(oldIndex, list.count - 1)].key
        } else {
            selection = nil
        }
    }

    private func reloadAppSettings() {
        var settings: [String: AppNotificationSetting] = [:]
        var failure: Error?
        for package in Set(notifications.map(\.packageName)) {
            do {
                settings[package] = try api.appSetting(packageName: package)
            } catch {
                if Self.nothingToShow(error) { break }
                failure = failure ?? error
            }
        }

        if let failure {
            // One line for the list: the apps' reads fail together.
            logger.error("app notification settings not read: \(error: failure)")
        }

        appSettings = settings
    }

    /// Before the engine starts (or without a phone) there is nothing to show; that is not a failure.
    private static func nothingToShow(_ error: Error) -> Bool {
        switch error as? FeatureApiError {
        case .NotStarted?, .NoPhone?: true
        default: false
        }
    }

    // MARK: - Navigation

    public func select(_ key: String) {
        selection = key
    }

    /// Selects `notification` and asks its reply field for the keyboard focus.
    public func beginReply(to notification: PhoneNotification) {
        precondition(notification.reply != nil, "Reply… is offered only for a notification with a reply action")
        selection = notification.key
        replyFocusRequest = notification.key
    }

    /// The user clicked a notification the engine posted: messages open in Messages, the rest here
    /// (the app has no calls view, so a call's notification opens here too).
    private func open(_ key: String, _ target: NotificationOpenTarget) {
        switch target {
        case .messaging:
            navigator.show(.messages)
        case .notifications, .calls:
            navigator.showNotification(key)
        }
    }

    /// Open in Messages for a messaging notification of the list (what a click on it does).
    public func openInMessages(_ notification: PhoneNotification) {
        precondition(Self.opensInMessages(notification), "Open in Messages is offered only for a messaging notification")
        open(notification.key, .messaging)
    }

    // MARK: - Actions

    @discardableResult
    public func dismiss(_ notification: PhoneNotification) -> Task<Void, Never> {
        let key = notification.key
        return Task {
            do {
                let outcome = try await api.dismiss(key: key)
                report(outcome, String(localized: "Couldn't Dismiss on Your Phone"), "notification not dismissed on the phone")
            } catch {
                report(error, String(localized: "Couldn't Dismiss Notification"), "notification not dismissed")
            }
        }
    }

    /// Clears every unpinned notification; the phone clears them too, and they leave the list only when
    /// it confirms.
    @discardableResult
    public func clearAll() -> Task<Void, Never> {
        isClearing = true
        return Task {
            defer { isClearing = false }
            do {
                if let outcome = try await api.clearAll() {
                    report(outcome, String(localized: "Couldn't Clear Notifications"), "notifications not cleared on the phone")
                }
            } catch {
                report(error, String(localized: "Couldn't Clear Notifications"), "notifications not cleared")
            }
        }
    }

    public func setPinned(_ notification: PhoneNotification, _ pinned: Bool) {
        do {
            try api.setPinned(key: notification.key, pinned: pinned)
        } catch {
            report(error, pinned ? String(localized: "Couldn't Pin Notification") : String(localized: "Couldn't Unpin Notification"), "notification pin not changed")
        }
    }

    /// Presses one of the notification's buttons; its outcome arrives later.
    public func press(_ action: NotificationButton, on notification: PhoneNotification) {
        do {
            try api.pressAction(key: notification.key, actionIndex: action.index)
            pendingActions[PendingAction(key: notification.key, reply: false), default: 0] += 1
        } catch {
            report(error, String(localized: "Couldn't Complete Action"), "notification action not queued")
        }
    }

    /// Queues `text` as the notification's reply; `false` when it was not queued.
    @discardableResult
    public func sendReply(_ text: String, to notification: PhoneNotification) -> Bool {
        guard let reply = notification.reply else {
            preconditionFailure("the reply field is shown only for a notification with a reply action")
        }
        guard Self.canSend(text) else { return false }

        do {
            try api.reply(key: notification.key, actionIndex: reply.index, text: text)
            pendingActions[PendingAction(key: notification.key, reply: true), default: 0] += 1
            return true
        } catch {
            report(error, String(localized: "Couldn't Send Reply"), "notification reply not queued")
            return false
        }
    }

    private func actionFinished(key: String, reply: Bool, outcome: RequestOutcome?) {
        let action = PendingAction(key: key, reply: reply)
        // The engine logs the action's outcome where it reports it.
        guard let count = pendingActions[action] else { return }
        pendingActions[action] = count > 1 ? count - 1 : nil

        let title = reply ? String(localized: "Couldn't Send Reply") : String(localized: "Couldn't Complete Action")
        guard let outcome else {
            lastFailure = NotificationsFailure(title: title, message: String(localized: "Phone Link couldn't reach your phone within 10 minutes."))
            return
        }
        report(outcome, title, nil)
    }

    // MARK: - Per-app commands

    /// Turns the app's notifications off on the phone and its banners off here, and lets Edit › Undo
    /// bring back what the app had.
    @discardableResult
    public func turnOffApp(of notification: PhoneNotification, undoManager: UndoManager?) -> Task<Void, Never> {
        turnOffApp(package: notification.packageName, name: Self.appName(of: notification), undoManager: undoManager)
    }

    /// `known`: the flags to restore on undo when the caller has them — the redo of an undo, whose
    /// request may still be under way, so the engine's flags are not yet the restored ones.
    private func turnOffApp(
        package: String,
        name: String,
        undoManager: UndoManager?,
        known: AppNotificationFlags? = nil
    ) -> Task<Void, Never> {
        let failureTitle = String(localized: "Couldn't Turn Off Notifications from \(name)")

        let previous: AppNotificationFlags
        if let known {
            previous = known
        } else {
            do {
                previous = try api.appSetting(packageName: package).flags
            } catch {
                report(error, failureTitle, "app notification setting not read")
                return Task {}
            }
        }

        if let undoManager {
            undoManagers.add(undoManager)
            undoManager.registerUndo(withTarget: self) { model in
                model.restoreApp(package: package, name: name, previous: previous, undoManager: undoManager)
            }
            undoManager.setActionName(String(localized: "Turn Off Notifications from \(name)"))
        }

        return Task {
            do {
                let outcome = try await api.toggleAppFromList(packageName: package, enabled: false)
                report(outcome, failureTitle, "app notifications not turned off on the phone")
            } catch {
                report(error, failureTitle, "app notifications not turned off")
            }
        }
    }

    /// The undo of a turn-off: re-applies the flags the app had, and offers the turn-off as the redo.
    private func restoreApp(package: String, name: String, previous: AppNotificationFlags, undoManager: UndoManager?) {
        undoManager?.registerUndo(withTarget: self) { model in
            _ = model.turnOffApp(package: package, name: name, undoManager: undoManager, known: previous)
        }
        undoManager?.setActionName(String(localized: "Turn Off Notifications from \(name)"))

        let failureTitle = String(localized: "Couldn't Turn On Notifications from \(name)")
        Task {
            do {
                let outcome = try await api.undoAppTurnOff(packageName: package, previous: previous)
                report(outcome, failureTitle, "app notifications not restored on the phone")
            } catch {
                report(error, failureTitle, "app notifications not restored")
            }
        }
    }

    /// Turns the app's notifications back on (a pinned item of an app that is off).
    @discardableResult
    public func turnOnApp(of notification: PhoneNotification) -> Task<Void, Never> {
        let package = notification.packageName
        let failureTitle = String(localized: "Couldn't Turn On Notifications from \(Self.appName(of: notification))")
        return Task {
            do {
                let outcome = try await api.toggleAppFromList(packageName: package, enabled: true)
                report(outcome, failureTitle, "app notifications not turned on on the phone")
            } catch {
                report(error, failureTitle, "app notifications not turned on")
            }
        }
    }

    public func muteApp(of notification: PhoneNotification, for duration: NotificationMuteDuration) {
        do {
            try api.muteApp(packageName: notification.packageName, duration: duration)
        } catch {
            report(error, String(localized: "Couldn't Mute \(Self.appName(of: notification))"), "app not muted")
        }
    }

    public func unmuteApp(of notification: PhoneNotification) {
        do {
            try api.unmuteApp(packageName: notification.packageName)
        } catch {
            report(error, String(localized: "Couldn't Unmute \(Self.appName(of: notification))"), "app not unmuted")
        }
    }

    // MARK: - Setup

    /// Asks the phone for notification access; the phone shows the request.
    @discardableResult
    public func requestAccess() -> Task<Void, Never> {
        isRequestingAccess = true
        return Task {
            defer { isRequestingAccess = false }
            do {
                let outcome = try await api.requestAccess()
                // A low-memory phone answers "not supported"; the list column then says so.
                if outcome != .deviceNotSupported {
                    report(outcome, String(localized: "Couldn't Get Notification Access"), "notification access not granted")
                }

                // The setup flow's request also turns this Mac's switch on once the phone grants
                // access, so the list follows without a second click.
                if outcome == .success || outcome == .accepted {
                    turnOnNotifications()
                }
            } catch {
                report(error, String(localized: "Couldn't Get Notification Access"), "notification access not requested")
            }

            reloadState()
        }
    }

    /// Opens the post-notification sheet: the link to the phone's setting, and its code to scan.
    public func showPostNotificationHelp() {
        precondition(content == .needsPostNotificationPermission, "the sheet belongs to the post-notification stage")
        postNotificationHelp = PostNotificationHelp()
    }

    /// The sheet closes once its stage no longer applies (the phone reported the permission).
    private func closePostNotificationHelpIfDone() {
        if postNotificationHelp != nil, content != .needsPostNotificationPermission {
            postNotificationHelp = nil
        }
    }

    /// Turns "Show notifications from my phone" back on.
    public func turnOnNotifications() {
        do {
            try api.setPcPermission(on: true)
        } catch {
            report(error, String(localized: "Couldn't Turn On Notifications"), "notifications not turned on")
        }
    }

    private func dropUndoActions() {
        for manager in undoManagers.allObjects {
            manager.removeAllActions(withTarget: self)
        }
    }

    // MARK: - Failures

    /// `subject` names the failure in the log; `nil` for an outcome the engine reported as an event,
    /// which the engine logs.
    private func report(_ outcome: RequestOutcome, _ title: String, _ subject: StaticString?) {
        guard let message = UserText.failure(outcome) else { return }
        if let subject {
            logger.error("\(subject.description, privacy: .public): \(outcome, privacy: .public)")
        }
        lastFailure = NotificationsFailure(title: title, message: message)
    }

    private func report(_ error: Error, _ title: String, _ subject: StaticString) {
        if case .NotStarted? = error as? FeatureApiError {
            logger.debug("notification action dropped; the engine has not started")
            return
        }

        logger.error("\(subject.description, privacy: .public): \(error: error)")
        lastFailure = NotificationsFailure(title: title, message: UserText.describe(error))
    }

    private static func unixMs(_ date: Date) -> Int64 {
        Int64((date.timeIntervalSince1970 * 1000).rounded())
    }
}

/// An action or reply of one notification, as `ActionFinished` names it.
private struct PendingAction: Hashable {
    let key: String
    let reply: Bool
}
