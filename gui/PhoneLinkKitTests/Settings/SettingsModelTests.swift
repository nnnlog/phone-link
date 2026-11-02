import Foundation
import PhoneLinkEngine
import Testing
@testable import PhoneLinkKit

@MainActor
struct SettingsModelTests {
    // MARK: - Loading

    @Test func readsEverySettingOnceTheEngineStarts() {
        let world = SettingsWorld()
        world.engine.started = false
        let model = world.makeModel()
        model.reload()
        #expect(!model.isAvailable)

        world.engine.started = true
        world.hub.postStarted()
        #expect(model.isAvailable)
        #expect(model.notificationsState == world.notifications.current)
        #expect(model.showsPhoneNotifications)
        #expect(model.alertsLowBattery)
        #expect(model.pausesNoisyApps)
        #expect(!model.pausesAllNotifications)
        #expect(model.pausedUntilText == nil)
        #expect(model.messagingSettings == SettingsFixtures.messaging)
        #expect(model.showsMessages && model.notifiesMessages && model.downloadsPictures)
        #expect(model.contactsState == world.contacts.current)
        #expect(model.usesPhoneContacts)
        #expect(model.photosState == world.photos.current)
        #expect(model.showsPhotos && model.notifiesPhotos && model.allowsPhotoDeletion)
        #expect(model.clipboardState == (try? world.clipboard.phoneState()))
        #expect(model.sharesClipboard)
        #expect(model.failure == nil)
    }

    @Test func beforeTheEngineStartsNothingIsShownAndNothingFails() async {
        let world = SettingsWorld()
        world.engine.started = false
        let model = world.makeModel()
        model.reload()
        #expect(!model.isAvailable)
        #expect(model.notificationsState == nil && model.lowBatteryEnabled == nil && model.messagingSettings == nil)
        #expect(model.contactsState == nil && model.photosState == nil && model.clipboardState == nil)
        #expect(model.notificationApps.isEmpty)
        #expect(!model.showsPhoneNotifications && !model.sharesClipboard && !model.showsPhotos)

        model.showsPhotos = true
        model.sharesClipboard = true
        await model.requestNotificationAccess()
        await model.notificationsPaneAppeared()
        #expect(model.failure == nil)
        #expect(world.engine.writes.isEmpty)
    }

    @Test func anEngineThatStopsLeavesNothingShown() {
        let world = SettingsWorld()
        world.notifications.items = [SettingsFixtures.item("1", package: "com.slack", appName: "Slack")]
        let model = world.makeModel()
        model.reload()
        #expect(model.isAvailable && !model.notificationApps.isEmpty)

        world.engine.started = false
        model.reload()
        #expect(!model.isAvailable)
        #expect(model.notificationsState == nil && model.notificationApps.isEmpty && model.clipboardState == nil)
        #expect(model.failure == nil)
    }

    @Test func featureEventsRereadWhatChanged() {
        let world = SettingsWorld()
        let model = world.makeModel()
        model.setNotificationsPaneVisible(true)
        model.reload()

        world.notifications.current.throttlingEnabled = false
        world.hub.post(.notifications(change: .stateChanged))
        #expect(!model.pausesNoisyApps)

        world.notifications.items = [SettingsFixtures.item("1", package: "com.slack", appName: "Slack")]
        world.hub.post(.notifications(change: .listChanged))
        #expect(model.notificationApps.map(\.name) == ["Slack"])

        world.notifications.appSettings["com.slack"] = SettingsFixtures.setting("com.slack", feed: true, banner: false)
        world.hub.post(.notifications(change: .settingsChanged))
        #expect(model.notificationApps.first?.choice == .listOnly)

        world.messaging.current.settings.pcConsent = false
        world.hub.post(.messaging(change: .stateChanged))
        #expect(!model.showsMessages)

        world.contacts.current.pcConsent = false
        world.hub.post(.contacts(change: .stateChanged))
        #expect(!model.usesPhoneContacts)

        world.photos.current.consent = false
        world.hub.post(.photos(change: .stateChanged))
        #expect(!model.showsPhotos)

        world.clipboard.global = false
        world.hub.post(.clipboard(change: .globalSwitchChanged(enabled: false)))
        #expect(!model.sharesClipboard)

        world.clipboard.global = true
        world.clipboard.capable = false
        world.hub.post(.clipboard(change: .phoneChanged))
        #expect(model.clipboardState?.capable == false)

        world.deviceStatus.lowBattery = false
        world.hub.post(.resync(feature: .deviceStatus))
        #expect(!model.alertsLowBattery)

        world.notifications.current.pcPermission = false
        world.notifications.items.append(SettingsFixtures.item("2", package: "com.kakao.talk", appName: "KakaoTalk"))
        world.hub.post(.resync(feature: .notifications))
        #expect(!model.showsPhoneNotifications)
        #expect(model.notificationApps.map(\.name) == ["KakaoTalk", "Slack"])
        #expect(world.engine.writes.isEmpty)
    }

