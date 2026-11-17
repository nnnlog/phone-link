import Foundation
import Testing
import PhoneLinkEngine
@testable import PhoneLinkKit

@MainActor
@Suite struct NotificationsModelTests {
    private let api = NotificationsApiFake()
    private let hub = EngineEventHub()
    private let navigator = AppNavigator()
    private let model: NotificationsModel

    init() {
        model = NotificationsModel(api: api, hub: hub, navigator: navigator)
    }

    /// Starts the engine with `list` and a phone that has given access.
    private func start(_ list: [PhoneNotification], state: NotificationsState = makeState()) {
        api.update {
            $0.state = .success(state)
            $0.list = .success(list)
        }
        hub.postStarted()
    }

    // MARK: - Loading

    @Test func startLoadsTheStateAndTheList() {
        start([kakao, gmail])
        #expect(model.content == .list)
        #expect(model.notifications.map(\.key) == ["kakao", "gmail"])
        #expect(model.badgeCount == 2)
    }

    @Test func beforeTheEngineStartsThereIsNothingToShowAndNoFailure() {
        hub.postStarted()
        #expect(model.state == nil)
        #expect(model.content == .loading)
        #expect(model.notifications.isEmpty)
        #expect(model.badgeCount == 0)

        api.update { $0.error = .NotStarted }
        model.setPinned(kakao, true)
        #expect(model.lastFailure == nil)
    }

    @Test func eachChangeRereadsWhatItNames() {
        start([kakao])
        api.update {
            $0.list = .success([gmail, kakao])
            $0.state = .success(makeState(pc: false))
        }
        hub.post(.notifications(change: .listChanged))
        #expect(model.notifications.map(\.key) == ["gmail", "kakao"])
        #expect(model.state?.pcPermission == true)

        hub.post(.notifications(change: .stateChanged))
        #expect(model.state?.pcPermission == false)
    }

    @Test func aSettingsChangeRereadsTheListAndTheAppsSettings() {
        start([kakao])
        #expect(model.isAppOn(kakao))
        api.update {
            $0.settings[kakao.packageName] = AppNotificationSetting(packageName: kakao.packageName, flags: offFlags, mutedUntilMs: nil)
            $0.list = .success([kakao, gmail])
        }
        hub.post(.notifications(change: .settingsChanged))
        #expect(!model.isAppOn(kakao))
        #expect(model.notifications.count == 2)
    }

    @Test func aResyncOrASessionChangeRereadsEverything() {
        start([kakao])
        api.update {
            $0.list = .success([])
            $0.state = .success(makeState(syncing: true))
        }
        hub.post(.resync(feature: .notifications))
        #expect(model.content == .syncing)

        api.update { $0.list = .success([gmail]) }
        hub.post(.resync(feature: .messaging))
        #expect(model.notifications.isEmpty)

        hub.post(.phoneSession(connected: true, failure: nil))
        #expect(model.notifications.map(\.key) == ["gmail"])
    }

    @Test(arguments: [
        (makeState(capable: false), NotificationsContent.unavailable),
        (makeState(capable: false, postNotification: true), .unavailable),
        (makeState(postNotification: true), .needsPostNotificationPermission),
        (makeState(pc: false, device: false, postNotification: true), .needsPostNotificationPermission),
        (makeState(device: false, lowRam: true, postNotification: true), .needsPostNotificationPermission),
        (makeState(pc: false), .turnedOff),
        (makeState(pc: false, device: false), .needsAccess),
        (makeState(pc: false, device: false, lowRam: true), .turnedOff),
        (makeState(device: false, lowRam: true), .lowMemoryPhone),
        (makeState(device: false), .needsAccess),
        (makeState(), .list),
    ])
    func setupStatesComeInPriorityOrder(state: NotificationsState, content: NotificationsContent) {
        start([kakao], state: state)
        #expect(model.content == content)
        #expect(model.badgeCount == (content == .list ? 1 : 0))
    }

    @Test func anEmptyListIsEmptyOrSyncing() {
        start([], state: makeState(syncing: true))
        #expect(model.content == .syncing)

        api.update { $0.state = .success(makeState()) }
        hub.post(.notifications(change: .stateChanged))
        #expect(model.content == .empty)
        #expect(model.selectedNotification == nil)
    }

