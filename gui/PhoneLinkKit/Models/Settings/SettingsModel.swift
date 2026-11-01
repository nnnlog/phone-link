import Foundation
import Observation
import os
import PhoneLinkEngine

/// How an app's notifications reach this Mac: the settings list's three choices over the app's feed
/// flag (the phone sends its notifications, so they are listed) and banner flag (this Mac shows them
/// as banners). Three values make a pop-up rather than two switches (HIG › Toggles).
public enum SettingsNotificationChoice: CaseIterable, Sendable {
    case bannersAndList
    case listOnly
    case off

    /// A banner flag without the feed flag is Off: the phone sends nothing for the app then.
    public init(_ flags: AppNotificationFlags) {
        switch (flags.feed, flags.banner) {
        case (true, true): self = .bannersAndList
        case (true, false): self = .listOnly
        case (false, _): self = .off
        }
    }

    /// The flags of this choice; the bits the phone reported beyond the two stay as they are.
    public func flags(keeping current: AppNotificationFlags) -> AppNotificationFlags {
        switch self {
        case .bannersAndList: AppNotificationFlags(feed: true, banner: true, otherBits: current.otherBits)
        case .listOnly: AppNotificationFlags(feed: true, banner: false, otherBits: current.otherBits)
        case .off: AppNotificationFlags(feed: false, banner: false, otherBits: current.otherBits)
        }
    }

    public var title: String {
        switch self {
        case .bannersAndList: String(localized: "Banners and List")
        case .listOnly: String(localized: "List Only")
        case .off: String(localized: "Off", comment: "How an app's notifications reach this Mac")
        }
    }
}

/// An app in the notification settings.
public struct SettingsNotificationApp: Identifiable, Equatable, Sendable {
    public let packageName: String
    public let name: String
    public let flags: AppNotificationFlags
    /// "Muted until 4:05 PM" while the app's banners are muted.
    public let mutedText: String?
    /// When the mute ends, Unix milliseconds.
    public let mutedUntilMs: Int64?

    public var id: String { packageName }
    public var choice: SettingsNotificationChoice { SettingsNotificationChoice(flags) }
}

/// A request the settings send to the phone; its row waits while one is under way.
public enum SettingsPhoneRequest: Hashable, Sendable {
    case notificationAccess
    case sensitiveContent
    case contactsAccess
}

/// A change the user made that did not take, for an alert.
public struct SettingsFailure: Equatable, Sendable {
    public let title: String
    public let message: String
}

/// The Settings window's model: the features' settings as the engine holds them, and the user's
/// changes to them.
///
/// Every value is the engine's. The model reads it when the engine starts, when a pane appears and
/// when a feature reports a change, and reads it back after writing a change, so a control shows
/// what the engine kept — only an app's pop-up shows the user's choice while the phone answers.
/// Before the engine starts every read throws `NotStarted`: `isAvailable` stays false and the panes
/// show their controls disabled.
@MainActor
@Observable
public final class SettingsModel {
    /// The engine runs, so the settings can be read and changed.
    public private(set) var isAvailable = false
    public private(set) var notificationsState: NotificationsState?
    public private(set) var lowBatteryEnabled: Bool?
    /// The apps that sent a notification, by name.
    public private(set) var notificationApps: [SettingsNotificationApp] = []
    public private(set) var messagingSettings: MessagingSettings?
    public private(set) var contactsState: ContactsState?
    public private(set) var photosState: PhotosState?
    /// `nil` also while the phone is not in the account's device list.
    public private(set) var clipboardState: ClipboardPhoneState?
    /// Apps whose change waits for the phone's answer.
    public private(set) var pendingApps: Set<String> = []
    public private(set) var pendingRequests: Set<SettingsPhoneRequest> = []
    /// The last change of the user's that failed; the view shows it and clears it.
    public var failure: SettingsFailure?