    @Test func withTheNotificationsPaneClosedAListChangeOnlyRemembersItsApps() {
        let world = SettingsWorld()
        let model = world.makeModel()
        model.reload()
        let reads = world.notifications.appSettingReads

        world.notifications.items = [SettingsFixtures.item("1", package: "com.slack", appName: "Slack")]
        world.hub.post(.notifications(change: .listChanged))
        #expect(model.notificationApps.isEmpty)
        #expect(world.notifications.appSettingReads == reads, "no app setting is read while the pane is closed")

        // The app turned off from the list leaves it, yet the pane lists it when it opens.
        world.notifications.items = []
        model.setNotificationsPaneVisible(true)
        model.reload()
        #expect(model.notificationApps.map(\.name) == ["Slack"])
    }

    @Test func eachPhoneRemembersItsOwnApps() {
        let world = SettingsWorld()
        world.notifications.items = [SettingsFixtures.item("1", package: "com.slack", appName: "Slack")]
        let model = world.makeModel()
        model.reload()
        #expect(model.notificationApps.map(\.name) == ["Slack"])

        world.phone.id = "another-phone"
        world.notifications.items = [SettingsFixtures.item("2", package: "com.kakao.talk", appName: "KakaoTalk")]
        world.hub.postStarted()
        #expect(model.notificationApps.map(\.name) == ["KakaoTalk"])

        world.phone.id = SettingsFakePhone.firstPhone
        world.notifications.items = []
        world.hub.postStarted()
        #expect(model.notificationApps.map(\.name) == ["Slack"])
    }

    @Test func afterTheAccountIsRemovedNothingIsReadOrChangedUntilTheNextStart() async {
        let world = SettingsWorld()
        let model = world.makeModel()
        model.reload()
        #expect(model.isAvailable)

        world.hub.post(.accountRemoved)
        #expect(!model.isAvailable)
        model.reload()
        #expect(!model.isAvailable && model.notificationsState == nil)

        model.showsPhotos = false
        await model.requestNotificationAccess()
        #expect(world.engine.writes.isEmpty)

        world.hub.postStarted()
        #expect(model.isAvailable && model.notificationsState != nil)
    }

    // MARK: - Switches

