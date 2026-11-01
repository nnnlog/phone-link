import AppKit
import Foundation
import PhoneLinkEngine
import Testing
@testable import PhoneLinkKit

@MainActor
struct PhoneAppsModelTests {
    @Test func readsTheListsOnceTheEngineStarts() {
        let api = FakePhoneApps()
        api.started = false
        let hub = EngineEventHub()
        let model = PhoneAppsModel(api: api, hub: hub, openURL: { _ in true })
        model.reload()
        #expect(model.state == nil && model.apps.isEmpty && !model.isViewVisible)

        api.started = true
        hub.postStarted()
        #expect(model.isViewVisible)
        #expect(model.apps.map(\.packageName) == ["com.a", "com.b"])
        #expect(model.favorites.map(\.packageName) == ["com.b"])
        #expect(model.recent.map(\.recentId) == [7])
    }

    @Test func theSearchFollowsTheText() {
        let api = FakePhoneApps()
        api.appList = Self.fiveApps
        let model = PhoneAppsModel(api: api, hub: EngineEventHub(), openURL: { _ in true })
        model.reload()
        #expect(!model.isSearching && model.searchResults.isEmpty)

        model.searchText = "be"
        #expect(model.isSearching)
        #expect(api.queries == ["be"])
        #expect(model.searchResults.map(\.packageName) == ["com.b"])

        model.searchText = "  "
        #expect(!model.isSearching && model.searchResults.isEmpty)
    }

    @Test func iconsDecodeOnceAndUndecodableOnesShowThePlaceholder() {
        let api = FakePhoneApps()
        api.icons = ["com.a": Self.png, "com.b": Data([1, 2, 3])]
        let hub = EngineEventHub()
        let model = PhoneAppsModel(api: api, hub: hub, openURL: { _ in true })
        model.reload()

        #expect(model.icon(for: "com.a") != nil)
        #expect(model.icon(for: "com.b") == nil, "not an image")
        #expect(model.icon(for: "com.unknown") == nil)

        _ = model.icon(for: "com.a")
        _ = model.icon(for: "com.b")
        #expect(api.iconReads == ["com.a", "com.b", "com.unknown"], "each read once")

        hub.post(.phoneApps(change: .appsChanged))
        _ = model.icon(for: "com.a")
        #expect(api.iconReads.count == 4, "a changed list reads icons again")
    }

    @Test func favoritesMoveByWritingTheWholeOrder() {
        let api = FakePhoneApps()
        api.favoriteList = ["com.a", "com.b", "com.c"]
        let model = PhoneAppsModel(api: api, hub: EngineEventHub(), openURL: { _ in true })
        model.reload()

        model.moveFavorite("com.c", to: 0)
        #expect(api.reorders == [["com.c", "com.a", "com.b"]])
        model.moveFavorite("com.c", to: 0)
        #expect(api.reorders.count == 1, "already there")
        model.moveFavorite("com.a", to: 10)
        #expect(api.reorders.last == ["com.c", "com.b", "com.a"])

        api.failWrites = true
        model.addFavorite(model.apps[0])
        #expect(model.failure?.title == "Couldn't Add to Favorites")
    }

    @Test func aTaskOpensItsDocumentOnThisMacOrInTheBrowser() {
        let api = FakePhoneApps()
        var opened: [URL] = []
        var opens = true
        let model = PhoneAppsModel(api: api, hub: EngineEventHub(), openURL: { url in
            opened.append(url)
            return opens
        })

        let task = RecentAppItem(
            recentId: 9, packageName: "com.office", name: "Report.docx", mainActivity: false, favorite: false, taskId: 4,
            webUri: "https://example.com/r.docx", desktopUri: "ms-word:ofe|u|https://example.com/r.docx", hasIcon: false
        )

        model.openOnMac(task)
        model.openInBrowser(task)
        // Foundation percent-encodes the characters a URL cannot hold (`|`); the scheme picks the app.
        #expect(opened.map(\.scheme) == ["ms-word", "https"])
        #expect(opened.last?.absoluteString == "https://example.com/r.docx")
        #expect(model.failure == nil)

        opens = false
        model.openInBrowser(task)
        #expect(model.failure?.title == "Couldn't Open the Document")
    }

    @Test func theAccountLeavingClearsEverything() {
        let api = FakePhoneApps()
        let hub = EngineEventHub()
        let model = PhoneAppsModel(api: api, hub: hub, openURL: { _ in true })
        model.reload()
        hub.post(.phoneUnlinked)
        #expect(model.state == nil && model.apps.isEmpty && model.recent.isEmpty)
    }

