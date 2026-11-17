import AppKit
import Foundation
import Observation
import os
import PhoneLinkEngine

private let log = AppLog(category: "phone-apps")

/// What a launch asks the phone to show on this Mac.
public enum PhoneAppLaunchTarget: Equatable, Sendable {
    /// Open the app: an app of the list, or a recent app that is the app itself.
    case app(packageName: String)
    /// Bring a recent task over: the app opened on the task the phone has running.
    case task(packageName: String, taskId: Int64)
}

/// How a recent task carries over to this Mac, for the mark on its tile.
public enum RecentHandoffStamp: Equatable, Sendable {
    /// Its document opens in an app of this Mac.
    case desktop
    /// Its document opens in the browser.
    case browser
    /// The task continues where the phone left it.
    case task
}

/// The phone's apps for the Apps section: whether it shows, the recent apps, the favorites, every app
/// and the search, and the apps' icons — which the notification list shows too.
///
/// Every list is the engine's, read when the engine starts and on every phone-apps event. While the
/// phone's lists are behind (at most two minutes) the section shows no lists; once they are not, it
/// shows them and follows every change. Icons are decoded once per app and kept until the list
/// changes; an app without an icon, or with bytes that are no image, shows the placeholder.
@MainActor
@Observable
public final class PhoneAppsModel {
    /// `nil` before the engine starts and while no phone is selected.
    public private(set) var state: PhoneAppsState?
    public private(set) var apps: [PhoneAppItem] = []
    public private(set) var favorites: [PhoneAppItem] = []
    public private(set) var recent: [RecentAppItem] = []
    /// Changes whenever the recent apps are built again: when they are read for a phone, when the
    /// phone's recent apps change, and when another set of them shows. A recent tile's notification
    /// count is the one at that moment (``recentCounts(_:_:)``).
    public private(set) var recentGeneration = 0
    /// The search field's text; the results follow it.
    public var searchText = "" {
        didSet { reloadSearch() }
    }
    public private(set) var searchResults: [PhoneAppItem] = []
    /// The last action of the user's that failed, for an alert; the view clears it.
    public var failure: SettingsFailure?
    /// The lists have shown since the engine started or the phone changed: they first show once the
    /// phone's lists are not behind.
    public private(set) var listsShown = false
    /// Whether an app's notifications are on at all (in the list or as banners); an app whose
    /// notifications are off has no count on its tile. Set by the app.
    @ObservationIgnored public var appNotificationsOn: @MainActor (String) -> Bool = { _ in true }

    /// Changes whenever the decoded icons are dropped, so views showing an icon read it again.
    private var iconGeneration = 0
    @ObservationIgnored private var icons: [String: NSImage] = [:]
    /// Packages whose icon is known to be missing or undecodable.
    @ObservationIgnored private var iconMisses: Set<String> = []
    /// An icon read failed since the icons were dropped (logged once: each tile reads its own).
    @ObservationIgnored private var iconFailureLogged = false

    private let api: any PhoneAppsApiProtocol
    /// Opens a URL with the app Launch Services picks (injected so tests open nothing).
    private let openURL: @MainActor (URL) -> Bool

    public convenience init(api: any PhoneAppsApiProtocol, hub: EngineEventHub) {
        self.init(api: api, hub: hub, openURL: { NSWorkspace.shared.open($0) })
    }

    init(api: any PhoneAppsApiProtocol, hub: EngineEventHub, openURL: @escaping @MainActor (URL) -> Bool) {
        self.api = api
        self.openURL = openURL

        hub.onEvent { [weak self] event in
            self?.handle(event)
        }
        hub.onStart { [weak self] in
            // A start may have selected another phone.
            self?.listsShown = false
            self?.dropIcons()
            self?.reload()
        }
    }

    // MARK: - What the views show

    /// The Apps section shows: the phone opens its apps on this Mac.
    public var isViewVisible: Bool { state?.appsViewVisible == true }

    /// The phone is still sending its lists.
    public var isSyncing: Bool { state?.syncing == true }

    /// The search is offered once the phone lists at least five apps.
    public var isSearchOffered: Bool { apps.count >= Self.searchMinimumApps }

    public var isSearching: Bool {
        isSearchOffered && !searchText.trimmingCharacters(in: .whitespaces).isEmpty
    }

    /// The phone sends its recent apps: the Recent section shows, empty or not.
    public var showsRecent: Bool { state?.recentAppsSupported == true }

    static let searchMinimumApps = 5

    /// The mark of a recent app's tile: a document opens on this Mac or in the browser, or a task
    /// continues; `nil` for a recent app that is the app itself.
    public static func handoffStamp(for entry: RecentAppItem) -> RecentHandoffStamp? {
        if entry.desktopUri?.isEmpty == false { return .desktop }
        if entry.webUri?.isEmpty == false { return .browser }
        return entry.mainActivity ? nil : .task
    }