    @Test func eachSwitchWritesItsSettingAndShowsWhatTheEngineKept() {
        let world = SettingsWorld()
        let model = world.makeModel()
        model.reload()

        model.showsPhoneNotifications = false
        model.alertsLowBattery = false
        model.pausesNoisyApps = false
        model.pausesAllNotifications = true
        model.usesPhoneContacts = false
        model.showsPhotos = false
        model.notifiesPhotos = false
        model.allowsPhotoDeletion = false
        #expect(world.engine.writes == [
            "setPcPermission(false)",
            "setLowBatteryNotificationsEnabled(false)",
            "setThrottlingEnabled(false)",
            "setMuteAll(true)",
            "setPcConsent(false)",
            "setConsent(false)",
            "setNewPhotoNotifications(false)",
            "setDeletionAllowed(false)",
        ])
        #expect(!model.showsPhoneNotifications && !model.alertsLowBattery && !model.pausesNoisyApps)
        #expect(model.pausesAllNotifications)
        #expect(!model.usesPhoneContacts)
        #expect(!model.showsPhotos && !model.notifiesPhotos && !model.allowsPhotoDeletion)
        #expect(model.failure == nil)
    }

    @Test func messagingSwitchesWriteBackTheOtherSettingsUnchanged() {
        let world = SettingsWorld()
        let model = world.makeModel()
        model.reload()
        let original = SettingsFixtures.messaging

        model.notifiesMessages = false
        #expect(world.messaging.written == [MessagingSettings(
            receiveMms: original.receiveMms, autoDownloadMms: original.autoDownloadMms, sendMms: original.sendMms,
            pcConsent: original.pcConsent, messageNotifications: false
        )])

        model.downloadsPictures = false
        #expect(world.messaging.written.last == MessagingSettings(
            receiveMms: original.receiveMms, autoDownloadMms: false, sendMms: original.sendMms,
            pcConsent: original.pcConsent, messageNotifications: false
        ))

        model.showsMessages = false
        #expect(world.messaging.written.last == MessagingSettings(
            receiveMms: original.receiveMms, autoDownloadMms: false, sendMms: original.sendMms,
            pcConsent: false, messageNotifications: false
        ))
        #expect(!model.showsMessages && !model.notifiesMessages && !model.downloadsPictures)
    }

    @Test func aSettingTheEngineRefusesIsReportedAndKeepsItsValue() {
        let world = SettingsWorld()
        let model = world.makeModel()
        model.reload()
        let refusal = FeatureApiError.Store(message: "disk full")
        world.engine.failNextWrite = refusal

        model.showsPhotos = false
        #expect(model.failure == SettingsFailure(title: "Couldn't Change the Setting", message: UserText.describe(refusal)))
        #expect(model.showsPhotos)
    }

    @Test func pausingAllNotificationsSaysUntilWhen() {
        let world = SettingsWorld()
        let until = world.now.addingTimeInterval(24 * 3600)
        world.notifications.current.mutedAllUntilMs = SettingsFixtures.unixMs(until)
        let model = world.makeModel()
        model.reload()
        #expect(model.pausesAllNotifications)
        #expect(model.pausedUntilText == "Paused until tomorrow at " + until.formatted(date: .omitted, time: .shortened))
    }

    // MARK: - Clipboard

    /// The phone's toggle is only the phone's; this Mac's global switch
    /// is never turned on by it.
    @Test func sharingTheClipboardSetsOnlyThePhonesToggle() {
        let world = SettingsWorld()
        world.clipboard.toggle = false
        let model = world.makeModel()
        model.reload()
        #expect(model.clipboardToggleApplies)
        #expect(!model.sharesClipboard)

        model.sharesClipboard = true
        #expect(world.engine.writes == ["setPhoneToggle(true)"])
        #expect(model.sharesClipboard)

        model.sharesClipboard = false
        #expect(world.engine.writes == ["setPhoneToggle(true)", "setPhoneToggle(false)"])
        #expect(!model.sharesClipboard)
    }

    /// The pane offers the per-phone toggle only while the global option is on and the phone
    /// supports clipboard sharing.
    @Test func thePhonesToggleAppliesOnlyWithTheGlobalSwitchOnAndACapablePhone() {
        let world = SettingsWorld()
        world.clipboard.global = false
        world.clipboard.toggle = true
        let model = world.makeModel()
        model.reload()
        #expect(!model.clipboardToggleApplies)
        #expect(!model.sharesClipboard)

        world.clipboard.global = true
        world.clipboard.capable = false
        world.hub.post(.clipboard(change: .globalSwitchChanged(enabled: true)))
        #expect(!model.clipboardToggleApplies)
        #expect(!model.sharesClipboard)

        world.clipboard.capable = true
        world.hub.post(.clipboard(change: .phoneChanged))
        #expect(model.clipboardToggleApplies)
        #expect(model.sharesClipboard)
        #expect(world.engine.writes.isEmpty)
    }

    // MARK: - Apps

    @Test func choicesMapToFlagsAndKeepTheOtherBits() {
        let flags = { (feed: Bool, banner: Bool) in AppNotificationFlags(feed: feed, banner: banner, otherBits: 0b1100) }
        #expect(SettingsNotificationChoice(flags(true, true)) == .bannersAndList)
        #expect(SettingsNotificationChoice(flags(true, false)) == .listOnly)
        #expect(SettingsNotificationChoice(flags(false, false)) == .off)
        #expect(SettingsNotificationChoice(flags(false, true)) == .off)

        let current = flags(false, true)
        #expect(SettingsNotificationChoice.bannersAndList.flags(keeping: current) == flags(true, true))
        #expect(SettingsNotificationChoice.listOnly.flags(keeping: current) == flags(true, false))
        #expect(SettingsNotificationChoice.off.flags(keeping: current) == flags(false, false))
        for choice in SettingsNotificationChoice.allCases {
            #expect(SettingsNotificationChoice(choice.flags(keeping: current)) == choice)
        }
    }

    @Test func appsAreNamedAfterTheirNewestNotificationSortedAndShowTheirMutes() {
        let world = SettingsWorld()
        world.notifications.items = [
            SettingsFixtures.item("1", package: "com.slack", appName: "Slack"),
            SettingsFixtures.item("2", package: "com.kakao.talk", appName: nil),
            SettingsFixtures.item("3", package: "com.kakao.talk", appName: "KakaoTalk"),
            SettingsFixtures.item("4", package: "com.google.calendar", appName: "calendar"),
            SettingsFixtures.item("5", package: "com.slack", appName: "Slack (old)"),
            SettingsFixtures.item("6", package: "org.example.unnamed", appName: "  "),
        ]

        let inAnHour = world.now.addingTimeInterval(3600)
        let tomorrow = world.now.addingTimeInterval(20 * 3600)
        world.notifications.appSettings = [
            "com.slack": SettingsFixtures.setting("com.slack", feed: true, banner: false, mutedUntil: inAnHour),
            "com.google.calendar": SettingsFixtures.setting("com.google.calendar", feed: true, banner: true, mutedUntil: tomorrow),
        ]

        let model = world.makeModel()
        model.reload()

        #expect(model.notificationApps.map(\.name) == ["calendar", "KakaoTalk", "org.example.unnamed", "Slack"])
        #expect(model.notificationApps.map(\.choice) == [.bannersAndList, .bannersAndList, .bannersAndList, .listOnly])
        #expect(model.notificationApps.map(\.mutedText) == [
            "Muted until tomorrow at " + tomorrow.formatted(date: .omitted, time: .shortened),
            nil,
            nil,
            "Muted until " + inAnHour.formatted(date: .omitted, time: .shortened),
        ])
    }

    @Test func aChoiceAsksThePhoneAndShowsTheChoiceMeanwhile() async {
        let world = SettingsWorld()
        world.notifications.items = [SettingsFixtures.item("1", package: "com.slack", appName: "Slack")]
        world.notifications.appSettings["com.slack"] = SettingsFixtures.setting("com.slack", feed: true, banner: true, otherBits: 0b1100)
        let model = world.makeModel()
        model.reload()
        world.notifications.changeGate.close()

        let change = Task { await model.setChoice(.listOnly, forApp: "com.slack") }
        while !world.notifications.changeGate.hasArrived {
            await Task.yield()
        }

        #expect(model.pendingApps == ["com.slack"])
        #expect(model.notificationApps.first?.choice == .listOnly)

        world.notifications.items.append(SettingsFixtures.item("2", package: "com.slack", appName: "Slack"))
        world.hub.post(.notifications(change: .listChanged))
        #expect(model.notificationApps.first?.choice == .listOnly)

        world.notifications.changeGate.open()
        await change.value

        #expect(model.pendingApps.isEmpty)
        #expect(world.engine.writes == ["changeAppSetting(com.slack, feed: true, banner: false, otherBits: 12)"])
        #expect(model.notificationApps.first?.flags == AppNotificationFlags(feed: true, banner: false, otherBits: 0b1100))
        #expect(model.failure == nil)

        await model.setChoice(.listOnly, forApp: "com.slack")
        #expect(world.engine.writes.count == 1)
    }

    @Test func aChoiceThePhoneDoesNotTakeIsReportedAndUndone() async {
        let world = SettingsWorld()
        world.notifications.items = [SettingsFixtures.item("1", package: "com.slack", appName: "Slack")]
        world.notifications.changeOutcome = .timedOut
        let model = world.makeModel()
        model.reload()

        await model.setChoice(.off, forApp: "com.slack")
        #expect(model.failure == SettingsFailure(
            title: "Couldn't Change Notifications from Slack", message: UserText.failure(.timedOut) ?? ""
        ))
        #expect(model.notificationApps.first?.choice == .bannersAndList)
    }

    @Test func anAppTurnedOffStaysListedAcrossLaunches() async {
        let world = SettingsWorld()
        world.notifications.items = [SettingsFixtures.item("1", package: "com.slack", appName: "Slack")]
        let model = world.makeModel()
        model.reload()

        await model.setChoice(.off, forApp: "com.slack")

        // The list hides an Off app's notifications, and the phone sends no more.
        world.notifications.items = []
        world.hub.post(.notifications(change: .listChanged))
        #expect(model.notificationApps.map(\.name) == ["Slack"])
        #expect(model.notificationApps.first?.choice == .off)

        let relaunched = world.makeModel()
        relaunched.reload()
        #expect(relaunched.notificationApps.map(\.name) == ["Slack"])
        #expect(relaunched.notificationApps.first?.choice == .off)
    }

    // MARK: - Requests to the phone

    @Test func theNotificationsPaneAsksThePhoneForItsAppSettings() async {
        let world = SettingsWorld()
        world.notifications.settingsViewOutcome = .timedOut
        let model = world.makeModel()
        await model.notificationsPaneAppeared()
        #expect(world.engine.writes == ["openSettingsView"])
        #expect(model.failure == nil)
    }

    @Test func accessRequestsReportOnlyWhatDidNotWork() async {
        let world = SettingsWorld()
        let model = world.makeModel()
        model.reload()

        await model.requestNotificationAccess()
        #expect(model.failure == nil)

        world.notifications.accessOutcome = .denied
        await model.requestSensitiveContent()
        #expect(model.failure == SettingsFailure(title: "Couldn't Request Access", message: UserText.failure(.denied) ?? ""))

        model.failure = nil
        world.contacts.permission = ContactsPermissionResult(outcome: .success, timedOut: true, sync: nil)
        await model.requestContactsAccess()
        #expect(model.failure == SettingsFailure(title: "Couldn't Request Access", message: UserText.failure(.timedOut) ?? ""))

        model.failure = nil
        world.contacts.permission = ContactsPermissionResult(outcome: .success, timedOut: false, sync: .failure)
        await model.requestContactsAccess()
        #expect(model.failure == nil)
        #expect(model.pendingRequests.isEmpty)
        #expect(world.engine.writes == ["requestAccess", "requestSensitiveAccess", "requestPermission", "requestPermission"])
    }

    @Test func callHistoryIsSwitchedOnThenAllowedOnThePhone() async {
        let world = SettingsWorld()
        let model = world.makeModel()
        model.reload()
        #expect(model.callingState?.access == .pcConsentRequired)
        #expect(!model.showsCallHistory)

        model.showsCallHistory = true
        #expect(model.callingState?.access == .devicePermissionRequired)

        await model.requestCallHistoryAccess()
        #expect(model.callingState?.access == .granted)
        #expect(model.failure == nil)
        #expect(world.engine.writes == ["calling.setPcConsent(true)", "calling.requestPermission"])
    }
}

