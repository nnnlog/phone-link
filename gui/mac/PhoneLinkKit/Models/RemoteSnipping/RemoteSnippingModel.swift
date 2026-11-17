import AppKit
import Observation
import os
import PhoneLinkEngine
import UniformTypeIdentifiers

private let log = AppLog(category: "remote-snipping")

/// New-photo notifications sent by the phone when a photo is taken: their settings for the selected
/// phone, and the image a clicked notification opens.
///
/// A click hands the engine's bytes — the photo, or its thumbnail when the photo did not come — to the
/// default app for their image type (Preview unless the user chose another, whose Markup tools edit
/// it; Phone Link on Windows opens Snipping Tool). Only bytes ImageIO reads as an image type other
/// than SVG are opened, from a file named after that type, because they come from the phone.
@MainActor
@Observable
public final class RemoteSnippingModel {
    /// `nil` until the engine has started (and while no phone is selected).
    public private(set) var settings: RemoteSnippingSettings?
    /// The last change of the user's that failed, for an alert; the view clears it.
    public var failure: SettingsFailure?

    private let api: any RemoteSnippingApiProtocol
    private let files: RemoteSnippingFiles
    /// Opens a file with its default app (injected so tests never launch one).
    private let openFile: @MainActor (URL) async throws -> Void

    public convenience init(api: any RemoteSnippingApiProtocol, hub: EngineEventHub) {
        self.init(
            api: api,
            hub: hub,
            temporaryDirectory: FileManager.default.temporaryDirectory,
            openFile: RemoteSnippingModel.openInWorkspace
        )
    }

    init(
        api: any RemoteSnippingApiProtocol,
        hub: EngineEventHub,
        temporaryDirectory: URL,
        openFile: @escaping @MainActor (URL) async throws -> Void
    ) {
        self.api = api
        files = RemoteSnippingFiles(base: temporaryDirectory)
        self.openFile = openFile

        hub.onEvent { [weak self] event in
            self?.handle(event)
        }
        hub.onStart { [weak self] in
            self?.reload()
        }

        _ = NotificationCenter.default.addObserver(
            forName: NSApplication.willTerminateNotification,
            object: nil,
            queue: .main
        ) { [files] _ in
            files.removeAll()
        }
    }

    /// "Get new photo notifications" shows: the phone supports them.
    public var showsToggle: Bool { settings?.applicable == true }

    /// "Get new photo notifications".
    public var getsNotifications: Bool {
        get { settings?.enabled ?? false }
        set {
            do {
                try api.setEnabled(on: newValue)
            } catch {
                log.error("new-photo notification setting not changed: \(error: error)")
                failure = SettingsFailure(title: String(localized: "Couldn't Change the Setting"), message: UserText.describe(error))
            }

            reload()
        }
    }

    /// The "Allow permissions" link shows.
    public var needsPermission: Bool { settings?.needsPermission == true }

    /// The permission request runs: the link reads "Finishing up".
    public var requestingPermission: Bool { settings?.requestingPermission == true }

    /// The photos settings' "Notify me about new photos" applies: these notifications do not stand in
    /// for it.
    public var photosNotificationsApply: Bool { settings?.coversPhotosNotifications != true }

    /// The "Allow permissions" link: asks the phone; the phone shows its prompt.
    public func requestPermission() async {
        do {
            let outcome = try await api.requestPermission()
            if case let .failed(reason) = outcome {
                log.error("new-photo notification permission not requested: \(reason, privacy: .public)")
                failure = SettingsFailure(
                    title: String(localized: "Couldn't Ask Your Phone"),
                    message: String(localized: "Your phone didn't answer. Make sure it's nearby and connected, then try again.")
                )
            }
        } catch {
            log.error("new-photo notification permission not requested: \(error: error)")
            failure = SettingsFailure(title: String(localized: "Couldn't Ask Your Phone"), message: UserText.describe(error))
        }

        reload()
    }

