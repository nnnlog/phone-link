import Foundation
import Testing
import PhoneLinkEngine
@testable import PhoneLinkKit

@MainActor
@Suite struct FileTransferModelTests {
    private let api = FileTransferApiFake()
    private let hub = EngineEventHub()
    private let opened = OpenedURLs()
    private let model: FileTransferModel

    init() {
        let opened = opened
        model = FileTransferModel(
            api: api,
            hub: hub,
            open: { opened.opened.append($0) },
            reveal: { opened.revealed.append($0) }
        )
    }

    private static func settings(
        capable: Bool = true,
        offered: Bool = true,
        share: Bool = true,
        folderShown: Bool = false
    ) -> FileTransferSettings {
        FileTransferSettings(
            shareCapable: capable,
            shareToggleShown: capable,
            shareEnabled: share,
            offeredAsSendTarget: offered,
            folderSettingShown: folderShown,
            receivedFilesFolder: "/Users/u/Downloads/Mobile Devices"
        )
    }

    @Test func theSettingsLoadOnStartAndOnEveryChange() {
        hub.postStarted()
        #expect(model.settings == nil)
        #expect(!model.canSend)
        #expect(model.sendUnavailableReason != nil)

        api.update { $0.settings = .success(Self.settings()) }
        hub.postStarted()
        #expect(model.canSend && model.showsShareToggle && model.shareEnabled && !model.showsFolderSetting)
        #expect(model.sendUnavailableReason == nil)

        api.update { $0.settings = .success(Self.settings(capable: false, offered: false)) }
        hub.post(.fileTransfer(change: .changed))
        #expect(!model.canSend && !model.showsShareToggle)
        #expect(model.sendUnavailableReason?.contains("can't receive") == true)

        api.update { $0.settings = .success(Self.settings(offered: false, share: false, folderShown: true)) }
        hub.post(.resync(feature: .fileTransfer))
        #expect(model.sendUnavailableReason?.contains("turned off") == true)
        #expect(model.showsFolderSetting)
        #expect(model.receivedFilesFolder?.path == "/Users/u/Downloads/Mobile Devices")

        hub.post(.accountRemoved)
        #expect(model.settings == nil)
    }

    @Test func theToggleAndTheFolderWriteAndReadBack() {
        api.update { $0.settings = .success(Self.settings()) }
        hub.postStarted()

        model.shareEnabled = false
        model.changeReceivedFilesFolder(to: URL(fileURLWithPath: "/tmp/received", isDirectory: true))
        #expect(api.shareWrites == [false])
        #expect(api.folderWrites == ["/tmp/received"])

        api.update { $0.setError = .Store(message: "disk") }
        model.shareEnabled = true
        #expect(model.failure?.title == "Couldn't Change the Setting")
    }

    @Test func aWindowSendReportsItsOutcome() async {
        api.update {
            $0.settings = .success(Self.settings())
            $0.sendResult = .success(.sent(count: 2))
        }
        hub.postStarted()
        model.sendFromWindow([URL(fileURLWithPath: "/tmp/a"), URL(fileURLWithPath: "/tmp/b")])
        await model.sending?.value
        #expect(api.sends.map(\.paths) == [["/tmp/a", "/tmp/b"]])
        #expect(api.sends.map(\.origin) == [.window])
        #expect(model.outcome == FileTransferModel.Outcome(title: "Sent", message: "2 files sent to your phone.", succeeded: true, needsPermission: false))

        api.update { $0.sendResult = .success(.failed(failure: .permissionRequired, message: "x")) }
        model.sendFromWindow([URL(fileURLWithPath: "/tmp/a")])
        await model.sending?.value
        #expect(model.outcome?.needsPermission == true)

        api.update { $0.sendResult = .success(.failed(failure: .failed, message: "the transfer channel closed")) }
        model.sendFromWindow([URL(fileURLWithPath: "/tmp/a")])
        await model.sending?.value
        #expect(model.outcome?.message == "The transfer channel closed.")
    }

    @Test func theProgressAndTheStatusFollowTheTransfers() {
        api.update {
            $0.settings = .success(Self.settings())
            $0.transfers = FileTransfers(
                send: FileSendProgress(phase: .sending, fileNames: ["a"], bytesDone: 25, bytesTotal: 100),
                receive: FileReceiveProgress(starting: false, fileNames: ["x.jpg", "y.jpg"], bytesDone: 0, bytesTotal: 10)
            )
        }
        hub.postStarted()
        #expect(model.isSending)
        #expect(model.sendFraction == 0.25)
        #expect(model.receiveStatus == "Receiving 2 files")

        api.update {
            $0.transfers = FileTransfers(
                send: FileSendProgress(phase: .preparing, fileNames: ["a"], bytesDone: 0, bytesTotal: 100),
                receive: nil
            )
        }
        hub.post(.fileTransfer(change: .changed))
        #expect(model.sendFraction == nil)
        #expect(model.sendStatus == "Waiting for your phone…")
        #expect(model.receiveStatus == nil)
    }

    @Test func aReceivedNotificationOpensTheFileOrTheFolder() {
        hub.post(.fileTransfer(change: .openFile(path: "/d/a.jpg")))
        hub.post(.fileTransfer(change: .revealInFinder(path: "/d")))
        #expect(opened.opened.map(\.path) == ["/d/a.jpg", "/d"])
    }
}

@MainActor
private final class OpenedURLs {
    var opened: [URL] = []
    var revealed: [URL] = []
}

private final class FileTransferApiFake: FileTransferApiProtocol, @unchecked Sendable {
    struct Store {
        var settings: Result<FileTransferSettings, FeatureApiError> = .failure(.NotStarted)
        var transfers = FileTransfers(send: nil, receive: nil)
        var setError: FeatureApiError?
        var shareWrites: [Bool] = []
        var folderWrites: [String] = []
        var sendResult: Result<FileSendResult, FeatureApiError> = .success(.sent(count: 1))
        var sends: [(paths: [String], origin: FileSendOrigin)] = []
        var cancels = 0
        var permission: Result<FilePermissionResult, FeatureApiError> = .success(.granted)
    }

    private let lock = NSLock()
    private var store = Store()

    func update(_ change: (inout Store) -> Void) {
        lock.withLock { change(&store) }
    }

    var shareWrites: [Bool] { lock.withLock { store.shareWrites } }
    var folderWrites: [String] { lock.withLock { store.folderWrites } }
    var sends: [(paths: [String], origin: FileSendOrigin)] { lock.withLock { store.sends } }

    func settings() throws -> FileTransferSettings {
        try lock.withLock { store.settings }.get()
    }

    func transfers() throws -> FileTransfers {
        lock.withLock { store.transfers }
    }

    func setShareEnabled(on: Bool) throws {
        let error: FeatureApiError? = lock.withLock {
            if store.setError == nil { store.shareWrites.append(on) }
            return store.setError
        }
        if let error { throw error }
    }

    func setReceivedFilesFolder(path: String) throws {
        let error: FeatureApiError? = lock.withLock {
            if store.setError == nil { store.folderWrites.append(path) }
            return store.setError
        }
        if let error { throw error }
    }

    func send(paths: [String], origin: FileSendOrigin) async throws -> FileSendResult {
        try lock.withLock {
            store.sends.append((paths, origin))
            return store.sendResult
        }.get()
    }

    func cancelSend() throws {
        lock.withLock { store.cancels += 1 }
    }

    func requestPermission() async throws -> FilePermissionResult {
        try lock.withLock { store.permission }.get()
    }
}