// MARK: - Fixtures

private enum SettingsFixtures {
    static let messaging = MessagingSettings(
        receiveMms: false, autoDownloadMms: true, sendMms: false, pcConsent: true, messageNotifications: true
    )

    static func unixMs(_ date: Date) -> Int64 {
        Int64((date.timeIntervalSince1970 * 1000).rounded())
    }

    static func item(_ key: String, package: String, appName: String?) -> PhoneNotification {
        PhoneNotification(
            key: key, packageName: package, appName: appName, title: "Title", subtitle: nil, body: "Body",
            conversation: [], timeMs: 0, pinned: false, openTarget: .notifications, launchSupported: false,
            isReadOnlyGroupMessage: false, reply: nil, actions: []
        )
    }

    static func setting(
        _ package: String, feed: Bool, banner: Bool, otherBits: UInt32 = 0, mutedUntil: Date? = nil
    ) -> AppNotificationSetting {
        AppNotificationSetting(
            packageName: package,
            flags: AppNotificationFlags(feed: feed, banner: banner, otherBits: otherBits),
            mutedUntilMs: mutedUntil.map(unixMs)
        )
    }
}

// MARK: - Fakes

/// What the fakes share: whether the engine runs, and what the model changed, in order. A test
/// awaits every call it starts, so the fakes need no locking beyond the gate's.
private final class SettingsFakeEngine: @unchecked Sendable {
    var started = true
    var writes: [String] = []
    var failNextWrite: FeatureApiError?