    @Test func theSelectionMovesToTheItemAtItsPlaceWhenItGoes() {
        let third = notification("third")
        start([kakao, gmail, third])
        model.select("gmail")
        #expect(model.selectedNotification?.key == "gmail")

        api.update { $0.list = .success([kakao, third]) }
        hub.post(.notifications(change: .listChanged))
        #expect(model.selection == "third")

        api.update { $0.list = .success([kakao]) }
        hub.post(.notifications(change: .listChanged))
        #expect(model.selection == "kakao")

        api.update { $0.list = .success([]) }
        hub.post(.notifications(change: .listChanged))
        #expect(model.selection == nil)
    }

    // MARK: - Actions

    @Test func aDismissThePhoneDidNotConfirmIsReported() async {
        start([kakao])
        api.update { $0.outcome = .timedOut }
        await model.dismiss(kakao).value
        #expect(api.calls == [.dismiss("kakao")])
        #expect(model.lastFailure == NotificationsFailure(title: "Couldn't Dismiss on Your Phone", message: UserText.failure(.timedOut)!))
    }

    @Test func clearAllReportsAFailureButNotHavingNothingToClear() async {
        start([kakao])
        #expect(model.canClearAll)
        api.update { $0.clearAllOutcome = nil }
        await model.clearAll().value
        #expect(model.lastFailure == nil)
        #expect(!model.isClearing)

        api.update { $0.clearAllOutcome = .denied }
        await model.clearAll().value
        #expect(model.lastFailure?.title == "Couldn't Clear Notifications")
        #expect(model.lastFailure?.message == UserText.failure(.denied))
    }

    @Test func pinnedNotificationsAreNotCleared() {
        start([notification("pinned", pinned: true)])
        #expect(!model.canClearAll)
    }

