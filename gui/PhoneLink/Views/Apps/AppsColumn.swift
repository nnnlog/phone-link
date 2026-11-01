import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// The Apps section's column: the phone's recent apps, favorites and every app as grids, and — once
/// the phone lists five apps — a search over them. A click opens the app from the phone in a window on
/// this Mac; a recent task's document opens on this Mac or in the browser instead when the phone hands
/// documents over. Apps are dragged onto the favorites to add them and back onto every app to remove
/// them.
struct AppsColumn: View {
    @Environment(PhoneAppsModel.self) private var model
    @Environment(EngineController.self) private var controller
    @Environment(ScreenMirroringModel.self) private var mirroring
    @Environment(NotificationsModel.self) private var notifications
    /// The recent tiles' counts, taken when the recent apps were last built.
    @State private var recentCounts: [Int64: Int] = [:]

    var body: some View {
        content
            .onChange(of: model.recentGeneration, initial: true) {
                recentCounts = PhoneAppsModel.recentCounts(model.recent, counts)
            }
            .navigationTitle("Apps")
            .modifier(AppsSearch(model: model))
            .alert(
                model.failure?.title ?? "",
                isPresented: Binding(get: { model.failure != nil }, set: { if !$0 { model.failure = nil } }),
                presenting: model.failure
            ) { _ in
                Button("OK") {}
            } message: { failure in
                Text(failure.message)
            }
    }

    private var phoneName: String { controller.phone?.name ?? String(localized: "Your phone") }

    /// The phone's screen and apps may open on this Mac (the Screen Mirroring setting).
    private var appsAllowed: Bool { mirroring.status?.settings.allowApps ?? true }

    @ViewBuilder private var content: some View {
        if !model.isViewVisible {
            ContentUnavailableView(
                "Apps Aren't Available",
                systemImage: "square.grid.2x2",
                description: Text("\(phoneName) doesn't open its apps on this Mac.")
            )
        } else if !appsAllowed {
            ContentUnavailableView {
                Label("Apps Are Turned Off", systemImage: "square.grid.2x2")
            } description: {
                Text("Turn them on to open \(phoneName)'s screen and apps on this Mac.")
            } actions: {
                Button("Turn On") {
                    mirroring.updateSettings { $0.allowApps = true }
                }
                .buttonStyle(.borderedProminent)
            }
        } else if !model.listsShown {
            ProgressView("Getting apps from \(phoneName)…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if model.isSearching {
            if model.searchResults.isEmpty {
                ContentUnavailableView.search(text: model.searchText)
            } else {
                ScrollView {
                    AppGrid(apps: model.searchResults, counts: counts)
                        .padding()
                }
            }
        } else {
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    if model.showsRecent {
                        AppsGroup(title: "Recent") {
                            if model.recent.isEmpty {
                                GroupHint("Apps you use on \(phoneName) appear here.")
                            } else {
                                RecentGrid(entries: model.recent, counts: recentCounts)
                            }
                        }
                    }
                    AppsGroup(title: "Favorites") {
                        if model.favorites.isEmpty {
                            GroupHint("Drag apps here to keep them at hand.")
                        } else {
                            FavoritesGrid(favorites: model.favorites, counts: counts)
                        }
                    }
                    .dropDestination(for: String.self) { packages, _ in
                        guard let package = packages.first else { return false }
                        model.dropOnFavorites(package, at: nil)
                        return true
                    }
                    AppsGroup(title: "All Apps") {
                        if model.apps.isEmpty {
                            GroupHint("Apps from \(phoneName) appear here.")
                        } else {
                            AppGrid(apps: model.apps, counts: counts)
                        }
                    }
                    .dropDestination(for: String.self) { packages, _ in
                        guard let package = packages.first else { return false }
                        model.dropOnAllApps(package)
                        return true
                    }
                }
                .padding()
            }
        }
    }

    /// Each app's notification count, keyed by its lower-cased package.
    private var counts: [String: Int] {
        model.notificationCounts(in: notifications.notifications)
    }
}

/// The search field, offered once the phone lists five apps; Return opens the first app found.
private struct AppsSearch: ViewModifier {
    @Bindable var model: PhoneAppsModel

    func body(content: Content) -> some View {
        if model.isSearchOffered {
            content
                .searchable(text: $model.searchText, placement: .toolbar, prompt: "Search Apps")
                .onSubmit(of: .search) { model.submitSearch() }
        } else {
            content
        }
    }
}

/// The grids' columns.
private let tileColumns = [GridItem(.adaptive(minimum: 88, maximum: 112), spacing: 12, alignment: .top)]

/// A titled group of tiles.
private struct AppsGroup<Content: View>: View {
    let title: LocalizedStringKey
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(title)
                .font(.headline)
            content
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
    }
}

/// The text of a group with nothing in it.
private struct GroupHint: View {
    let text: LocalizedStringKey

    init(_ text: LocalizedStringKey) {
        self.text = text
    }

    var body: some View {
        Text(text)
            .font(.callout)
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, minHeight: 48, alignment: .leading)
    }
}

/// An icon over its name, with the app's notification count and, for a recent task, how it carries
/// over to this Mac.
private struct Tile: View {
    let packageName: String
    let name: String
    let count: Int
    var stamp: RecentHandoffStamp?