    private let notifications: any NotificationsApiProtocol
    private let messaging: any MessagingApiProtocol
    private let contacts: any ContactsApiProtocol
    private let photos: any PhotosApiProtocol
    private let clipboard: any ClipboardApiProtocol
    private let deviceStatus: any DeviceStatusApiProtocol
    private let phone: any PhoneApiProtocol
    private let defaults: UserDefaults
    private let now: () -> Date
    /// Every app the selected phone's notification list has shown, package → name, kept across
    /// launches per phone: an app set to Off leaves the list — the phone stops sending its
    /// notifications and the list hides those it has — yet must stay here to be turned on again.
    @ObservationIgnored private var knownApps: [String: String] = [:]
    /// The phone `knownApps` belongs to.
    @ObservationIgnored private var knownAppsPhone: String?
    /// The Notifications pane is on screen: only then are the apps' settings read on every change of
    /// the list (with the pane closed a change only adds its apps to `knownApps`).
    @ObservationIgnored private var notificationsPaneVisible = false
    /// The account was removed or the phone left it: the engine still answers for the old phone
    /// until the next start, so nothing is read or changed until then.
    @ObservationIgnored private var suspended = false
    /// Reads the settings again when the next mute ends (the engine announces no expiry).
    @ObservationIgnored private var expiryReload: Task<Void, Never>?

    private static let knownAppsKey = PreferenceKeys.notificationSettingsApps
    private static let log = AppLog(category: "settings")

    public init(
        notifications: any NotificationsApiProtocol,
        messaging: any MessagingApiProtocol,
        contacts: any ContactsApiProtocol,
        photos: any PhotosApiProtocol,
        clipboard: any ClipboardApiProtocol,
        deviceStatus: any DeviceStatusApiProtocol,
        phone: any PhoneApiProtocol,
        hub: EngineEventHub,
        defaults: UserDefaults = .standard,
        now: @escaping () -> Date = { Date() }
    ) {
        self.notifications = notifications
        self.messaging = messaging
        self.contacts = contacts
        self.photos = photos
        self.clipboard = clipboard
        self.deviceStatus = deviceStatus
        self.phone = phone
        self.defaults = defaults
        self.now = now

        hub.onEvent { [weak self] event in
            self?.handle(event)
        }
        hub.onStart { [weak self] in
            self?.suspended = false
            self?.reload()
        }
    }

    // MARK: - Notifications

    /// "Show notifications from my phone".
    public var showsPhoneNotifications: Bool {
        get { notificationsState?.pcPermission ?? false }
        set {
            change { try notifications.setPcPermission(on: newValue) }
            reloadNotifications()
        }
    }

    /// "Alert me when the phone's battery is low".
    public var alertsLowBattery: Bool {
        get { lowBatteryEnabled ?? false }
        set {
            change { try deviceStatus.setLowBatteryNotificationsEnabled(enabled: newValue) }
            reloadLowBattery()
        }
    }

    /// "Pause apps that send too many notifications".
    public var pausesNoisyApps: Bool {
        get { notificationsState?.throttlingEnabled ?? false }
        set {
            change { try notifications.setThrottlingEnabled(on: newValue) }
            reloadNotifications()
        }
    }

    /// All notifications paused for 24 hours (the Notifications section's Pause Notifications).
    public var pausesAllNotifications: Bool {
        get { notificationsState?.mutedAllUntilMs != nil }
        set {
            change { try notifications.setMuteAll(on: newValue) }
            reloadNotifications()
        }
    }

    /// "Paused until 4:05 PM" while every banner is muted.
    public var pausedUntilText: String? {
        notificationsState?.mutedAllUntilMs.map { String(localized: "Paused until \(untilText($0))") }
    }

    /// Sets how an app's notifications reach this Mac. Changing whether they are listed asks the
    /// phone, which can take a moment: the pop-up shows the choice and waits meanwhile, then shows
    /// what the engine kept.
    public func setChoice(_ choice: SettingsNotificationChoice, forApp packageName: String) async {
        guard !pendingApps.contains(packageName),
              let index = notificationApps.firstIndex(where: { $0.packageName == packageName })
        else { return }

        let app = notificationApps[index]
        let flags = choice.flags(keeping: app.flags)
        guard flags != app.flags else { return }

        pendingApps.insert(packageName)
        notificationApps[index] = SettingsNotificationApp(
            packageName: packageName, name: app.name, flags: flags, mutedText: app.mutedText, mutedUntilMs: app.mutedUntilMs
        )

        defer {
            pendingApps.remove(packageName)
            reloadApps()
        }

        let title = String(localized: "Couldn't Change Notifications from \(app.name)")
        do {
            let outcome = try await notifications.changeAppSetting(packageName: packageName, flags: flags)
            if let message = UserText.failure(outcome) {
                Self.log.error("app notification setting not changed on the phone: \(outcome, privacy: .public)")
                failure = SettingsFailure(title: title, message: message)
            }
        } catch FeatureApiError.NotStarted {
            markUnavailable()
        } catch {
            Self.log.error("app notification setting not changed: \(error: error)")
            failure = SettingsFailure(title: title, message: UserText.describe(error))
        }
    }