    @Test func aThrownErrorIsReportedWithItsText() {
        start([kakao])
        api.update { $0.error = .UnknownItem(what: "notification") }
        model.setPinned(kakao, true)
        #expect(model.lastFailure == NotificationsFailure(
            title: "Couldn't Pin Notification",
            message: UserText.describe(FeatureApiError.UnknownItem(what: "notification"))
        ))
    }

    @Test func onlyActionsQueuedHereReportTheirOutcome() {
        start([kakao])
        model.press(markAsRead, on: kakao)
        #expect(api.calls == [.pressAction("kakao", 3)])

        hub.post(.notifications(change: .actionFinished(key: "kakao", reply: false, outcome: .failure)))
        #expect(model.lastFailure == NotificationsFailure(title: "Couldn't Complete Action", message: UserText.failure(.failure)!))

        // Its outcome arrived; a later one for the key came from a banner or an earlier run.
        model.lastFailure = nil
        hub.post(.notifications(change: .actionFinished(key: "kakao", reply: false, outcome: .failure)))
        #expect(model.lastFailure == nil)
    }

    @Test func anActionWhoseNotificationLeftTheListIsForgotten() {
        start([kakao])
        model.press(markAsRead, on: kakao)

        // The phone removed the notification: the engine drops the queued action unsent.
        api.update { $0.list = .success([]) }
        hub.post(.notifications(change: .listChanged))
        api.update { $0.list = .success([kakao]) }
        hub.post(.notifications(change: .listChanged))

        // An outcome for the key now belongs to another request (a banner's).
        hub.post(.notifications(change: .actionFinished(key: "kakao", reply: false, outcome: .failure)))
        #expect(model.lastFailure == nil)
    }

    @Test func aStateThatCannotBeReadShowsWhyUntilTryAgainReadsIt() {
        api.update {
            $0.state = .failure(.Store(message: "disk full"))
            $0.list = .success([kakao])
        }
        hub.postStarted()
        #expect(model.content == .failed(UserText.describe(FeatureApiError.Store(message: "disk full"))))
        #expect(model.badgeCount == 0)

        api.update { $0.state = .success(makeState()) }
        model.reload()
        #expect(model.content == .list)
    }

    @Test func aReplyDroppedUnsentIsReported() {
        start([kakao])
        #expect(model.sendReply("See you", to: kakao))
        #expect(api.calls == [.reply("kakao", 2, "See you")])
        hub.post(.notifications(change: .actionFinished(key: "kakao", reply: true, outcome: .success)))
        #expect(model.lastFailure == nil)

        #expect(model.sendReply("Again", to: kakao))
        hub.post(.notifications(change: .actionFinished(key: "kakao", reply: true, outcome: nil)))
        #expect(model.lastFailure?.title == "Couldn't Send Reply")
        #expect(model.lastFailure?.message == "Phone Link couldn't reach your phone within 10 minutes.")
    }

    @Test func aBlankReplyIsNotSent() {
        start([kakao])
        #expect(!model.sendReply("  \n", to: kakao))
        #expect(api.calls.isEmpty)
    }

    @Test func replySelectsTheNotificationAndAsksForTheField() {
        start([kakao, gmail])
        model.beginReply(to: kakao)
        #expect(model.selection == "kakao")
        #expect(model.replyFocusRequest == "kakao")
    }

    // MARK: - Per-app commands

    @Test func turningAnAppOffCanBeUndoneWithTheFlagsItHad() async {
        let previous = AppNotificationFlags(feed: true, banner: false, otherBits: 0b1100)
        start([kakao])
        api.update { $0.settings[kakao.packageName] = AppNotificationSetting(packageName: kakao.packageName, flags: previous, mutedUntilMs: nil) }
        let undoManager = UndoManager()
        undoManager.groupsByEvent = false

        undoManager.beginUndoGrouping()
        let turnOff = model.turnOffApp(of: kakao, undoManager: undoManager)
        undoManager.endUndoGrouping()
        await turnOff.value
        #expect(api.calls == [.toggleApp(kakao.packageName, false)])
        #expect(undoManager.undoActionName == "Turn Off Notifications from KakaoTalk")

        undoManager.undo()
        await waitUntil { api.calls.count == 2 }
        #expect(api.calls.last == .undoTurnOff(kakao.packageName, previous))
        #expect(undoManager.redoActionName == "Turn Off Notifications from KakaoTalk")

        undoManager.redo()
        await waitUntil { api.calls.count == 3 }
        #expect(api.calls.last == .toggleApp(kakao.packageName, false))
        #expect(undoManager.canUndo)
    }

    @Test func theRedoOfAnUndoneTurnOffKeepsTheFlagsToRestore() async {
        let previous = AppNotificationFlags(feed: true, banner: false, otherBits: 0b1100)
        start([kakao])
        api.update { $0.settings[kakao.packageName] = AppNotificationSetting(packageName: kakao.packageName, flags: previous, mutedUntilMs: nil) }
        let undoManager = UndoManager()
        undoManager.groupsByEvent = false

        undoManager.beginUndoGrouping()
        let turnOff = model.turnOffApp(of: kakao, undoManager: undoManager)
        undoManager.endUndoGrouping()
        await turnOff.value

        undoManager.undo()
        await waitUntil { api.calls.count == 2 }

        // The undo's request has not finished: the engine still reports the app off.
        api.update { $0.settings[kakao.packageName] = AppNotificationSetting(packageName: kakao.packageName, flags: offFlags, mutedUntilMs: nil) }
        undoManager.redo()
        await waitUntil { api.calls.count == 3 }

        undoManager.undo()
        await waitUntil { api.calls.count == 4 }
        #expect(api.calls.last == .undoTurnOff(kakao.packageName, previous))
    }

    @Test func turnOffUndoIsDroppedWhenAnotherPhoneMayBeSelected() async {
        start([kakao])
        let undoManager = UndoManager()
        undoManager.groupsByEvent = false

        undoManager.beginUndoGrouping()
        let turnOff = model.turnOffApp(of: kakao, undoManager: undoManager)
        undoManager.endUndoGrouping()
        await turnOff.value
        #expect(undoManager.canUndo)

        hub.post(.phoneUnlinked)
        #expect(!undoManager.canUndo)
    }

    @Test func aFailedTurnOffIsReportedAndStillUndoable() async {
        start([kakao])
        api.update { $0.outcome = .timedOut }
        let undoManager = UndoManager()
        undoManager.groupsByEvent = false

        undoManager.beginUndoGrouping()
        let turnOff = model.turnOffApp(of: kakao, undoManager: undoManager)
        undoManager.endUndoGrouping()
        await turnOff.value

        // The banner flag changed here whatever the phone answered, so undo still applies.
        #expect(undoManager.canUndo)
        #expect(model.lastFailure?.title == "Couldn't Turn Off Notifications from KakaoTalk")
    }

    @Test func muteCommandsFollowTheAppsSettings() {
        let future = Int64(Date.now.addingTimeInterval(3600).timeIntervalSince1970 * 1000)
        start([kakao])
        #expect(model.appCommandsAvailable)
        #expect(model.canMuteApp(kakao))
        #expect(!model.isAppMuted(kakao))

        api.update { $0.settings[kakao.packageName] = AppNotificationSetting(packageName: kakao.packageName, flags: onFlags, mutedUntilMs: future) }
        hub.post(.notifications(change: .settingsChanged))
        #expect(model.isAppMuted(kakao))
        #expect(!model.isAppMuted(kakao, now: Date.now.addingTimeInterval(7200)))

        // Every banner muted: the app's own mute is not offered.
        api.update { $0.state = .success(makeState(mutedAllUntilMs: future)) }
        hub.post(.notifications(change: .stateChanged))
        #expect(!model.canMuteApp(kakao))

        api.update { $0.state = .success(makeState(appList: false)) }
        hub.post(.notifications(change: .stateChanged))
        #expect(!model.appCommandsAvailable)

        model.muteApp(of: kakao, for: .eightHours)
        model.unmuteApp(of: kakao)
        #expect(api.calls == [.mute(kakao.packageName, .eightHours), .unmute(kakao.packageName)])
    }

    @Test func anAppThatIsOffCanBeTurnedBackOn() async {
        start([kakao])
        api.update { $0.settings[kakao.packageName] = AppNotificationSetting(packageName: kakao.packageName, flags: offFlags, mutedUntilMs: nil) }
        hub.post(.notifications(change: .settingsChanged))
        #expect(!model.isAppOn(kakao))
        #expect(!model.canMuteApp(kakao))

        await model.turnOnApp(of: kakao).value
        #expect(api.calls == [.toggleApp(kakao.packageName, true)])
    }

    // MARK: - Setup and navigation

    @Test func anAccessRequestReportsADenialButNotALowMemoryPhone() async {
        start([], state: makeState(device: false))
        api.update { $0.outcome = .denied }
        await model.requestAccess().value
        #expect(model.lastFailure?.title == "Couldn't Get Notification Access")
        #expect(!model.isRequestingAccess)

        model.lastFailure = nil
        api.update {
            $0.outcome = .deviceNotSupported
            $0.state = .success(makeState(device: false, lowRam: true))
        }
        await model.requestAccess().value
        #expect(model.lastFailure == nil)
        #expect(model.content == .lowMemoryPhone)
    }

    @Test func aGrantedAccessRequestAlsoTurnsTheMacsSwitchOn() async {
        start([], state: makeState(pc: false, device: false))
        api.update { $0.outcome = .accepted }
        await model.requestAccess().value
        #expect(api.calls == [.requestAccess, .setPcPermission(true)])

        api.update {
            $0.calls = []
            $0.outcome = .denied
        }
        await model.requestAccess().value
        #expect(api.calls == [.requestAccess], "a denial leaves the switch alone")
    }

    @Test func turningNotificationsOnSetsTheMacsSwitch() {
        start([], state: makeState(pc: false))
        model.turnOnNotifications()
        #expect(api.calls == [.setPcPermission(true)])
    }

    @Test(arguments: [
        (NotificationOpenTarget.messaging, AppSection.messages, nil as String?),
        (.notifications, .notifications, "kakao"),
        (.calls, .calls, nil),
    ])
    func aClickedBannerOpensItsView(target: NotificationOpenTarget, section: AppSection, request: String?) {
        start([kakao])
        navigator.section = .photos
        hub.post(.notifications(change: .openRequested(key: "kakao", target: target)))
        #expect(navigator.section == section)
        #expect(navigator.notificationRequest == request)
    }

    /// A messaging notification of the list opens Messages, as a click on it does.
    @Test func aMessagingNotificationOfTheListOpensInMessages() {
        let chat = notification("chat", openTarget: .messaging)
        let call = notification("call", openTarget: .calls)
        start([chat, call, gmail])
        navigator.section = .notifications

        #expect(NotificationsModel.opensInMessages(chat))
        #expect(!NotificationsModel.opensInMessages(call) && !NotificationsModel.opensInMessages(gmail))

        model.openInMessages(chat)
        #expect(navigator.section == .messages)
        #expect(navigator.notificationRequest == nil)
        #expect(api.calls.isEmpty, "nothing is sent to the phone")
    }

    /// The post-notification sheet shows the link that opens the setting on the phone, with a fresh
    /// trace id each time, and closes by itself once the stage ends.
    @Test func thePostNotificationSheetShowsTheLinkAndClosesWhenTheStageEnds() throws {
        start([kakao], state: makeState(postNotification: true))
        #expect(model.content == .needsPostNotificationPermission)
        #expect(model.badgeCount == 0)
        #expect(model.postNotificationHelp == nil)

        model.showPostNotificationHelp()
        let help = try #require(model.postNotificationHelp)
        let components = try #require(URLComponents(url: help.link, resolvingAgainstBaseURL: false))
        #expect(components.scheme == "https" && components.host == "aka.ms" && components.path == "/notification")

        let query = components.queryItems ?? []
        #expect(query.map(\.name) == ["src", "traceId"])
        #expect(query.first?.value == "1")
        let traceId = try #require(query.last?.value)
        #expect(UUID(uuidString: traceId) != nil && traceId == traceId.lowercased())
        #expect(PostNotificationHelp().link != help.link, "a fresh trace id per sheet")
        #expect(PostNotificationHelp.shortLink == "aka.ms/notification")

        let code = help.qrCode()
        #expect(code.width == code.height && code.width > 0)
        #expect(api.calls.isEmpty, "the sheet sends nothing")

        // Another change leaves it open; the phone reporting the permission closes it.
        hub.post(.notifications(change: .listChanged))
        #expect(model.postNotificationHelp == help)

        api.update { $0.state = .success(makeState()) }
        hub.post(.notifications(change: .stateChanged))
        #expect(model.content == .list)
        #expect(model.postNotificationHelp == nil)
    }

    // MARK: - Texts

    @Test func aRowShowsTheBodyOrTheLastMessage() {
        #expect(NotificationsModel.preview(of: gmail) == "Here is this week's summary.")
        let conversation = notification("chat", body: "", conversation: [
            .sender(name: "Jiho"), .message(text: "Saturday at 7?"), .message(text: "Exit 3"),
        ])
        #expect(NotificationsModel.preview(of: conversation) == "Exit 3")
        #expect(NotificationsModel.appName(of: notification("nameless", appName: "")) == "com.example.nameless")
    }

    @Test func listTimesReadLikeMails() throws {
        var calendar = Calendar(identifier: .gregorian)
        calendar.locale = Locale(identifier: "en_US")
        calendar.timeZone = try #require(TimeZone(identifier: "UTC"))
        let now = try #require(calendar.date(from: DateComponents(year: 2026, month: 10, day: 21, hour: 14, minute: 5)))

        func ago(days: Int, hour: Int = 9) throws -> Date {
            let day = try #require(calendar.date(byAdding: .day, value: -days, to: now))
            return try #require(calendar.date(bySettingHour: hour, minute: 30, second: 0, of: day))
        }

        #expect(NotificationTimeText.short(try ago(days: 0), now: now, calendar: calendar).hasPrefix("9:30"))
        #expect(NotificationTimeText.short(try ago(days: 1), now: now, calendar: calendar) == "Yesterday")
        #expect(NotificationTimeText.short(try ago(days: 2), now: now, calendar: calendar) == "Monday")
        #expect(NotificationTimeText.short(try ago(days: 8), now: now, calendar: calendar) == "10/13/2026")

        #expect(NotificationTimeText.full(try ago(days: 0), now: now, calendar: calendar).hasPrefix("9:30"))
        #expect(NotificationTimeText.full(try ago(days: 1), now: now, calendar: calendar).hasPrefix("Oct 20, 2026"))
    }

    @Test func theListIsInPinnedThenDaySectionsNewestFirst() throws {
        var calendar = Calendar(identifier: .gregorian)
        calendar.locale = Locale(identifier: "en_US")
        calendar.timeZone = try #require(TimeZone(identifier: "UTC"))
        let now = try #require(calendar.date(from: DateComponents(year: 2026, month: 10, day: 21, hour: 14, minute: 5)))

        func posted(_ key: String, hoursAgo: Int, pinned: Bool = false) -> PhoneNotification {
            var item = notification(key, pinned: pinned)
            item.timeMs = Int64(now.addingTimeInterval(Double(-hoursAgo) * 3600).timeIntervalSince1970 * 1000)
            return item
        }

        // Newest first, as the engine lists them; the first is dated tomorrow, by a phone clock running ahead.
        let list = [
            posted("ahead", hoursAgo: -11),
            posted("noon", hoursAgo: 2),
            posted("pinnedOld", hoursAgo: 200, pinned: true),
            posted("morning", hoursAgo: 5),
            posted("lastNight", hoursAgo: 17),
            posted("monday", hoursAgo: 50),
            posted("lastWeek", hoursAgo: 8 * 24),
        ]

        let sections = NotificationsModel.sections(of: list, now: now, calendar: calendar)

        #expect(sections.map(\.title) == ["Pinned", "Today", "Yesterday", "Monday", "Oct 13, 2026"])
        #expect(sections.map { $0.notifications.map(\.key) } == [
            ["pinnedOld"], ["ahead", "noon", "morning"], ["lastNight"], ["monday"], ["lastWeek"],
        ])
        #expect(sections.first?.id == .pinned)
        #expect(sections[1].id == .day(calendar.startOfDay(for: now)))
    }

    @Test func withNothingPinnedThereIsNoPinnedSection() {
        start([kakao, gmail])
        #expect(!model.sections().contains { $0.id == .pinned })
        #expect(model.sections().flatMap(\.notifications).map(\.key) == ["kakao", "gmail"])
    }
}