    func check() throws {
        guard started else { throw FeatureApiError.NotStarted }
    }

    func write(_ call: String) throws {
        try check()
        if let failure = failNextWrite {
            failNextWrite = nil
            throw failure
        }
        writes.append(call)
    }
}

/// Holds a fake's call until the test opens it, so the test can look at the model meanwhile.
private final class SettingsGate: @unchecked Sendable {
    private let lock = NSLock()
    private var closed = false
    private var arrived = false
    private var waiting: CheckedContinuation<Void, Never>?

    var hasArrived: Bool { lock.withLock { arrived } }

    func close() {
        lock.withLock { closed = true }
    }

    func open() {
        let continuation = lock.withLock {
            closed = false
            defer { waiting = nil }
            return waiting
        }
        continuation?.resume()
    }

    func pass() async {
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            let held = lock.withLock {
                arrived = true
                if closed {
                    waiting = continuation
                }
                return closed
            }

            if !held {
                continuation.resume()
            }
        }
    }
}

private final class SettingsFakePhone: PhoneApiProtocol, @unchecked Sendable {
    static let firstPhone = "phone-1"
    let engine: SettingsFakeEngine
    var id = firstPhone

    init(engine: SettingsFakeEngine) {
        self.engine = engine
    }

    func info() throws -> PhoneInfo {
        try engine.check()
        return PhoneInfo(
            id: id, name: "Pixel", manufacturer: nil, model: nil, osVersion: nil, connected: true, allFeaturesDisabled: false
        )
    }

    func phones() throws -> [PhoneChoice] {
        [PhoneChoice(id: id, name: "Pixel", model: nil, lastSeen: 0, selected: true)]
    }

    func select(id: String) async throws {
        self.id = id
    }

    func accountPcs() throws -> [AccountPc] { [] }

    func unlinkPc(id: String) async throws {}
}

private final class SettingsFakeNotifications: NotificationsApiProtocol, @unchecked Sendable {
    let engine: SettingsFakeEngine
    var current = NotificationsState(
        capable: true, appListSupported: true, pcPermission: true, devicePermission: true,
        sensitivePermission: false, sensitivePromptOffered: true, syncing: false, lastUpdatedMs: nil,
        mutedAllUntilMs: nil, throttlingEnabled: true, lowRamDevice: false, setupCompleted: true,
        postNotificationNeeded: false
    )
    var items: [PhoneNotification] = []
    /// Apps without an entry read as the engine's default, banners and list.
    var appSettings: [String: AppNotificationSetting] = [:]
    var changeOutcome = RequestOutcome.success
    var accessOutcome = RequestOutcome.success
    var settingsViewOutcome: RequestOutcome? = .success
    /// How many app settings were read.
    var appSettingReads = 0
    let changeGate = SettingsGate()
    /// When `setMuteAll(on: true)` mutes until.
    var muteAllUntilMs: Int64 = 1

    init(engine: SettingsFakeEngine) {
        self.engine = engine
    }

    func state() throws -> NotificationsState {
        try engine.check()
        return current
    }

    func list() throws -> [PhoneNotification] {
        try engine.check()
        return items
    }

    func appSetting(packageName: String) throws -> AppNotificationSetting {
        try engine.check()
        appSettingReads += 1
        return appSettings[packageName] ?? SettingsFixtures.setting(packageName, feed: true, banner: true)
    }

    func changeAppSetting(packageName: String, flags: AppNotificationFlags) async throws -> RequestOutcome {
        try engine.write("changeAppSetting(\(packageName), feed: \(flags.feed), banner: \(flags.banner), otherBits: \(flags.otherBits))")
        await changeGate.pass()
        if changeOutcome == .success {
            appSettings[packageName] = AppNotificationSetting(
                packageName: packageName, flags: flags, mutedUntilMs: appSettings[packageName]?.mutedUntilMs
            )
        }
        return changeOutcome
    }

    func setPcPermission(on: Bool) throws {
        try engine.write("setPcPermission(\(on))")
        current.pcPermission = on
    }

    func setThrottlingEnabled(on: Bool) throws {
        try engine.write("setThrottlingEnabled(\(on))")
        current.throttlingEnabled = on
    }

    func setMuteAll(on: Bool) throws {
        try engine.write("setMuteAll(\(on))")
        current.mutedAllUntilMs = on ? muteAllUntilMs : nil
    }