    /// Asks the phone for notification access.
    public func requestNotificationAccess() async {
        await request(.notificationAccess) { try await notifications.requestAccess() }
        reloadNotifications()
    }

    /// Asks the phone for access to sensitive notification content.
    public func requestSensitiveContent() async {
        await request(.sensitiveContent) { try await notifications.requestSensitiveAccess() }
        reloadNotifications()
    }

    /// The Notifications pane appeared (the pane then reloads) or disappeared.
    public func setNotificationsPaneVisible(_ visible: Bool) {
        notificationsPaneVisible = visible
    }

    /// The notifications pane appeared: the phone is asked for its apps' settings (when it reports
    /// its apps), which arrive as a settings change.
    public func notificationsPaneAppeared() async {
        guard !suspended else { return }
        do {
            if let outcome = try await notifications.openSettingsView(), UserText.failure(outcome) != nil {
                Self.log.notice("phone app settings not sent: \(outcome, privacy: .public); the pane shows those last read")
            }
        } catch FeatureApiError.NotStarted {
            // Nothing to ask until the engine starts; the pane shows that already.
        } catch {
            Self.log.notice("phone app settings not requested: \(error: error); the pane shows those last read")
        }
    }

    // MARK: - Messages and contacts

    /// "Show messages on this Mac".
    public var showsMessages: Bool {
        get { messagingSettings?.pcConsent ?? false }
        set { changeMessagingSettings { $0.pcConsent = newValue } }
    }

    /// "Notify me about new messages".
    public var notifiesMessages: Bool {
        get { messagingSettings?.messageNotifications ?? false }
        set { changeMessagingSettings { $0.messageNotifications = newValue } }
    }

    /// "Download pictures automatically".
    public var downloadsPictures: Bool {
        get { messagingSettings?.autoDownloadMms ?? false }
        set { changeMessagingSettings { $0.autoDownloadMms = newValue } }
    }

    /// "Use my phone's contacts".
    public var usesPhoneContacts: Bool {
        get { contactsState?.pcConsent ?? false }
        set {
            change { try contacts.setPcConsent(consent: newValue) }
            reloadContacts()
        }
    }

    /// Asks the phone for contacts access; a grant is followed by a contacts sync.
    public func requestContactsAccess() async {
        await request(.contactsAccess) {
            let result = try await contacts.requestPermission()
            if let sync = result.sync, UserText.failure(sync) != nil {
                Self.log.notice("contacts not synced after the grant: \(sync, privacy: .public)")
            }
            return result.timedOut ? .timedOut : result.outcome
        }

        reloadContacts()
    }

    /// The engine takes the messaging settings whole: they are written back with one field changed.
    private func changeMessagingSettings(_ edit: (inout MessagingSettings) -> Void) {
        change {
            var settings = try messaging.state().settings
            edit(&settings)
            try messaging.setSettings(settings: settings)
        }

        reloadMessaging()
    }

    // MARK: - Photos

    /// "Show photos on this Mac".
    public var showsPhotos: Bool {
        get { photosState?.consent ?? false }
        set {
            change { try photos.setConsent(on: newValue) }
            reloadPhotos()
        }
    }

    /// "Notify me about new photos".
    public var notifiesPhotos: Bool {
        get { photosState?.newPhotoNotifications ?? false }
        set {
            change { try photos.setNewPhotoNotifications(on: newValue) }
            reloadPhotos()
        }
    }

    /// "Allow deleting photos from this Mac".
    public var allowsPhotoDeletion: Bool {
        get { photosState?.deletionAllowed ?? false }
        set {
            change { try photos.setDeletionAllowed(on: newValue) }
            reloadPhotos()
        }
    }

    // MARK: - Clipboard

    /// "Share clipboard": the phone's toggle. It shares only while it applies
    /// (`clipboardToggleApplies`); changing it never touches this Mac's global switch.
    public var sharesClipboard: Bool {
        get { clipboardState.map { $0.toggle && $0.applicable } ?? false }
        set {
            change { try clipboard.setPhoneToggle(enabled: newValue) }
            reloadClipboard()
        }
    }

    /// The phone's clipboard toggle applies — the phone supports clipboard sharing and this Mac's
    /// global switch is on — so the pane offers it only then.
    public var clipboardToggleApplies: Bool {
        clipboardState?.applicable == true
    }