// MARK: - Fixtures

private let onFlags = AppNotificationFlags(feed: true, banner: true, otherBits: 0)
private let offFlags = AppNotificationFlags(feed: false, banner: false, otherBits: 0)
private let markAsRead = NotificationButton(name: "Mark as Read", inlineReply: false, index: 3, position: 1)

private let kakao = notification(
    "kakao",
    appName: "KakaoTalk",
    body: "Jiho: Exit 3",
    reply: NotificationButton(name: "Reply", inlineReply: true, index: 2, position: 0),
    actions: [markAsRead]
)
private let gmail = notification("gmail", appName: "Gmail", body: "Here is this week's summary.")

private func notification(
    _ key: String,
    appName: String? = nil,
    body: String = "Text",
    conversation: [NotificationLine] = [],
    pinned: Bool = false,
    openTarget: NotificationOpenTarget = .notifications,
    reply: NotificationButton? = nil,
    actions: [NotificationButton] = []
) -> PhoneNotification {
    PhoneNotification(
        key: key,
        packageName: "com.example.\(key)",
        appName: appName,
        title: "Title",
        subtitle: nil,
        body: body,
        conversation: conversation,
        timeMs: 1_792_000_000_000,
        pinned: pinned,
        openTarget: openTarget,
        launchSupported: true,
        isReadOnlyGroupMessage: false,
        reply: reply,
        actions: actions
    )
}