    var body: some View {
        VStack(spacing: 6) {
            PhoneAppIcon(packageName: packageName)
                .overlay(alignment: .topTrailing) {
                    if count > 0 {
                        Text(count, format: .number)
                            .font(.caption2.bold())
                            .monospacedDigit()
                            .foregroundStyle(.white)
                            .padding(.horizontal, 5)
                            .frame(minWidth: 18, minHeight: 18)
                            .background(.red, in: Capsule())
                            .offset(x: 6, y: -6)
                    }
                }
                .overlay(alignment: .bottomTrailing) {
                    if let stamp {
                        Image(systemName: Self.symbol(stamp))
                            .font(.system(size: 10, weight: .semibold))
                            .foregroundStyle(.secondary)
                            .frame(width: 18, height: 18)
                            .background(.regularMaterial, in: Circle())
                            .offset(x: 6, y: 6)
                    }
                }
            Text(name)
                .font(.callout)
                .lineLimit(2)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 6)
        .contentShape(Rectangle())
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(count > 0 ? String(localized: "\(name), \(count) notifications") : name)
    }

    private static func symbol(_ stamp: RecentHandoffStamp) -> String {
        switch stamp {
        case .desktop: "desktopcomputer"
        case .browser: "globe"
        case .task: "arrow.up.forward.app"
        }
    }
}

private func notificationCount(_ counts: [String: Int], _ packageName: String) -> Int {
    counts[packageName.lowercased()] ?? 0
}

/// Apps, each opening on a click and draggable onto the favorites; the context menu adds or removes
/// it as a favorite.
private struct AppGrid: View {
    let apps: [PhoneAppItem]
    let counts: [String: Int]
    @Environment(PhoneAppsModel.self) private var model

    var body: some View {
        LazyVGrid(columns: tileColumns, spacing: 16) {
            ForEach(apps, id: \.packageName) { app in
                Button { model.launch(app) } label: {
                    Tile(packageName: app.packageName, name: app.name, count: notificationCount(counts, app.packageName))
                }
                .buttonStyle(.plain)
                .help(app.name)
                .draggable(app.packageName)
                .contextMenu {
                    Button("Open") { model.launch(app) }
                    Divider()
                    if app.favorite {
                        Button("Remove from Favorites") { model.removeFavorite(app) }
                    } else {
                        Button("Add to Favorites") { model.addFavorite(app) }
                    }
                }
            }
        }
    }
}

/// The favorites in their order: drag one onto another to move it there (or onto every app to remove
/// it), drop another app on them to add it, or move one from its context menu.
private struct FavoritesGrid: View {
    let favorites: [PhoneAppItem]
    let counts: [String: Int]
    @Environment(PhoneAppsModel.self) private var model

    var body: some View {
        LazyVGrid(columns: tileColumns, spacing: 16) {
            ForEach(Array(favorites.enumerated()), id: \.element.packageName) { index, app in
                Button { model.launch(app) } label: {
                    Tile(packageName: app.packageName, name: app.name, count: notificationCount(counts, app.packageName))
                }
                .buttonStyle(.plain)
                .help(app.name)
                .draggable(app.packageName)
                .dropDestination(for: String.self) { packages, _ in
                    guard let dropped = packages.first, dropped != app.packageName else { return false }
                    model.dropOnFavorites(dropped, at: index)
                    return true
                }
                .contextMenu {
                    Button("Open") { model.launch(app) }
                    Divider()
                    Button("Move Left") { model.moveFavorite(app.packageName, to: index - 1) }
                        .disabled(index == 0)
                    Button("Move Right") { model.moveFavorite(app.packageName, to: index + 1) }
                        .disabled(index == favorites.count - 1)
                    Divider()
                    Button("Remove from Favorites") { model.removeFavorite(app) }
                }
            }
        }
    }
}

/// The recent apps: a click opens the app, or a task where the phone left it — or, when the phone
/// hands documents over, a task's document on this Mac or in the browser. A recent app that is the
/// app itself can be added to or removed from the favorites. `counts` are the tiles' notification
/// counts by recent id, as they were when the recent apps were last built.
private struct RecentGrid: View {
    let entries: [RecentAppItem]
    let counts: [Int64: Int]
    @Environment(PhoneAppsModel.self) private var model

    var body: some View {
        LazyVGrid(columns: tileColumns, spacing: 16) {
            ForEach(entries, id: \.recentId) { entry in
                Button { model.open(entry) } label: {
                    Tile(
                        packageName: entry.packageName,
                        name: entry.name,
                        count: counts[entry.recentId] ?? 0,
                        stamp: PhoneAppsModel.handoffStamp(for: entry)
                    )
                }
                .buttonStyle(.plain)
                .help(entry.name)
                .contextMenu {
                    Button("Open") { model.launch(entry) }
                    if entry.desktopUri?.isEmpty == false || entry.webUri?.isEmpty == false {
                        Divider()
                    }
                    if entry.desktopUri?.isEmpty == false {
                        Button("Open on This Mac") { model.openOnMac(entry) }
                    }
                    if entry.webUri?.isEmpty == false {
                        Button("Open in Browser") { model.openInBrowser(entry) }
                    }
                    if entry.mainActivity {
                        Divider()
                        if entry.favorite {
                            Button("Remove from Favorites") { model.removeFavorite(entry) }
                        } else {
                            Button("Add to Favorites") { model.addFavorite(entry) }
                        }
                    }
                }
            }
        }
    }
}