    static let fiveApps = [("com.a", "Alpha"), ("com.b", "Beta"), ("com.c", "Gamma"), ("com.d", "Delta"), ("com.e", "Epsilon")]

    @Test func theListsFirstShowOnceThePhoneIsNoLongerBehind() {
        let api = FakePhoneApps()
        api.syncing = true
        let hub = EngineEventHub()
        let model = PhoneAppsModel(api: api, hub: hub, openURL: { _ in true })
        model.reload()
        #expect(!model.listsShown, "behind: no lists yet")
        #expect(model.apps.count == 2, "read, not shown")

        api.syncing = false
        hub.post(.phoneApps(change: .stateChanged))
        #expect(model.listsShown)

        api.syncing = true
        hub.post(.phoneApps(change: .stateChanged))
        #expect(model.listsShown, "once shown, the lists follow every change")

        hub.post(.phoneChanged)
        #expect(!model.listsShown, "another phone waits again")
    }

    @Test func theSearchIsOfferedFromFiveAppsAndReturnOpensTheFirstFound() {
        let api = FakePhoneApps()
        let model = PhoneAppsModel(api: api, hub: EngineEventHub(), openURL: { _ in true })
        var launched: [PhoneAppLaunchTarget] = []
        model.launcher = { launched.append($0) }
        model.reload()

        #expect(!model.isSearchOffered)
        model.searchText = "al"
        #expect(!model.isSearching && api.queries.isEmpty, "no search under five apps")

        api.appList = Self.fiveApps
        model.reload()
        #expect(model.isSearchOffered && model.isSearching)
        #expect(model.searchResults.map(\.packageName) == ["com.a"])

        model.submitSearch()
        #expect(launched == [.app(packageName: "com.a")])
        #expect(model.searchText.isEmpty)

        model.searchText = "zzz"
        model.submitSearch()
        #expect(launched.count == 1, "nothing found opens nothing")
    }

    @Test func aRecentTaskWithADocumentOpensItWhenThePhoneHandsDocumentsOver() {
        let api = FakePhoneApps()
        var opened: [URL] = []
        let model = PhoneAppsModel(api: api, hub: EngineEventHub(), openURL: { opened.append($0); return true })
        var launched: [PhoneAppLaunchTarget] = []
        model.launcher = { launched.append($0) }

        let doc = RecentAppItem(
            recentId: 9, packageName: "com.office", name: "Report.docx", mainActivity: false, favorite: false, taskId: 4,
            webUri: "https://example.com/r.docx", desktopUri: "ms-word:ofe|u|https://example.com/r.docx", hasIcon: false
        )
        let web = RecentAppItem(
            recentId: 10, packageName: "com.office", name: "Sheet", mainActivity: false, favorite: false, taskId: 5,
            webUri: "https://example.com/s.xlsx", desktopUri: nil, hasIcon: false
        )

        model.reload()
        model.open(doc)
        #expect(launched == [.task(packageName: "com.office", taskId: 4)] && opened.isEmpty, "no handoff: the task")

        api.appHandoff = true
        model.reload()
        model.open(doc)
        model.open(web)
        model.open(api.recentList[0])
        #expect(opened.map(\.scheme) == ["ms-word", "https"])
        #expect(launched.last == .app(packageName: "com.a"), "nothing to hand over: the app")

        model.launch(doc)
        #expect(launched.last == .task(packageName: "com.office", taskId: 4), "Open always opens from the phone")
    }

    @Test func aRecentAppsMarkSaysHowItCarriesOver() {
        func entry(main: Bool, web: String?, desktop: String?) -> RecentAppItem {
            RecentAppItem(
                recentId: 1, packageName: "p", name: "n", mainActivity: main, favorite: false, taskId: 1,
                webUri: web, desktopUri: desktop, hasIcon: false
            )
        }

        #expect(PhoneAppsModel.handoffStamp(for: entry(main: true, web: nil, desktop: nil)) == nil)
        #expect(PhoneAppsModel.handoffStamp(for: entry(main: false, web: nil, desktop: nil)) == .task)
        #expect(PhoneAppsModel.handoffStamp(for: entry(main: false, web: "https://x", desktop: "")) == .browser)
        #expect(PhoneAppsModel.handoffStamp(for: entry(main: false, web: "https://x", desktop: "ms-word:x")) == .desktop)
    }