private func makeState(
    capable: Bool = true,
    appList: Bool = true,
    pc: Bool = true,
    device: Bool = true,
    lowRam: Bool = false,
    syncing: Bool = false,
    mutedAllUntilMs: Int64? = nil,
    postNotification: Bool = false
) -> NotificationsState {
    NotificationsState(
        capable: capable,
        appListSupported: appList,
        pcPermission: pc,
        devicePermission: device,
        sensitivePermission: false,
        sensitivePromptOffered: false,
        syncing: syncing,
        lastUpdatedMs: nil,
        mutedAllUntilMs: mutedAllUntilMs,
        throttlingEnabled: true,
        lowRamDevice: lowRam,
        setupCompleted: true,
        postNotificationNeeded: postNotification
    )
}

/// Waits for work the model started in a task it does not hand out (an undo's).
@MainActor
private func waitUntil(_ condition: () -> Bool) async {
    for _ in 0..<1000 {
        if condition() { return }
        try? await Task.sleep(for: .milliseconds(2))
    }
}

/// The notifications API with canned answers; it records every call that changes something.
private final class NotificationsApiFake: NotificationsApiProtocol, @unchecked Sendable {
    enum Call: Equatable {
        case dismiss(String)
        case clearAll
        case setPinned(String, Bool)
        case pressAction(String, Int32)
        case reply(String, Int32, String)
        case toggleApp(String, Bool)
        case undoTurnOff(String, AppNotificationFlags)
        case mute(String, NotificationMuteDuration)
        case unmute(String)
        case requestAccess
        case setPcPermission(Bool)
    }