    // MARK: - Loading

    /// Reads every setting again.
    public func reload() {
        reloadNotifications()
        reloadApps()
        reloadLowBattery()
        reloadMessaging()
        reloadContacts()
        reloadPhotos()
        reloadClipboard()
    }

    private func handle(_ event: EngineEvent) {
        switch event {
        case .accountRemoved, .phoneUnlinked:
            suspended = true
            markUnavailable()
        case .notifications(.stateChanged):
            reloadNotifications()
        case .notifications(.settingsChanged), .notifications(.listChanged):
            if notificationsPaneVisible {
                reloadApps()
            } else if let list = read("notification list not read", { try notifications.list() }) {
                remember(list)
            }
        case .messaging(.stateChanged):
            reloadMessaging()
        case .contacts(.stateChanged):
            reloadContacts()
        case .photos(.stateChanged):
            reloadPhotos()
        case .clipboard(.globalSwitchChanged), .clipboard(.phoneChanged):
            reloadClipboard()
        case let .resync(feature):
            switch feature {
            case .notifications:
                reloadNotifications()
                reloadApps()
            case .deviceStatus: reloadLowBattery()
            case .messaging: reloadMessaging()
            case .contacts: reloadContacts()
            case .photos: reloadPhotos()
            case .clipboard: reloadClipboard()
            case .wallpaper: break
            case .remoteSnipping: break
            case .fileTransfer, .phoneFiles, .connectedCamera, .phoneApps, .screenMirroring: break
            case .audio: break
            }
        default:
            break
        }
    }

    private func reloadNotifications() {
        if let state = read("notifications state not read", { try notifications.state() }) {
            notificationsState = state
            scheduleExpiryReload()
        }
    }

    private func reloadLowBattery() {
        if let enabled = read("low-battery setting not read", { try deviceStatus.lowBatteryNotificationsEnabled() }) {
            lowBatteryEnabled = enabled
        }
    }

    private func reloadMessaging() {
        if let state = read("messaging state not read", { try messaging.state() }) {
            messagingSettings = state.settings
        }
    }

    private func reloadContacts() {
        if let state = read("contacts state not read", { try contacts.state() }) {
            contactsState = state
        }
    }

    private func reloadPhotos() {
        if let state = read("photos state not read", { try photos.state() }) {
            photosState = state
        }
    }

    private func reloadClipboard() {
        if let state = read("clipboard state not read", { try clipboard.phoneState() }) {
            clipboardState = state
        }
    }

    /// The apps the list shows now and every app it showed before, with their settings.
    private func reloadApps() {
        guard let list = read("notification list not read", { try notifications.list() }),
              remember(list)
        else { return }

        // A row whose change waits for the phone keeps showing the user's choice until it ends.
        var chosen: [String: AppNotificationFlags] = [:]
        for app in notificationApps where pendingApps.contains(app.packageName) {
            chosen[app.packageName] = app.flags
        }

        var apps: [SettingsNotificationApp] = []
        for (packageName, name) in knownApps {
            guard let setting = read("app notification setting not read", {
                try notifications.appSetting(packageName: packageName)
            }) else { return }

            apps.append(SettingsNotificationApp(
                packageName: packageName,
                name: name,
                flags: chosen[packageName] ?? setting.flags,
                mutedText: setting.mutedUntilMs.map { String(localized: "Muted until \(untilText($0))") },
                mutedUntilMs: setting.mutedUntilMs
            ))
        }

        notificationApps = apps.sorted {
            switch $0.name.localizedStandardCompare($1.name) {
            case .orderedAscending: true
            case .orderedDescending: false
            case .orderedSame: $0.packageName < $1.packageName
            }
        }

        scheduleExpiryReload()
    }

    /// Adds the apps of `list` to the selected phone's remembered apps; `false` when the phone could
    /// not be read. The list is newest first, so an app is named after its newest notification that
    /// carries a name.
    @discardableResult
    private func remember(_ list: [PhoneNotification]) -> Bool {
        guard let phoneId = read("phone not read", { try phone.info().id }) else { return false }
        var stored = defaults.dictionary(forKey: Self.knownAppsKey) as? [String: [String: String]] ?? [:]
        if knownAppsPhone != phoneId {
            knownApps = stored[phoneId] ?? [:]
            knownAppsPhone = phoneId
        }

        var known = knownApps
        var named = Set<String>()
        for item in list where !named.contains(item.packageName) {
            if let name = item.appName?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty {
                known[item.packageName] = name
                named.insert(item.packageName)
            } else if known[item.packageName] == nil {
                known[item.packageName] = item.packageName
            }
        }

        if known != knownApps {
            knownApps = known
            stored[phoneId] = known
            defaults.set(stored, forKey: Self.knownAppsKey)
        }

        return true
    }

