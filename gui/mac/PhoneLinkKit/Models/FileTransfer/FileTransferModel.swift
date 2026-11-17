import AppKit
import Observation
import os
import PhoneLinkEngine

private let log = AppLog(category: "file-transfer")

/// Sending files to the selected phone and receiving the files it shares: the phone's file settings,
/// the transfers running with it, a send from the send window (which reports its own progress and
/// outcome) or from outside it (a drop on the app or Open With, reported by notifications), and the
/// file or folder a received notification opens.
@MainActor
@Observable
public final class FileTransferModel {
    /// How a send from the window ended, for the window to show.
    public struct Outcome: Equatable, Sendable {
        public let title: String
        public let message: String
        public let succeeded: Bool
        /// The phone needs the files permission: the window offers to ask for it.
        public let needsPermission: Bool
    }

    /// `nil` until the engine has started (and while no phone is selected).
    public private(set) var settings: FileTransferSettings?
    /// The transfers running with the phone.
    public private(set) var transfers = FileTransfers(send: nil, receive: nil)
    /// The last window send's outcome; cleared when the next one starts.
    public private(set) var outcome: Outcome?
    /// The last change of the user's that failed, for an alert; the view clears it.
    public var failure: SettingsFailure?

    private let api: any FileTransferApiProtocol
    private let open: @MainActor (URL) -> Void
    private let reveal: @MainActor (URL) -> Void

    public convenience init(api: any FileTransferApiProtocol, hub: EngineEventHub) {
        self.init(
            api: api,
            hub: hub,
            open: { url in
                if !NSWorkspace.shared.open(url) {
                    log.error("received file or folder not opened; no app opened it")
                }
            },
            reveal: { NSWorkspace.shared.activateFileViewerSelecting([$0]) }
        )
    }

    init(
        api: any FileTransferApiProtocol,
        hub: EngineEventHub,
        open: @escaping @MainActor (URL) -> Void,
        reveal: @escaping @MainActor (URL) -> Void
    ) {
        self.api = api
        self.open = open
        self.reveal = reveal

        hub.onEvent { [weak self] event in
            self?.handle(event)
        }
        hub.onStart { [weak self] in
            self?.reload()
        }
    }

    // MARK: - Settings

    /// The phone is offered as a send target: the send window and the menus offer to send.
    public var canSend: Bool { settings?.offeredAsSendTarget == true }

    /// Why sending is not offered, for the send window; `nil` when it is.
    public var sendUnavailableReason: String? {
        guard let settings else { return String(localized: "Phone Link isn't connected to your phone yet.") }
        if !settings.shareCapable {
            return String(localized: "Your phone can't receive files from this Mac. Update Link to Windows on your phone.")
        }
        if !settings.offeredAsSendTarget {
            return String(localized: "Sending files to this phone is turned off in Phone Link Settings.")
        }
        return nil
    }

    /// The *Share* toggle shows.
    public var showsShareToggle: Bool { settings?.shareToggleShown == true }

    /// *Share*: the user may send files to the phone.
    public var shareEnabled: Bool {
        get { settings?.shareEnabled ?? false }
        set {
            do {
                try api.setShareEnabled(on: newValue)
            } catch {
                log.error("file sharing setting not changed: \(error: error)")
                failure = SettingsFailure(title: String(localized: "Couldn't Change the Setting"), message: UserText.describe(error))
            }

            reload()
        }
    }

    /// The received-files folder setting shows (once the phone has sent files).
    public var showsFolderSetting: Bool { settings?.folderSettingShown == true }

    /// Where files from the phone go.
    public var receivedFilesFolder: URL? {
        settings?.receivedFilesFolder.map { URL(fileURLWithPath: $0, isDirectory: true) }
    }

    public func changeReceivedFilesFolder(to folder: URL) {
        do {
            try api.setReceivedFilesFolder(path: folder.path)
        } catch {
            log.error("received-files folder not changed: \(error: error)")
            failure = SettingsFailure(title: String(localized: "Couldn't Change the Folder"), message: UserText.describe(error))
        }

        reload()
    }

    /// Shows the received-files folder in the Finder (it may not exist before the first file).
    public func openReceivedFilesFolder() {
        guard let folder = receivedFilesFolder else { return }
        if FileManager.default.fileExists(atPath: folder.path) {
            open(folder)
        } else {
            reveal(folder.deletingLastPathComponent())
        }
    }

    public func reload() {
        do {
            settings = try api.settings()
            transfers = try api.transfers()
        } catch let error as FeatureApiError where error == .NotStarted || error == .NoPhone {
            settings = nil
            transfers = FileTransfers(send: nil, receive: nil)
        } catch {
            log.error("file-transfer settings not read: \(error: error)")
        }
    }

    // MARK: - Sending

    /// A send runs to the phone.
    public var isSending: Bool { transfers.send != nil || sending != nil }

    /// The running send's progress (0–1) while files are being sent; `nil` before.
    public var sendFraction: Double? {
        guard let send = transfers.send, send.phase == .sending, send.bytesTotal > 0 else { return nil }
        return Double(send.bytesDone) / Double(send.bytesTotal)
    }