    struct Store {
        var state: Result<NotificationsState, FeatureApiError> = .failure(.NotStarted)
        var list: Result<[PhoneNotification], FeatureApiError> = .failure(.NotStarted)
        var settings: [String: AppNotificationSetting] = [:]
        /// What every request to the phone answers.
        var outcome: RequestOutcome = .success
        var clearAllOutcome: RequestOutcome? = .success
        /// Thrown by every call that changes something.
        var error: FeatureApiError?
        var calls: [Call] = []
    }

    private let lock = NSLock()
    private var store = Store()

    func update(_ change: (inout Store) -> Void) {
        lock.withLock { change(&store) }
    }

    var calls: [Call] {
        lock.withLock { store.calls }
    }

    /// Records `call` and gives the canned error, if any.
    private func record(_ call: Call) throws {
        let error: FeatureApiError? = lock.withLock {
            store.calls.append(call)
            return store.error
        }
        if let error { throw error }
    }

    private var outcome: RequestOutcome {
        lock.withLock { store.outcome }
    }

    func state() throws -> NotificationsState {
        try lock.withLock { store.state }.get()
    }

    func list() throws -> [PhoneNotification] {
        try lock.withLock { store.list }.get()
    }

    func appSetting(packageName: String) throws -> AppNotificationSetting {
        lock.withLock { store.settings[packageName] } ?? AppNotificationSetting(packageName: packageName, flags: onFlags, mutedUntilMs: nil)
    }