    /// The count on each app's tile: the notifications of `notifications` per package (ignoring
    /// case), leaving out apps whose notifications are off. Keyed by the lower-cased package.
    public func notificationCounts(in notifications: [PhoneNotification]) -> [String: Int] {
        var counts: [String: Int] = [:]
        var on: [String: Bool] = [:]
        for notification in notifications where !notification.packageName.isEmpty {
            let key = notification.packageName.lowercased()
            let appOn = on[key] ?? appNotificationsOn(notification.packageName)
            on[key] = appOn
            if appOn { counts[key, default: 0] += 1 }
        }
        return counts
    }

    /// The count on each recent tile, keyed by recent id: a recent app that is the app itself shows its
    /// app's count in `counts`, a task none. Taken when the recent apps are built again (see
    /// ``recentGeneration``) and kept until then, while the other tiles follow every change.
    public static func recentCounts(_ entries: [RecentAppItem], _ counts: [String: Int]) -> [Int64: Int] {
        var result: [Int64: Int] = [:]
        for entry in entries {
            result[entry.recentId] = entry.mainActivity ? (counts[entry.packageName.lowercased()] ?? 0) : 0
        }
        return result
    }

    /// The name the phone sent for `packageName`, as stored (excluded packages included): a mirroring
    /// window's title; `nil` when the phone never listed it.
    public func storedName(of packageName: String) -> String? {
        do {
            return try api.storedName(packageName: packageName)
        } catch let error as FeatureApiError where error == .NotStarted || error == .NoPhone {
            return nil
        } catch {
            log.notice("app name not read: \(error: error); the window shows none")
            return nil
        }
    }

    /// `packageName`'s icon; `nil` shows the placeholder.
    public func icon(for packageName: String) -> NSImage? {
        _ = iconGeneration
        if let image = icons[packageName] { return image }
        if iconMisses.contains(packageName) { return nil }

        var image: NSImage?
        do {
            if let data = try api.icon(packageName: packageName) {
                image = NSImage(data: data)
                if image == nil {
                    log.debug("app icon not decoded; its tile shows the placeholder")
                }
            }
        } catch let error as FeatureApiError where error == .NotStarted || error == .NoPhone {
            image = nil
        } catch {
            // Each tile reads its icon: the first failure since the icons were dropped is logged.
            if !iconFailureLogged {
                iconFailureLogged = true
                log.error("app icon not read: \(error: error); later failures are not logged until the icons reload")
            }
            image = nil
        }

        if let image {
            icons[packageName] = image
        } else {
            iconMisses.insert(packageName)
        }
        return image
    }

    // MARK: - Launching

    /// Opens a launch in a mirroring window (set by the app).
    @ObservationIgnored public var launcher: (@MainActor (PhoneAppLaunchTarget) -> Void)?

    /// The hand-off of every launch to screen mirroring.
    public func launch(_ target: PhoneAppLaunchTarget) {
        guard let launcher else {
            log.error("app launch dropped; screen mirroring is not set up")
            return
        }
        launcher(target)
    }

    public func launch(_ app: PhoneAppItem) {
        launch(.app(packageName: app.packageName))
    }

    /// A click on a recent app: when the phone hands documents over, a task's document opens on this
    /// Mac, else in the browser; otherwise the app (or its task) opens from the phone.
    public func open(_ entry: RecentAppItem) {
        if state?.appHandoffEnabled == true {
            if entry.desktopUri?.isEmpty == false { return openOnMac(entry) }
            if entry.webUri?.isEmpty == false { return openInBrowser(entry) }
        }
        launch(entry)
    }

    /// The search's Return: the first app found opens and the search is cleared.
    public func submitSearch() {
        guard isSearching, let first = searchResults.first else { return }
        launch(first)
        searchText = ""
    }

    /// "Open": a recent app opens from the phone — the app itself, or its task.
    public func launch(_ entry: RecentAppItem) {
        if entry.mainActivity {
            launch(.app(packageName: entry.packageName))
        } else {
            launch(.task(packageName: entry.packageName, taskId: entry.taskId))
        }
    }

    /// "Open on This Mac": the task's document in the app of this Mac that opens it.
    public func openOnMac(_ entry: RecentAppItem) {
        open(entry.desktopUri, title: String(localized: "Couldn't Open the Document"))
    }

    /// "Open in Browser": the task's document on the web.
    public func openInBrowser(_ entry: RecentAppItem) {
        open(entry.webUri, title: String(localized: "Couldn't Open the Document"))
    }

    private func open(_ uri: String?, title: String) {
        guard let uri, let url = URL(string: uri), url.scheme != nil else { return }
        if !openURL(url) {
            log.error("document not opened; no app on this Mac opened it")
            failure = SettingsFailure(title: title, message: String(localized: "No app on this Mac opened it."))
        }
    }

    // MARK: - Favorites

    public func addFavorite(_ app: PhoneAppItem) {
        addFavorite(packageName: app.packageName)
    }