    /// What the running send is doing.
    public var sendStatus: String? {
        guard let send = transfers.send else { return sending == nil ? nil : String(localized: "Connecting to your phone…") }
        switch send.phase {
        case .connecting: return String(localized: "Connecting to your phone…")
        case .preparing: return String(localized: "Waiting for your phone…")
        case .sending:
            let done = ByteCountFormatter.string(fromByteCount: Int64(send.bytesDone), countStyle: .file)
            let total = ByteCountFormatter.string(fromByteCount: Int64(send.bytesTotal), countStyle: .file)
            return String(localized: "Sending \(done) of \(total)")
        }
    }

    /// The running receive's status line; `nil` while none runs.
    public var receiveStatus: String? {
        guard let receive = transfers.receive else { return nil }
        if receive.starting { return String(localized: "Your phone is sending files…") }
        let count = receive.fileNames.count
        return count == 1 ? String(localized: "Receiving \(receive.fileNames[0])") : String(localized: "Receiving \(count) files")
    }

    /// The window send under way, for tests to wait on.
    @ObservationIgnored private(set) var sending: Task<Void, Never>?

    /// Sends `files` from the send window: the window shows the progress and the outcome.
    public func sendFromWindow(_ files: [URL]) {
        guard !files.isEmpty, sending == nil else { return }
        outcome = nil
        let paths = files.map(\.path)

        sending = Task { [api] in
            let result: Outcome
            do {
                let sent = try await api.send(paths: paths, origin: .window)
                Self.logFailure(of: sent)
                result = Self.outcome(of: sent)
            } catch {
                log.error("file send not started: \(error: error)")
                result = Outcome(title: String(localized: "Couldn't Send"), message: UserText.describe(error), succeeded: false, needsPermission: false)
            }

            outcome = result
            sending = nil
            reload()
        }
    }

    /// Sends `files` handed to the app from outside its windows: notifications report the progress
    /// and the outcome.
    public func sendFromOutside(_ files: [URL]) {
        let paths = files.map(\.path)
        Task { [api] in
            do {
                // The engine's notifications show the outcome.
                Self.logFailure(of: try await api.send(paths: paths, origin: .shareMenu))
            } catch {
                log.error("file send not started: \(error: error)")
            }
        }
    }

    public func cancelSend() {
        do {
            try api.cancelSend()
        } catch {
            log.error("file send not cancelled: \(error: error)")
        }
    }

    /// Asks the phone for the files permission (it prompts).
    public func requestPermission() async {
        do {
            switch try await api.requestPermission() {
            case .granted:
                outcome = nil
            case .required:
                outcome = Outcome(
                    title: String(localized: "Check Your Phone"),
                    message: String(localized: "Allow Link to Windows to receive files on your phone, then send again."),
                    succeeded: false,
                    needsPermission: false
                )
            case let .failed(reason):
                log.error("file-transfer permission not requested: \(reason, privacy: .public)")
                failure = SettingsFailure(
                    title: String(localized: "Couldn't Ask Your Phone"),
                    message: String(localized: "Your phone didn't answer. Make sure it's nearby and connected, then try again.")
                )
            }
        } catch {
            log.error("file-transfer permission not requested: \(error: error)")
            failure = SettingsFailure(title: String(localized: "Couldn't Ask Your Phone"), message: UserText.describe(error))
        }
    }

    /// A send that did not deliver its files; a cancel is the user's.
    private static func logFailure(of result: FileSendResult) {
        guard case let .failed(failure, message) = result else { return }
        if case .cancelled = failure {
            log.debug("file send cancelled")
        } else {
            log.error("file send failed: \(failure, privacy: .public), \(message, privacy: .public)")
        }
    }

    static func outcome(of result: FileSendResult) -> Outcome {
        switch result {
        case let .sent(count):
            return Outcome(title: String(localized: "Sent"), message: String(localized: "\(count) files sent to your phone."), succeeded: true, needsPermission: false)
        case let .failed(failure, message):
            let text: String
            var needsPermission = false
            switch failure {
            case .cancelled: return Outcome(title: String(localized: "Canceled"), message: String(localized: "The transfer was canceled."), succeeded: false, needsPermission: false)
            case .turnedOff: text = String(localized: "Sending files to this phone is turned off in Phone Link Settings.")
            case .phoneNotSupported: text = String(localized: "Your phone can't receive files from this Mac.")
            case .noFile: text = String(localized: "Choose files to send; folders can't be sent.")
            case .anotherTransferInProgress: text = String(localized: "Another transfer with your phone is in progress. Try again when it ends.")
            case .permissionRequired:
                text = String(localized: "Your phone needs to allow Link to Windows to receive files.")
                needsPermission = true
            case .pcLocked: text = String(localized: "Unlock this Mac and try again.")
            case .metered: text = String(localized: "Your phone doesn't transfer files on a metered connection.")
            case .failed: text = UserText.capitalized(message) + "."
            }

            return Outcome(title: String(localized: "Couldn't Send"), message: text, succeeded: false, needsPermission: needsPermission)
        }
    }

    // MARK: - Engine events

    private func handle(_ event: EngineEvent) {
        switch event {
        case .fileTransfer(.changed), .resync(feature: .fileTransfer), .phoneChanged:
            reload()
        case let .fileTransfer(.openFile(path)):
            open(URL(fileURLWithPath: path))
        case let .fileTransfer(.revealInFinder(path)):
            // The received files' folder: it opens in a Finder window.
            open(URL(fileURLWithPath: path, isDirectory: true))
        case .accountRemoved, .phoneUnlinked:
            settings = nil
            transfers = FileTransfers(send: nil, receive: nil)
        default:
            break
        }
    }
}