    public func reload() {
        do {
            settings = try api.settings()
        } catch let error as FeatureApiError where error == .NotStarted || error == .NoPhone {
            settings = nil
        } catch {
            log.error("new-photo notification settings not read: \(error: error)")
        }
    }

    // MARK: - Engine events

    /// The open of the last clicked notification, for tests to wait on.
    @ObservationIgnored private(set) var opening: Task<Void, Never>?

    private func handle(_ event: EngineEvent) {
        switch event {
        case .remoteSnipping(.settingsChanged), .resync(feature: .remoteSnipping), .phoneChanged:
            reload()
        case let .remoteSnipping(.openImage(mediaId, bytes)):
            open(bytes, mediaId: mediaId)
        case .accountRemoved, .phoneUnlinked:
            settings = nil
        default:
            break
        }
    }

    private func open(_ bytes: Data, mediaId: Int64) {
        let job: @Sendable () throws -> URL? = { [files] in try files.write(bytes, mediaId: mediaId) }
        opening = Task { [files, openFile] in
            do {
                guard let file = try await files.perform(job) else {
                    log.error("clicked new photo \(mediaId, privacy: .private) not opened; its bytes are no image ImageIO reads")
                    return
                }
                try await openFile(file)
            } catch {
                log.error("clicked new photo not opened: \(error: error)")
            }
        }
    }

    static func openInWorkspace(_ file: URL) async throws {
        guard NSWorkspace.shared.open(file) else { throw RemoteSnippingNoAppToOpen() }
    }
}

/// No app opened the photo.
struct RemoteSnippingNoAppToOpen: LocalizedError {
    var errorDescription: String? { String(localized: "No app could open the photo.") }
}

/// The image files a clicked notification hands to an app: one folder of this run under the temporary
/// directory, removed when the app quits; a launch removes the folders earlier runs left. Disk work
/// runs on one serial queue off the main thread.
final class RemoteSnippingFiles: Sendable {
    let folder: URL
    private let queue = DispatchQueue(label: "\(PhoneLinkIdentity.bundleID).remotesnipping.files", qos: .userInitiated)

    init(base: URL) {
        let root = base.appendingPathComponent("\(PhoneLinkIdentity.bundleID).remotesnipping", isDirectory: true)
        let fileManager = FileManager.default
        do {
            for earlier in try fileManager.contentsOfDirectory(at: root, includingPropertiesForKeys: nil) {
                try fileManager.removeItem(at: earlier)
            }
        } catch let error as CocoaError where error.code == .fileReadNoSuchFile {
            // No run has written a file yet.
        } catch {
            log.notice("earlier runs' new-photo files not removed: \(error: error)")
        }

        folder = root.appendingPathComponent(UUID().uuidString, isDirectory: true)
    }

    func perform<T: Sendable>(_ work: @escaping @Sendable () throws -> T) async throws -> T {
        try await withCheckedThrowingContinuation { continuation in
            queue.async {
                continuation.resume(with: Result(catching: work))
            }
        }
    }

    /// The file name of `data`: the media id with the extension of the image type ImageIO detects;
    /// `nil` when the bytes are no image or are SVG.
    static func fileName(of data: Data, mediaId: Int64) -> String? {
        guard let type = PhotosImageDecoder.type(of: data), PhotosModel.isPhoto(type),
              let ext = type.preferredFilenameExtension
        else {
            return nil
        }
        return "\(mediaId).\(ext)"
    }

    /// Writes `data` for the app to open; `nil`, writing nothing, when it is no image.
    func write(_ data: Data, mediaId: Int64) throws -> URL? {
        guard let name = Self.fileName(of: data, mediaId: mediaId) else { return nil }
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let url = folder.appendingPathComponent(name)
        try data.write(to: url, options: .atomic)
        return url
    }

    /// Removes this run's files (the app quits), waiting for the work under way.
    func removeAll() {
        queue.sync {
            do {
                try FileManager.default.removeItem(at: folder)
            } catch let error as CocoaError where error.code == .fileNoSuchFile {
                // Nothing was written.
            } catch {
                log.notice("this run's new-photo files not removed: \(error: error)")
            }
        }
    }
}