    public func removeFavorite(_ app: PhoneAppItem) {
        removeFavorite(packageName: app.packageName)
    }

    /// A recent app that is the app itself joins the favorites (a task cannot).
    public func addFavorite(_ entry: RecentAppItem) {
        guard entry.mainActivity else { return }
        addFavorite(packageName: entry.packageName)
    }

    public func removeFavorite(_ entry: RecentAppItem) {
        guard entry.favorite else { return }
        removeFavorite(packageName: entry.packageName)
    }

    /// An app dropped on the favorites: a favorite moves to `index` (the end when `nil`), any other
    /// app of the list joins the favorites last.
    public func dropOnFavorites(_ packageName: String, at index: Int?) {
        if favorites.contains(where: { $0.packageName == packageName }) {
            if let index { moveFavorite(packageName, to: index) }
        } else if apps.contains(where: { $0.packageName == packageName }) {
            addFavorite(packageName: packageName)
        }
    }

    /// An app dropped on every app: a favorite leaves the favorites.
    public func dropOnAllApps(_ packageName: String) {
        guard favorites.contains(where: { $0.packageName == packageName }) else { return }
        removeFavorite(packageName: packageName)
    }

    private func addFavorite(packageName: String) {
        perform(String(localized: "Couldn't Add to Favorites"), "favorite not added") { try $0.addFavorite(packageName: packageName) }
    }

    private func removeFavorite(packageName: String) {
        perform(String(localized: "Couldn't Remove from Favorites"), "favorite not removed") { try $0.removeFavorite(packageName: packageName) }
    }

    /// Moves the favorite `packageName` to `index` of the favorites.
    public func moveFavorite(_ packageName: String, to index: Int) {
        var order = favorites.map(\.packageName)
        guard let from = order.firstIndex(of: packageName) else { return }
        let target = min(max(index, 0), order.count - 1)
        guard from != target else { return }

        order.remove(at: from)
        order.insert(packageName, at: target)

        perform(String(localized: "Couldn't Reorder Favorites"), "favorites not reordered") { try $0.reorderFavorites(packageNames: order) }
    }

    private func perform(_ title: String, _ subject: StaticString, _ action: (any PhoneAppsApiProtocol) throws -> Void) {
        do {
            try action(api)
        } catch {
            log.error("\(subject.description, privacy: .public): \(error: error)")
            failure = SettingsFailure(title: title, message: UserText.describe(error))
        }

        reloadLists()
    }

    // MARK: - Reading

    public func reload() {
        reload(rebuildingRecent: true)
    }

    /// Reads the state and the lists; the recent apps count as built again when `rebuildingRecent` or
    /// when the lists first show.
    private func reload(rebuildingRecent: Bool) {
        let wasShown = listsShown
        do {
            state = try api.state()
            if state?.syncing == false { listsShown = true }
        } catch let error as FeatureApiError where error == .NotStarted || error == .NoPhone {
            clear()
            return
        } catch {
            log.error("phone apps state not read: \(error: error)")
        }

        reloadLists(rebuildingRecent: rebuildingRecent || (listsShown && !wasShown))
    }

    /// Reads the lists again; the recent apps count as built again when `rebuildingRecent` or when
    /// another set of them shows.
    private func reloadLists(rebuildingRecent: Bool = false) {
        do {
            apps = try api.apps()
            favorites = try api.favorites()
            let read = try api.recent()
            if rebuildingRecent || read.map(\.recentId) != recent.map(\.recentId) {
                recentGeneration += 1
            }
            recent = read
        } catch let error as FeatureApiError where error == .NotStarted || error == .NoPhone {
            clear()
            return
        } catch {
            log.error("phone apps not read: \(error: error)")
        }

        reloadSearch()
    }

    private func reloadSearch() {
        guard isSearching else {
            searchResults = []
            return
        }

        do {
            searchResults = try api.search(query: searchText)
        } catch let error as FeatureApiError where error == .NotStarted || error == .NoPhone {
            searchResults = []
        } catch {
            searchResults = []
            log.error("phone apps not searched: \(error: error)")
        }
    }

    private func dropIcons() {
        icons = [:]
        iconMisses = []
        iconFailureLogged = false
        iconGeneration += 1
    }

    private func clear() {
        state = nil
        listsShown = false
        apps = []
        favorites = []
        recent = []
        recentGeneration += 1
        searchResults = []
        dropIcons()
    }

    private func handle(_ event: EngineEvent) {
        switch event {
        case .phoneApps(.appsChanged):
            dropIcons()
            reloadLists()
        case .phoneApps(.recentChanged):
            reloadLists(rebuildingRecent: true)
        case .phoneApps(.stateChanged):
            dropIcons()
            reload(rebuildingRecent: false)
        case .resync(feature: .phoneApps):
            dropIcons()
            reload()
        case .phoneChanged:
            listsShown = false
            dropIcons()
            reload()
        case .accountRemoved, .phoneUnlinked:
            clear()
        default:
            break
        }
    }
}