    @Test func dropsAddAndRemoveFavorites() {
        let api = FakePhoneApps()
        let model = PhoneAppsModel(api: api, hub: EngineEventHub(), openURL: { _ in true })
        model.reload()

        model.dropOnFavorites("com.a", at: 0)
        #expect(api.favoriteList == ["com.b", "com.a"], "an app joins last, wherever it is dropped")
        #expect(api.reorders.isEmpty)
        model.dropOnFavorites("com.a", at: 0)
        #expect(api.reorders == [["com.a", "com.b"]], "a favorite moves")

        model.dropOnFavorites("com.unknown", at: nil)
        model.dropOnAllApps("com.unknown")
        #expect(api.favoriteList == ["com.a", "com.b"])
        model.dropOnAllApps("com.b")
        #expect(api.favoriteList == ["com.a"])
    }

    @Test func onlyARecentAppThatIsTheAppItselfChangesTheFavorites() {
        let api = FakePhoneApps()
        api.favoriteList = []
        let model = PhoneAppsModel(api: api, hub: EngineEventHub(), openURL: { _ in true })
        model.reload()

        let task = RecentAppItem(
            recentId: 9, packageName: "com.b", name: "Doc", mainActivity: false, favorite: false, taskId: 4,
            webUri: nil, desktopUri: nil, hasIcon: false
        )

        model.addFavorite(task)
        #expect(api.favoriteList.isEmpty)
        model.addFavorite(model.recent[0])
        #expect(api.favoriteList == ["com.a"])

        var favorite = model.recent[0]
        favorite.favorite = true
        model.removeFavorite(favorite)
        #expect(api.favoriteList.isEmpty)
    }

    @Test func tilesCountTheNotificationsOfAppsWhoseNotificationsAreOn() {
        let model = PhoneAppsModel(api: FakePhoneApps(), hub: EngineEventHub(), openURL: { _ in true })
        var asked: [String] = []
        model.appNotificationsOn = { package in
            asked.append(package)
            return package != "com.off"
        }

        func note(_ key: String, _ package: String) -> PhoneNotification {
            PhoneNotification(
                key: key, packageName: package, appName: nil, title: nil, subtitle: nil, body: "", conversation: [],
                timeMs: 0, pinned: false, openTarget: .notifications, launchSupported: false, isReadOnlyGroupMessage: false,
                reply: nil, actions: []
            )
        }

        let counts = model.notificationCounts(in: [
            note("1", "com.a"), note("2", "COM.A"), note("3", "com.off"), note("4", "com.b"), note("5", ""),
        ])

        #expect(counts == ["com.a": 2, "com.b": 1])
        #expect(asked == ["com.a", "com.off", "com.b"], "each app asked once")
    }

    @Test func recentTileCountsAreTakenWhenTheRecentAppsAreBuilt() {
        let api = FakePhoneApps()
        let hub = EngineEventHub()
        let model = PhoneAppsModel(api: api, hub: hub, openURL: { _ in true })
        model.reload()
        var generation = model.recentGeneration

        // Other lists, favorites and the phone's state leave the recent apps as built.
        hub.post(.phoneApps(change: .appsChanged))
        model.addFavorite(model.apps[0])
        hub.post(.phoneApps(change: .stateChanged))
        #expect(model.recentGeneration == generation)

        // The phone's recent apps changing builds them again, as does another set showing.
        hub.post(.phoneApps(change: .recentChanged))
        #expect(model.recentGeneration == generation + 1)

        generation = model.recentGeneration
        api.recentList.append(RecentAppItem(
            recentId: 8, packageName: "com.b", name: "Notes", mainActivity: false, favorite: false, taskId: 80,
            webUri: nil, desktopUri: nil, hasIcon: false
        ))
        hub.post(.phoneApps(change: .appsChanged))
        #expect(model.recentGeneration == generation + 1)

        // The app itself shows its app's count; a task none.
        #expect(PhoneAppsModel.recentCounts(model.recent, ["com.a": 3, "com.b": 2]) == [7: 3, 8: 0])
    }

    @Test func recentAppsAreBuiltWhenTheListsFirstShow() {
        let api = FakePhoneApps()
        api.syncing = true
        let hub = EngineEventHub()
        let model = PhoneAppsModel(api: api, hub: hub, openURL: { _ in true })
        model.reload()
        let generation = model.recentGeneration

        api.syncing = false
        hub.post(.phoneApps(change: .stateChanged))
        #expect(model.listsShown)
        #expect(model.recentGeneration == generation + 1)
    }

