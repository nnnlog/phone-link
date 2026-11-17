import Foundation
import Testing
import PhoneLinkEngine
@testable import PhoneLinkKit

private func state(
    capable: Bool = true,
    enabled: Bool = true,
    running: Bool = true,
    permission: Bool = true,
    sync: PhoneFilesSync = .succeeded,
    free: Int64 = 0,
    total: Int64 = 0,
    noticeOff: Bool = false
) -> PhoneFilesState {
    PhoneFilesState(
        domain: "phone-1",
        phoneName: "Galaxy",
        capable: capable,
        enabled: enabled,
        running: running,
        permissionGranted: permission,
        available: true,
        sync: sync,
        initialSyncDone: true,
        storageFree: free,
        storageTotal: total,
        gigabyteBytes: 1_000_000_000,
        recycleBin: "storage\\Recycle bin - Connected device",
        ltwOff: false,
        recycleNoticeOff: noticeOff
    )
}

private func item(_ identifier: String, parent: String = "", name: String = "a.jpg", directory: Bool = false) -> PhoneFileItem {
    PhoneFileItem(
        identifier: identifier,
        parent: parent,
        name: name,
        isDirectory: directory,
        size: 5,
        created: 100,
        modified: 200,
        fileType: 1,
        contentVersion: "5:200",
        metadataVersion: "11:storage\\a.jpg",
        hasThumbnail: true,
        synthetic: false
    )
}

@MainActor
@Suite struct PhoneFilesModelTests {
    private let api = PhoneFilesApiFake()
    private let hub = EngineEventHub()
    private let opened = FinderOpens()
    private let model: PhoneFilesModel

    init() {
        let opened = opened
        model = PhoneFilesModel(
            api: api,
            hub: hub,
            finderURL: { URL(fileURLWithPath: "/Users/u/Library/CloudStorage/Galaxy") },
            open: { opened.opened.append($0) }
        )
    }

    @Test func theStateLoadsOnStartAndOnChanges() {
        hub.postStarted()
        #expect(model.state == nil && !model.available)

        api.update { $0.state = .success(state(running: false, permission: false)) }
        hub.post(.phoneFiles(change: .stateChanged))
        #expect(model.available && !model.needsPermission && model.syncStatus == nil)

        api.update { $0.state = .success(state(permission: false, free: 25_000_000_000, total: 128_000_000_000)) }
        hub.post(.phoneFiles(change: .running(domain: "phone-1", running: true)))
        #expect(model.needsPermission)
        #expect(model.syncStatus == "Up to date")
        #expect(model.storageStatus == "25.0 GB free of 128.0 GB")
    }

    @Test func theSwitchWritesAndFailuresShow() {
        api.update { $0.state = .success(state()) }
        hub.postStarted()

        model.enabled = false
        #expect(api.enabledWrites == [false])

        api.update { $0.setError = .Store(message: "disk") }
        model.enabled = true
        #expect(model.failure?.title == "Couldn't Change the Setting")
    }

    @Test func aPermissionRequestTheUserMustAnswerIsShown() async {
        api.update {
            $0.state = .success(state(permission: false))
            $0.permission = .success(.required)
        }
        hub.postStarted()
        await model.requestPermission()
        #expect(model.permissionRequested)
        #expect(api.permissionOrigins == [.statusUi])
    }

    @Test func theRecycleNoticeShowsUntilTurnedOff() {
        api.update { $0.state = .success(state()) }
        hub.postStarted()
        hub.post(.phoneFiles(change: .recycleBinNotice(domain: "phone-1")))
        #expect(model.recycleNoticeShown)
        #expect(model.recycleBinName == "Recycle bin - Connected device")

        model.dismissRecycleNotice(again: false)
        #expect(!model.recycleNoticeShown)
        #expect(api.noticeOffWrites == 1)

        api.update { $0.state = .success(state(noticeOff: true)) }
        hub.post(.phoneFiles(change: .stateChanged))
        hub.post(.phoneFiles(change: .recycleBinNotice(domain: "phone-1")))
        #expect(!model.recycleNoticeShown)
    }

