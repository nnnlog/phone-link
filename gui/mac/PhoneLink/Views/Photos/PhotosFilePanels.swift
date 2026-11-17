import AppKit
import PhoneLinkEngine
import PhoneLinkKit
import UniformTypeIdentifiers

/// Where a share picker points: the pointer's place in the window when the user chose Share, which
/// is the toolbar button or the context menu's photo.
struct PhotosAnchor {
    weak var window: NSWindow?
    /// In the window's coordinates.
    let location: NSPoint

    /// `nil` when the grid is in no window.
    @MainActor
    static func pointer(in window: NSWindow?) -> PhotosAnchor? {
        window.map { PhotosAnchor(window: $0, location: $0.convertPoint(fromScreen: NSEvent.mouseLocation)) }
    }
}

/// The AppKit panels of the photo file actions: each first starts getting the full image, so a
/// panel the user fills in meanwhile does not wait for it.
@MainActor
enum PhotosFilePanels {
    /// The picker on screen; kept until the next one replaces it.
    private static var sharingPicker: NSSharingServicePicker?

    /// Share…: the share picker with every photo once their full images are here, unless the user
    /// moved on meanwhile or one of them failed (after its alert).
    static func share(_ ids: [Int64], model: PhotosModel, anchor: PhotosAnchor?) {
        guard let anchor, !ids.isEmpty else { return }

        var pendings: [PhotosPendingFile] = []
        for id in ids {
            guard let pending = model.startFile(id, for: .share) else { return }
            pendings.append(pending)
        }

        Task {
            var urls: [URL] = []
            for pending in pendings {
                guard let file = await model.fileToShare(from: pending) else { return }
                urls.append(file.url)
            }

            guard let window = anchor.window, window.isVisible, let view = window.contentView else { return }
            let point = view.convert(anchor.location, from: nil)
            let picker = NSSharingServicePicker(items: urls)
            sharingPicker = picker
            picker.show(
                relativeTo: NSRect(x: point.x - 1, y: point.y - 1, width: 2, height: 2),
                of: view,
                preferredEdge: view.isFlipped ? .maxY : .minY
            )
        }
    }

    /// Save of the selection: the save panel for one photo, a folder for several.
    static func save(_ ids: [Int64], model: PhotosModel, window: NSWindow?) {
        if ids.count == 1 {
            saveAs(ids[0], model: model, window: window)
        } else {
            saveToFolder(ids, model: model, window: window)
        }
    }

    /// Save N Photos… and Save All Photos…: the folder is chosen once, then each photo is fetched
    /// and written there in turn under its own name.
    static func saveToFolder(_ ids: [Int64], model: PhotosModel, window: NSWindow?) {
        guard let window, !ids.isEmpty else { return }

        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.canCreateDirectories = true
        panel.allowsMultipleSelection = false
        panel.prompt = String(localized: "Save")
        panel.message = String(localized: "Choose a folder to save \(ids.count) photos in.")

        Task {
            guard await panel.beginSheetModal(for: window) == .OK, let folder = panel.url else { return }
            model.save(ids, toFolder: folder)
        }
    }

    /// Save As…: the save panel with the photo's name; the file is written once it is here.
    static func saveAs(_ id: Int64, model: PhotosModel, window: NSWindow?) {
        guard let window, let pending = model.startFile(id, for: .saveAs) else { return }

        let panel = NSSavePanel()
        panel.nameFieldStringValue = pending.fileName
        if !pending.contentType.isDynamic {
            panel.allowedContentTypes = [pending.contentType]
        }
        panel.canCreateDirectories = true
        panel.isExtensionHidden = false

        Task {
            guard await panel.beginSheetModal(for: window) == .OK, let destination = panel.url else { return }
            model.save(pending, to: destination)
        }
    }

    /// Open With › Other…: an app from Applications opens the photo.
    static func chooseApp(_ id: Int64, model: PhotosModel, window: NSWindow?) {
        guard let window, let pending = model.startFile(id, for: .openWith) else { return }

        let panel = NSOpenPanel()
        panel.directoryURL = URL(fileURLWithPath: "/Applications", isDirectory: true)
        panel.allowedContentTypes = [.application]
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        panel.prompt = String(localized: "Open")
        panel.message = String(localized: "Choose an app to open “\(pending.fileName)”.")

        Task {
            guard await panel.beginSheetModal(for: window) == .OK, let app = panel.url else { return }
            model.open(pending, with: app)
        }
    }
}

/// The apps that open a photo's type, as Finder lists them in Open With: the default app first.
@MainActor
enum PhotosOpenWithApps {
    struct App: Identifiable {
        let url: URL
        let name: String
        let icon: NSImage
        let isDefault: Bool

        var id: URL { url }
    }

    /// Launch Services is asked once per type while the gallery is on screen: menus are built often.
    private static var known: [UTType: [App]] = [:]

    static func apps(for type: UTType) -> [App] {
        if let apps = known[type] {
            return apps
        }

        let workspace = NSWorkspace.shared
        let defaultApp = workspace.urlForApplication(toOpen: type)
        func app(_ url: URL) -> App {
            let icon = workspace.icon(forFile: url.path)
            icon.size = NSSize(width: 16, height: 16)
            return App(
                url: url,
                name: FileManager.default.displayName(atPath: url.path),
                icon: icon,
                isDefault: url == defaultApp
            )
        }

        let others = workspace.urlsForApplications(toOpen: type)
            .filter { $0 != defaultApp }
            .map(app)
            .sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
        let apps = (defaultApp.map { [app($0)] } ?? []) + others
        known[type] = apps
        return apps
    }

    /// Apps may have come or gone since the gallery was last shown.
    static func forget() {
        known = [:]
    }
}