    /// A 1 × 1 PNG.
    static let png: Data = {
        let image = NSImage(size: NSSize(width: 1, height: 1))
        image.lockFocus()
        NSColor.red.setFill()
        NSRect(x: 0, y: 0, width: 1, height: 1).fill()
        image.unlockFocus()

        let rep = NSBitmapImageRep(data: image.tiffRepresentation!)!
        return rep.representation(using: .png, properties: [:])!
    }()
}

@MainActor
struct PhoneAppsHostAdapterTests {
    @Test func namesSortInTheLocaleIgnoringCaseDiacriticsWidthAndKana() {
        let host = PhoneAppsHostAdapter(locale: { Locale(identifier: "en_US") }, defaultApplication: { _ in nil })
        let names = ["zebra", "Émail", "apple", "ＡＢＣ", "email"]
        let order = host.nameOrder(names: names).map { names[Int($0)] }
        #expect(order == ["ＡＢＣ", "apple", "Émail", "email", "zebra"], "equal names keep their order")

        let kana = ["カメラ", "かめら", "あ"]
        #expect(host.nameOrder(names: kana) == [2, 0, 1])
    }

    @Test func aUriOpensWhenLaunchServicesHasAnApp() {
        let host = PhoneAppsHostAdapter(
            locale: { Locale(identifier: "en_US") },
            defaultApplication: { $0.scheme == "ms-word" ? URL(fileURLWithPath: "/Applications/Word.app") : nil }
        )
        #expect(host.canOpenUri(uri: "ms-word:ofe|u|https://x/y.docx"))
        #expect(!host.canOpenUri(uri: "ms-excel:ofe|u|https://x/y.xlsx"))
        #expect(!host.canOpenUri(uri: "no scheme"))
    }
}

private final class FakePhoneApps: PhoneAppsApiProtocol, @unchecked Sendable {
    var started = true
    var failWrites = false
    var favoriteList = ["com.b"]
    var appList: [(String, String)] = [("com.a", "Alpha"), ("com.b", "Beta")]
    var syncing = false
    var appHandoff = false
    var recentList: [RecentAppItem] = [
        RecentAppItem(
            recentId: 7, packageName: "com.a", name: "Alpha", mainActivity: true, favorite: false, taskId: 70,
            webUri: nil, desktopUri: nil, hasIcon: false
        ),
    ]
    var icons: [String: Data] = [:]
    var iconReads: [String] = []
    var queries: [String] = []
    var reorders: [[String]] = []

    private func guardStarted() throws {
        guard started else { throw FeatureApiError.NotStarted }
    }

    private func item(_ package: String, _ name: String) -> PhoneAppItem {
        PhoneAppItem(packageName: package, name: name, version: "1", hasIcon: icons[package] != nil, favorite: favoriteList.contains(package))
    }

    func state() throws -> PhoneAppsState {
        try guardStarted()
        return PhoneAppsState(
            appsViewVisible: true, recentAppsSupported: true, appHandoffEnabled: appHandoff, hostedApps: false,
            syncing: syncing, upToDate: !syncing, lastSyncMs: nil
        )
    }

    func apps() throws -> [PhoneAppItem] {
        try guardStarted()
        return appList.map { item($0.0, $0.1) }
    }

    func search(query: String) throws -> [PhoneAppItem] {
        try guardStarted()
        queries.append(query)
        return try apps().filter { $0.name.lowercased().contains(query.lowercased()) }
    }

    func favorites() throws -> [PhoneAppItem] {
        try guardStarted()
        return favoriteList.map { item($0, $0) }
    }

    func recent() throws -> [RecentAppItem] {
        try guardStarted()
        return recentList
    }

    var storedNames: [String: String] = [:]
    func storedName(packageName: String) throws -> String? {
        try guardStarted()
        return storedNames[packageName]
    }

    func icon(packageName: String) throws -> Data? {
        try guardStarted()
        iconReads.append(packageName)
        return icons[packageName]
    }

    private func write() throws {
        try guardStarted()
        if failWrites { throw FeatureApiError.Store(message: "disk full") }
    }

    func addFavorite(packageName: String) throws {
        try write()
        favoriteList.append(packageName)
    }

    func removeFavorite(packageName: String) throws {
        try write()
        favoriteList.removeAll { $0 == packageName }
    }

    func reorderFavorites(packageNames: [String]) throws {
        try write()
        reorders.append(packageNames)
        favoriteList = packageNames
    }
}