    @Test func showInFinderOpensTheDomainFolder() async {
        await model.showInFinder()
        #expect(opened.opened == [URL(fileURLWithPath: "/Users/u/Library/CloudStorage/Galaxy")])
    }
}

@Suite struct PhoneFilesHostServiceTests {
    private let api = PhoneFilesApiFake()

    private func call<T: Sendable>(_ body: (@escaping @Sendable (T) -> Void) -> Void) async -> T {
        await withCheckedContinuation { continuation in
            body { continuation.resume(returning: $0) }
        }
    }

    @Test func itemsTravelAsJSON() async throws {
        api.update { $0.children = .success([item("2", parent: "1")]) }
        let service = PhoneFilesHostService(api: api)

        let (data, error) = await call { reply in service.children(domain: "phone-1", identifier: "1") { reply(($0, $1)) } }
        #expect(error == nil)
        let records = try JSONDecoder().decode([PhoneFileRecord].self, from: try #require(data))
        #expect(records.map(\.identifier) == ["2"])
        #expect(records[0].parent == "1" && records[0].contentVersion == "5:200" && records[0].hasThumbnail)
    }

    @Test func failuresKeepTheirKind() async {
        api.update { $0.item = .failure(.Failure(failure: .exists, message: "taken")) }
        let service = PhoneFilesHostService(api: api)
        let (_, error) = await call { reply in service.item(domain: "phone-1", identifier: "9") { reply(($0, $1)) } }
        #expect(error?.domain == PhoneFilesXPC.errorDomain)
        #expect(error?.code == PhoneFilesXPCError.exists.rawValue)
        #expect(error?.localizedDescription == "taken")
    }

    @Test func aGenerationTooOldIsReportedExpired() async {
        api.update { $0.changes = .success(nil) }
        let service = PhoneFilesHostService(api: api)
        let (data, expired, error) = await call { reply in service.changes(domain: "phone-1", since: 3) { reply(($0, $1, $2)) } }
        #expect(data == nil && expired && error == nil)
    }

    @Test func aDownloadIsHandedOverOpenAndItsFileUnlinked() async throws {
        let file = FileManager.default.temporaryDirectory.appendingPathComponent("phone-files-test-\(UUID().uuidString)")
        try Data("hello".utf8).write(to: file)
        api.update { $0.download = .success(PhoneFileContent(path: file.path, item: item("2"))) }
        let service = PhoneFilesHostService(api: api)

        let (handle, data, error) = await call { reply in
            service.download(domain: "phone-1", identifier: "2", requestID: "r1") { reply(($0, $1, $2)) }
        }

        #expect(error == nil && data != nil)
        #expect(try handle?.readToEnd() == Data("hello".utf8))
        #expect(!FileManager.default.fileExists(atPath: file.path))
    }

    @Test func anUploadReadsTheHandleIntoAFileTheEngineReads() async throws {
        let source = FileManager.default.temporaryDirectory.appendingPathComponent("phone-files-src-\(UUID().uuidString)")
        try Data("12345".utf8).write(to: source)
        api.update { $0.upload = .success(item("40", parent: "3", name: "report.pdf")) }
        let service = PhoneFilesHostService(api: api)
        let handle = try FileHandle(forReadingFrom: source)

        let (data, error) = await call { reply in
            service.upload(
                domain: "phone-1", parent: "3", name: "report.pdf", content: handle,
                createdMs: NSNumber(value: 100_000), modifiedMs: NSNumber(value: 200_500), requestID: "r2"
            ) { reply(($0, $1)) }
        }

        #expect(error == nil && data != nil)
        let upload = try #require(api.uploads.first)
        #expect(upload.bytes == Data("12345".utf8))
        #expect(upload.parent == "3" && upload.name == "report.pdf")
        #expect(upload.createdMs == 100_000 && upload.modifiedMs == 200_500)

        try? FileManager.default.removeItem(at: source)
    }
}

@MainActor
private final class FinderOpens {
    var opened: [URL] = []
}

final class PhoneFilesApiFake: PhoneFilesApiProtocol, @unchecked Sendable {
    struct Store {
        var state: Result<PhoneFilesState, FeatureApiError> = .failure(.NotStarted)
        var setError: FeatureApiError?
        var enabledWrites: [Bool] = []
        var noticeOffWrites = 0
        var permission: Result<PhoneFilesPermissionResult, FeatureApiError> = .success(.granted)
        var permissionOrigins: [PhoneFilesPermissionOrigin] = []
        var item: Result<PhoneFileItem, PhoneFileError> = .failure(.Failure(failure: .unknownItem, message: "none"))
        var children: Result<[PhoneFileItem], PhoneFileError> = .success([])
        var changes: Result<PhoneFileChanges?, PhoneFileError> = .success(nil)
        var download: Result<PhoneFileContent, PhoneFileError> = .failure(.Failure(failure: .failed, message: "none"))
        var upload: Result<PhoneFileItem, PhoneFileError> = .failure(.Failure(failure: .failed, message: "none"))
        var uploads: [(parent: String, name: String, bytes: Data?, createdMs: Int64?, modifiedMs: Int64?)] = []
    }