    /// Schedules a read of the notification settings when the earliest mute shown ends.
    private func scheduleExpiryReload() {
        expiryReload?.cancel()
        expiryReload = nil

        let nowMs = Int64((now().timeIntervalSince1970 * 1000).rounded())
        let ends = ([notificationsState?.mutedAllUntilMs] + notificationApps.map(\.mutedUntilMs))
            .compactMap { $0 }
            .filter { $0 > nowMs }
        guard let first = ends.min() else { return }

        // A moment past the end, so the engine reads the mute as over.
        let delay = Duration.milliseconds(first - nowMs) + .seconds(1)
        expiryReload = Task { [weak self] in
            try? await Task.sleep(for: delay)
            guard !Task.isCancelled, let self else { return }
            self.reloadNotifications()
            if self.notificationsPaneVisible {
                self.reloadApps()
            }
        }
    }

    // MARK: - Engine calls

    /// One read from the engine; `nil` when it failed. Before the engine starts that means there is
    /// nothing to show; any other failure is logged and the last value stays.
    private func read<Value>(_ subject: StaticString, _ body: () throws -> Value) -> Value? {
        guard !suspended else { return nil }
        do {
            let value = try body()
            if !isAvailable {
                isAvailable = true
            }
            return value
        } catch FeatureApiError.NotStarted {
            markUnavailable()
        } catch {
            Self.log.error("\(subject.description, privacy: .public): \(error: error)")
        }
        return nil
    }

    /// A change the user made; its failure is presented. Before the engine starts the controls are
    /// disabled, so a `NotStarted` here only means the engine stopped meanwhile.
    private func change(_ body: () throws -> Void) {
        guard !suspended else { return }
        do {
            try body()
        } catch FeatureApiError.NotStarted {
            markUnavailable()
        } catch {
            Self.log.error("setting not changed: \(error: error)")
            failure = SettingsFailure(title: String(localized: "Couldn't Change the Setting"), message: UserText.describe(error))
        }
    }

    /// A request the user sent to the phone; an outcome other than success is presented.
    private func request(_ kind: SettingsPhoneRequest, _ send: () async throws -> RequestOutcome) async {
        guard !suspended, !pendingRequests.contains(kind) else { return }
        pendingRequests.insert(kind)
        defer { pendingRequests.remove(kind) }

        do {
            let outcome = try await send()
            if let message = UserText.failure(outcome) {
                Self.log.error("phone access not granted: \(String(describing: kind), privacy: .public), \(outcome, privacy: .public)")
                failure = SettingsFailure(title: String(localized: "Couldn't Request Access"), message: message)
            }
        } catch FeatureApiError.NotStarted {
            markUnavailable()
        } catch {
            Self.log.error("phone access not requested: \(String(describing: kind), privacy: .public), \(error: error)")
            failure = SettingsFailure(title: String(localized: "Couldn't Request Access"), message: UserText.describe(error))
        }
    }

    /// The engine is not running: nothing is shown until it starts. (Every value comes from a read
    /// that made the model available, so an unavailable model holds none.)
    private func markUnavailable() {
        guard isAvailable else { return }
        isAvailable = false
        notificationsState = nil
        lowBatteryEnabled = nil
        notificationApps = []
        messagingSettings = nil
        contactsState = nil
        photosState = nil
        clipboardState = nil
    }

    /// "4:05 PM" today and "tomorrow at 4:05 PM" the next day — a mute lasts at most 24 hours — else
    /// a date and time.
    private func untilText(_ unixMs: Int64) -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(unixMs) / 1000)
        let today = now()
        let calendar = Calendar.current
        let time = date.formatted(date: .omitted, time: .shortened)

        if calendar.isDate(date, inSameDayAs: today) {
            return time
        }
        if let tomorrow = calendar.date(byAdding: .day, value: 1, to: today), calendar.isDate(date, inSameDayAs: tomorrow) {
            return String(localized: "tomorrow at \(time)")
        }
        return date.formatted(date: .abbreviated, time: .shortened)
    }
}
