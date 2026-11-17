import Foundation
import Testing
import PhoneLinkEngine
@testable import PhoneLinkKit

@MainActor
@Suite struct RemoteSnippingModelTests {
    private let api = RemoteSnippingApiFake()
    private let hub = EngineEventHub()
    private let folder = FileManager.default.temporaryDirectory
        .appendingPathComponent("RemoteSnippingModelTests-\(UUID().uuidString)")
    private let opened = OpenedFiles()
    private let model: RemoteSnippingModel

    init() {
        let opened = opened
        model = RemoteSnippingModel(api: api, hub: hub, temporaryDirectory: folder, openFile: { opened.files.append($0) })
    }

    private static func settings(
        applicable: Bool = true,
        enabled: Bool = true,
        needsPermission: Bool = false,
        requesting: Bool = false,
        covers: Bool = true
    ) -> RemoteSnippingSettings {
        RemoteSnippingSettings(
            applicable: applicable,
            enabled: enabled,
            needsPermission: needsPermission,
            requestingPermission: requesting,
            coversPhotosNotifications: covers
        )
    }

    @Test func theSettingsLoadOnStartAndOnEveryChange() {
        hub.postStarted()
        #expect(model.settings == nil)
        #expect(!model.showsToggle)
        #expect(model.photosNotificationsApply)

        api.update { $0.settings = .success(Self.settings()) }
        hub.postStarted()
        #expect(model.showsToggle && model.getsNotifications && !model.needsPermission)
        #expect(!model.photosNotificationsApply)

        api.update { $0.settings = .success(Self.settings(needsPermission: true, covers: false)) }
        hub.post(.remoteSnipping(change: .settingsChanged))
        #expect(model.needsPermission && model.photosNotificationsApply)

        api.update { $0.settings = .success(Self.settings(applicable: false)) }
        hub.post(.resync(feature: .remoteSnipping))
        #expect(!model.showsToggle)

        hub.post(.accountRemoved)
        #expect(model.settings == nil)
    }

    @Test func theToggleWritesAndReadsBack() {
        api.update { $0.settings = .success(Self.settings()) }
        hub.postStarted()

        api.update { $0.settings = .success(Self.settings(enabled: false)) }
        model.getsNotifications = false
        #expect(api.written == [false])
        #expect(!model.getsNotifications)

        api.update { $0.setError = .Store(message: "disk") }
        model.getsNotifications = true
        #expect(model.failure != nil)
    }

    @Test func aFailedPermissionRequestShowsAnAlert() async {
        api.update { $0.settings = .success(Self.settings(needsPermission: true)) }
        hub.postStarted()

        api.update { $0.permission = .success(.requestSent) }
        await model.requestPermission()
        #expect(model.failure == nil)

        api.update { $0.permission = .success(.failed(reason: "no current account")) }
        await model.requestPermission()
        #expect(model.failure?.title == "Couldn't Ask Your Phone")
        #expect(api.permissionRequests == 2)
    }

    @Test func aClickedNotificationOpensItsImageNamedAfterItsType() async throws {
        let png = Self.png()
        hub.post(.remoteSnipping(change: .openImage(mediaId: 42, bytes: png)))
        await model.opening?.value
        #expect(opened.files.map(\.lastPathComponent) == ["42.png"])
        #expect(try Data(contentsOf: opened.files[0]) == png)

        // Bytes that are no image open nothing.
        hub.post(.remoteSnipping(change: .openImage(mediaId: 43, bytes: Data("<svg/>".utf8))))
        await model.opening?.value
        #expect(opened.files.count == 1)

        try? FileManager.default.removeItem(at: folder)
    }

    /// A 1 × 1 PNG.
    private static func png() -> Data {
        Data(base64Encoded: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==")!
    }
}

@MainActor
private final class OpenedFiles {
    var files: [URL] = []
}

private final class RemoteSnippingApiFake: RemoteSnippingApiProtocol, @unchecked Sendable {
    struct Store {
        var settings: Result<RemoteSnippingSettings, FeatureApiError> = .failure(.NotStarted)
        var setError: FeatureApiError?
        var written: [Bool] = []
        var permission: Result<RemoteSnippingPermission, FeatureApiError> = .success(.granted)
        var permissionRequests = 0
    }

    private let lock = NSLock()
    private var store = Store()

    func update(_ change: (inout Store) -> Void) {
        lock.withLock { change(&store) }
    }

    var written: [Bool] { lock.withLock { store.written } }
    var permissionRequests: Int { lock.withLock { store.permissionRequests } }

    func settings() throws -> RemoteSnippingSettings {
        try lock.withLock { store.settings }.get()
    }

    func setEnabled(on: Bool) throws {
        let error: FeatureApiError? = lock.withLock {
            if store.setError == nil { store.written.append(on) }
            return store.setError
        }
        if let error { throw error }
    }

    func requestPermission() async throws -> RemoteSnippingPermission {
        try lock.withLock {
            store.permissionRequests += 1
            return store.permission
        }.get()
    }
}