    func openSettingsView() async throws -> RequestOutcome? {
        try engine.write("openSettingsView")
        return settingsViewOutcome
    }

    func requestAccess() async throws -> RequestOutcome {
        try engine.write("requestAccess")
        return accessOutcome
    }

    func requestSensitiveAccess() async throws -> RequestOutcome {
        try engine.write("requestSensitiveAccess")
        return accessOutcome
    }

    func applyPreferences(change: NotificationPreferencesChange) async throws -> NotificationPreferencesResult {
        preconditionFailure("the settings do not call \(#function)")
    }

    func clearAll() async throws -> RequestOutcome? {
        preconditionFailure("the settings do not call \(#function)")
    }

    func dismiss(key: String) async throws -> RequestOutcome {
        preconditionFailure("the settings do not call \(#function)")
    }

    func muteApp(packageName: String, duration: NotificationMuteDuration) throws {
        preconditionFailure("the settings do not call \(#function)")
    }

    func pressAction(key: String, actionIndex: Int32) throws {
        preconditionFailure("the settings do not call \(#function)")
    }

    func reply(key: String, actionIndex: Int32, text: String) throws {
        preconditionFailure("the settings do not call \(#function)")
    }

    func setAppLocalSetting(packageName: String, flags: AppNotificationFlags) throws {
        preconditionFailure("the settings do not call \(#function)")
    }

    func setPinned(key: String, pinned: Bool) throws {
        preconditionFailure("the settings do not call \(#function)")
    }

    func toggleAppFromList(packageName: String, enabled: Bool) async throws -> RequestOutcome {
        preconditionFailure("the settings do not call \(#function)")
    }

    func undoAppTurnOff(packageName: String, previous: AppNotificationFlags) async throws -> RequestOutcome {
        preconditionFailure("the settings do not call \(#function)")
    }

    func unmuteApp(packageName: String) throws {
        preconditionFailure("the settings do not call \(#function)")
    }
}

private final class SettingsFakeMessaging: MessagingApiProtocol, @unchecked Sendable {
    let engine: SettingsFakeEngine
    var current = MessagingState(
        phonePermission: true, settings: SettingsFixtures.messaging, upToDate: true, syncing: false,
        lastUpdatedMs: nil, setupCompletedOnce: true, proximity: nil
    )
    var written: [MessagingSettings] = []

    init(engine: SettingsFakeEngine) {
        self.engine = engine
    }

    func state() throws -> MessagingState {
        try engine.check()
        return current
    }

    func setSettings(settings: MessagingSettings) throws {
        try engine.write("setSettings")
        written.append(settings)
        current.settings = settings
    }

    func composedConversationReplaced(threadId: Int64) throws {
        preconditionFailure("the settings do not call \(#function)")
    }

    func conversationSelected(threadId: Int64?) throws {
        preconditionFailure("the settings do not call \(#function)")
    }

    func conversations() throws -> [Conversation] {
        preconditionFailure("the settings do not call \(#function)")
    }

    func downloadPart(part: MessagePartRef) async throws -> RequestOutcome {
        preconditionFailure("the settings do not call \(#function)")
    }

    func messages(threadId: Int64) throws -> [Message] {
        preconditionFailure("the settings do not call \(#function)")
    }

    func newConversationMessages() throws -> [NewConversationMessage] {
        preconditionFailure("the settings do not call \(#function)")
    }

    func newMessageLimits() throws -> NewMessageLimits {
        preconditionFailure("the settings do not call \(#function)")
    }

    func partBytes(part: MessagePartRef) throws -> Data? {
        preconditionFailure("the settings do not call \(#function)")
    }

    func messagesViewChanged(active: Bool) throws {
        preconditionFailure("the settings do not call \(#function)")
    }

    func requestPermission() async throws -> RequestOutcome {
        preconditionFailure("the settings do not call \(#function)")
    }

    func retry(sendId: Int64) async throws -> SendResult {
        preconditionFailure("the settings do not call \(#function)")
    }

    func selectSubscription(threadId: Int64, subscriptionId: Int64?) throws {
        preconditionFailure("the settings do not call \(#function)")
    }

    func send(target: MessageTarget, text: String, attachments: [MessageAttachment]) async throws -> SendResult {
        preconditionFailure("the settings do not call \(#function)")
    }

    func setConversationFlags(threadId: Int64, muted: Bool, hidden: Bool) throws {
        preconditionFailure("the settings do not call \(#function)")
    }

    func subscriptions() throws -> [SimSubscription] {
        preconditionFailure("the settings do not call \(#function)")
    }

    func windowFocused(focused: Bool) throws {
        preconditionFailure("the settings do not call \(#function)")
    }
}

private final class SettingsFakeContacts: ContactsApiProtocol, @unchecked Sendable {
    let engine: SettingsFakeEngine
    var current = ContactsState(capable: true, pcConsent: true, access: .granted, upToDate: true, lastUpdatedMs: nil)
    var permission = ContactsPermissionResult(outcome: .success, timedOut: false, sync: .success)

    init(engine: SettingsFakeEngine) {
        self.engine = engine
    }