    private let lock = NSLock()
    private var store = Store()

    func update(_ change: (inout Store) -> Void) {
        lock.withLock { change(&store) }
    }

    var enabledWrites: [Bool] { lock.withLock { store.enabledWrites } }
    var noticeOffWrites: Int { lock.withLock { store.noticeOffWrites } }
    var permissionOrigins: [PhoneFilesPermissionOrigin] { lock.withLock { store.permissionOrigins } }
    var uploads: [(parent: String, name: String, bytes: Data?, createdMs: Int64?, modifiedMs: Int64?)] {
        lock.withLock { store.uploads }
    }

    func state() throws -> PhoneFilesState { try lock.withLock { store.state }.get() }

    func setEnabled(on: Bool) throws {
        let error: FeatureApiError? = lock.withLock {
            if store.setError == nil { store.enabledWrites.append(on) }
            return store.setError
        }
        if let error { throw error }
    }

    func setRecycleNoticeOff() throws { lock.withLock { store.noticeOffWrites += 1 } }

    func requestPermission(origin: PhoneFilesPermissionOrigin) async throws -> PhoneFilesPermissionResult {
        try lock.withLock {
            store.permissionOrigins.append(origin)
            return store.permission
        }.get()
    }

    func refresh() throws {}

    func item(domain: String, identifier: String) throws -> PhoneFileItem { try lock.withLock { store.item }.get() }

    func children(domain: String, identifier: String) throws -> [PhoneFileItem] {
        try lock.withLock { store.children }.get()
    }

    func generation(domain: String) throws -> Int64 { 7 }

    func changes(domain: String, generation: Int64) throws -> PhoneFileChanges? {
        try lock.withLock { store.changes }.get()
    }

    func thumbnail(domain: String, identifier: String) async throws -> Data? { nil }

    func download(domain: String, identifier: String, requestId: String, progress: PhoneFileProgress) async throws -> PhoneFileContent {
        progress.progress(done: 5, total: 5)
        return try lock.withLock { store.download }.get()
    }

    func upload(
        domain: String, parent: String, name: String, source: String, createdMs: Int64?, modifiedMs: Int64?,
        requestId: String
    ) async throws -> PhoneFileItem {
        let bytes = FileManager.default.contents(atPath: source)
        return try lock.withLock {
            store.uploads.append((parent, name, bytes, createdMs, modifiedMs))
            return store.upload
        }.get()
    }

    func cancel(requestId: String) {}

    func createFolder(domain: String, parent: String, name: String) async throws -> PhoneFileItem {
        throw PhoneFileError.Failure(failure: .notSupported, message: "fake")
    }

    func rename(domain: String, identifier: String, parent: String, name: String) async throws -> PhoneFileItem {
        throw PhoneFileError.Failure(failure: .notSupported, message: "fake")
    }

    func delete(domain: String, identifier: String) async throws {}
}