    func dismiss(key: String) async throws -> RequestOutcome {
        try record(.dismiss(key))
        return outcome
    }

    func clearAll() async throws -> RequestOutcome? {
        try record(.clearAll)
        return lock.withLock { store.clearAllOutcome }
    }

    func setPinned(key: String, pinned: Bool) throws {
        try record(.setPinned(key, pinned))
    }

    func pressAction(key: String, actionIndex: Int32) throws {
        try record(.pressAction(key, actionIndex))
    }

    func reply(key: String, actionIndex: Int32, text: String) throws {
        try record(.reply(key, actionIndex, text))
    }

    func toggleAppFromList(packageName: String, enabled: Bool) async throws -> RequestOutcome {
        try record(.toggleApp(packageName, enabled))
        return outcome
    }

    func undoAppTurnOff(packageName: String, previous: AppNotificationFlags) async throws -> RequestOutcome {
        try record(.undoTurnOff(packageName, previous))
        return outcome
    }

    func muteApp(packageName: String, duration: NotificationMuteDuration) throws {
        try record(.mute(packageName, duration))
    }

    func unmuteApp(packageName: String) throws {
        try record(.unmute(packageName))
    }

    func requestAccess() async throws -> RequestOutcome {
        try record(.requestAccess)
        return outcome
    }

    func setPcPermission(on: Bool) throws {
        try record(.setPcPermission(on))
    }

    // Not used by the notifications model (the Settings part's).

    func applyPreferences(change: NotificationPreferencesChange) async throws -> NotificationPreferencesResult {
        preconditionFailure("not used by NotificationsModel")
    }

    func changeAppSetting(packageName: String, flags: AppNotificationFlags) async throws -> RequestOutcome {
        preconditionFailure("not used by NotificationsModel")
    }

    func openSettingsView() async throws -> RequestOutcome? {
        preconditionFailure("not used by NotificationsModel")
    }

    func requestSensitiveAccess() async throws -> RequestOutcome {
        preconditionFailure("not used by NotificationsModel")
    }

    func setAppLocalSetting(packageName: String, flags: AppNotificationFlags) throws {
        preconditionFailure("not used by NotificationsModel")
    }

    func setMuteAll(on: Bool) throws {
        preconditionFailure("not used by NotificationsModel")
    }

    func setThrottlingEnabled(on: Bool) throws {
        preconditionFailure("not used by NotificationsModel")
    }
}