    func state() throws -> ContactsState {
        try engine.check()
        return current
    }

    func setPcConsent(consent: Bool) throws {
        try engine.write("setPcConsent(\(consent))")
        current.pcConsent = consent
        current.access = consent ? .granted : .pcConsentRequired
    }

    func list() throws -> [ContactListItem] { [] }

    func card(contactId: Int64) throws -> ContactCard? { nil }

    func requestPermission() async throws -> ContactsPermissionResult {
        try engine.write("requestPermission")
        return permission
    }

    func suggestions(query: String, limit: UInt32) throws -> [ContactSuggestion] {
        preconditionFailure("the settings do not call \(#function)")
    }

    func thumbnail(contactId: Int64) throws -> Data? {
        preconditionFailure("the settings do not call \(#function)")
    }
}

private final class SettingsFakeCalling: CallingApiProtocol, @unchecked Sendable {
    let engine: SettingsFakeEngine
    var current = CallingState(
        access: .pcConsentRequired, pcConsent: false, upToDate: true, lastUpdatedMs: nil, refreshing: false, unseenMissed: 0
    )
    var permission = CallsPermissionResult(outcome: .accepted, timedOut: false, refresh: .success)

    init(engine: SettingsFakeEngine) {
        self.engine = engine
    }

    func state() throws -> CallingState {
        try engine.check()
        return current
    }

    func setPcConsent(consent: Bool) throws {
        try engine.write("calling.setPcConsent(\(consent))")
        current.pcConsent = consent
        current.access = consent ? .devicePermissionRequired : .pcConsentRequired
    }

    func requestPermission() async throws -> CallsPermissionResult {
        try engine.write("calling.requestPermission")
        if permission.outcome == .accepted {
            current.access = .granted
        }
        return permission
    }

    func history() throws -> [CallHistoryItem] {
        preconditionFailure("the settings do not call \(#function)")
    }

    func markSeen() throws {
        preconditionFailure("the settings do not call \(#function)")
    }

    func refreshIfEmpty() async throws -> RequestOutcome? {
        preconditionFailure("the settings do not call \(#function)")
    }
}

private final class SettingsFakePhotos: PhotosApiProtocol, @unchecked Sendable {
    let engine: SettingsFakeEngine
    var current = PhotosState(
        enabled: true, consent: true, photosPermission: true, fullAccessReminder: false, deletionAllowed: true,
        deleteAvailable: true, newPhotoNotifications: true, syncing: false, upToDate: true, lastSyncMs: nil
    )

    init(engine: SettingsFakeEngine) {
        self.engine = engine
    }

    func state() throws -> PhotosState {
        try engine.check()
        return current
    }

    func setConsent(on: Bool) throws {
        try engine.write("setConsent(\(on))")
        current.consent = on
    }

    func setNewPhotoNotifications(on: Bool) throws {
        try engine.write("setNewPhotoNotifications(\(on))")
        current.newPhotoNotifications = on
    }

    func setDeletionAllowed(on: Bool) throws {
        try engine.write("setDeletionAllowed(\(on))")
        current.deletionAllowed = on
    }

    func contextMenuOpened(id: Int64) throws {
        preconditionFailure("the settings do not call \(#function)")
    }

    func delete(id: Int64) async throws -> PhotoDeleteResult {
        preconditionFailure("the settings do not call \(#function)")
    }

    func dismissFullAccessReminder() throws {
        preconditionFailure("the settings do not call \(#function)")
    }

    func fullImage(id: Int64) throws -> Data? {
        preconditionFailure("the settings do not call \(#function)")
    }

    func fullImageForAction(id: Int64, action: PhotoAction) async throws -> Data? {
        preconditionFailure("the settings do not call \(#function)")
    }

    func gallery() throws -> [Photo] {
        preconditionFailure("the settings do not call \(#function)")
    }

    func galleryLeft() throws {
        preconditionFailure("the settings do not call \(#function)")
    }

    func gridLayout(visible: [Int64]) throws {
        preconditionFailure("the settings do not call \(#function)")
    }

    func itemHidden(id: Int64) throws {
        preconditionFailure("the settings do not call \(#function)")
    }

    func itemVisible(id: Int64) throws {
        preconditionFailure("the settings do not call \(#function)")
    }

    func openItem(id: Int64) throws -> Bool {
        preconditionFailure("the settings do not call \(#function)")
    }

    func prepareDelete(id: Int64) async throws -> PhotoDeleteCheck {
        preconditionFailure("the settings do not call \(#function)")
    }

    func requestDeletePermission() async throws -> RequestOutcome {
        preconditionFailure("the settings do not call \(#function)")
    }

    func requestFullAccess() async throws -> RequestOutcome {
        preconditionFailure("the settings do not call \(#function)")
    }

    func requestPhotoAccess() async throws -> RequestOutcome {
        preconditionFailure("the settings do not call \(#function)")
    }

    func saveAsName(id: Int64) throws -> PhotoFileName {
        preconditionFailure("the settings do not call \(#function)")
    }

    func thumbnail(id: Int64) throws -> Data? {
        preconditionFailure("the settings do not call \(#function)")
    }

    func viewerClosed() throws {
        preconditionFailure("the settings do not call \(#function)")
    }

    func viewerItemShown(id: Int64) throws {
        preconditionFailure("the settings do not call \(#function)")
    }

    func viewerSelection(id: Int64) throws {
        preconditionFailure("the settings do not call \(#function)")
    }
}

private final class SettingsFakeClipboard: ClipboardApiProtocol, @unchecked Sendable {
    let engine: SettingsFakeEngine
    var global = true
    var toggle = true
    var capable = true

    init(engine: SettingsFakeEngine) {
        self.engine = engine
    }

    func globalSwitch() throws -> Bool {
        try engine.check()
        return global
    }

    func phoneState() throws -> ClipboardPhoneState? {
        try engine.check()
        return ClipboardPhoneState(
            toggle: toggle, applicable: capable && global, capable: capable, available: true,
            supported: toggle && capable && global
        )
    }

    func setGlobalSwitch(enabled: Bool) throws {
        try engine.write("setGlobalSwitch(\(enabled))")
        global = enabled
    }

    func setPhoneToggle(enabled: Bool) throws {
        try engine.write("setPhoneToggle(\(enabled))")
        toggle = enabled
    }
}

private final class SettingsFakeDeviceStatus: DeviceStatusApiProtocol, @unchecked Sendable {
    let engine: SettingsFakeEngine
    var lowBattery = true

    init(engine: SettingsFakeEngine) {
        self.engine = engine
    }

    func lowBatteryNotificationsEnabled() throws -> Bool {
        try engine.check()
        return lowBattery
    }

    func setLowBatteryNotificationsEnabled(enabled: Bool) throws {
        try engine.write("setLowBatteryNotificationsEnabled(\(enabled))")
        lowBattery = enabled
    }

    func acceptPrompt(prompt: StatusPrompt) throws {
        preconditionFailure("the settings do not call \(#function)")
    }

    func cycleRingerMode() throws -> StatusControlOutcome {
        preconditionFailure("the settings do not call \(#function)")
    }

    func status() throws -> PhoneStatus {
        preconditionFailure("the settings do not call \(#function)")
    }

    func toggleDoNotDisturb() throws -> StatusControlOutcome {
        preconditionFailure("the settings do not call \(#function)")
    }

    func ringMyPhone() throws -> StatusControlOutcome {
        preconditionFailure("the settings do not call \(#function)")
    }

    func stopRinging() throws -> StatusControlOutcome {
        preconditionFailure("the settings do not call \(#function)")
    }
}

/// The fakes behind one model, its event hub, and a defaults domain of its own that a second model
/// (a relaunch) can share.
@MainActor
private final class SettingsWorld {
    let engine: SettingsFakeEngine
    let notifications: SettingsFakeNotifications
    let messaging: SettingsFakeMessaging
    let contacts: SettingsFakeContacts
    let calling: SettingsFakeCalling
    let photos: SettingsFakePhotos
    let clipboard: SettingsFakeClipboard
    let deviceStatus: SettingsFakeDeviceStatus
    let phone: SettingsFakePhone
    let hub: EngineEventHub
    /// 10:00 today: an hour later is the same day, 20 hours later the next.
    let now: Date
    private let defaults = InMemoryDefaults()

    init() {
        let engine = SettingsFakeEngine()
        self.engine = engine
        hub = EngineEventHub()
        notifications = SettingsFakeNotifications(engine: engine)
        messaging = SettingsFakeMessaging(engine: engine)
        contacts = SettingsFakeContacts(engine: engine)
        calling = SettingsFakeCalling(engine: engine)
        photos = SettingsFakePhotos(engine: engine)
        clipboard = SettingsFakeClipboard(engine: engine)
        deviceStatus = SettingsFakeDeviceStatus(engine: engine)
        phone = SettingsFakePhone(engine: engine)

        guard let morning = Calendar.current.date(bySettingHour: 10, minute: 0, second: 0, of: Date()) else {
            preconditionFailure("10:00 exists today")
        }
        now = morning
    }

    func makeModel() -> SettingsModel {
        let now = self.now
        return SettingsModel(
            notifications: notifications,
            messaging: messaging,
            contacts: contacts,
            calling: calling,
            photos: photos,
            clipboard: clipboard,
            deviceStatus: deviceStatus,
            phone: phone,
            hub: hub,
            defaults: defaults,
            now: { now }
        )
    }
}

/// A defaults domain that lives only in memory: a `UserDefaults(suiteName:)` domain leaves its plist in
/// `~/Library/Preferences` even after `removePersistentDomain`, one file per test run.
private final class InMemoryDefaults: UserDefaults, @unchecked Sendable {
    private var values: [String: Any] = [:]

    init() {
        super.init(suiteName: nil)!
    }

    override func object(forKey defaultName: String) -> Any? {
        values[defaultName]
    }

    override func dictionary(forKey defaultName: String) -> [String: Any]? {
        values[defaultName] as? [String: Any]
    }

    override func set(_ value: Any?, forKey defaultName: String) {
        values[defaultName] = value
    }

    override func removeObject(forKey defaultName: String) {
        values[defaultName] = nil
    }
}
