import Foundation
import os
import PhoneLinkEngine
import Testing
@testable import PhoneLinkKit

/// The controller's start, reconnect and notification-response rules against an engine whose
/// start the test finishes; nothing reaches the Rust engine.
@MainActor
struct EngineControllerTests {
    @Test func reconnectWhileAStartIsUnderWayLeavesTheStartAlone() async {
        let engine = FakeEngine()
        let controller = EngineController(engine: engine, hub: EngineEventHub())

        controller.start()
        let start = controller.pendingStart
        controller.refresh()
        #expect(controller.phase == .starting)

        engine.gate.open(.success(()))
        await start?.value
        #expect(controller.phase == .running)
        #expect(engine.calls.withLock { $0 } == FakeEngine.Calls(starts: 1, refreshes: 0, responses: []))
    }

    @Test func reconnectStartsAnEngineThatIsNotRunning() async {
        let engine = FakeEngine()
        let controller = EngineController(engine: engine, hub: EngineEventHub())

        controller.refresh()
        let failing = controller.pendingStart
        engine.gate.open(.failure(EngineError.Auth(message: "offline")))
        await failing?.value
        guard case .failed = controller.phase else {
            Issue.record("expected a failed start, got \(controller.phase)")
            return
        }

        controller.refresh()
        let retry = controller.pendingStart
        engine.gate.open(.success(()))
        await retry?.value
        #expect(controller.phase == .running)
        #expect(engine.calls.withLock { $0.starts } == 2)
    }

    @Test func aStartWithoutASelectedPhoneWaitsForTheUsersChoice() async {
        let engine = FakeEngine(needsPhoneSelection: true)
        let controller = EngineController(engine: engine, hub: EngineEventHub())
        var asked = 0
        controller.onPhoneSelectionNeeded = { asked += 1 }

        controller.start()
        let start = controller.pendingStart
        engine.gate.open(.success(()))
        await start?.value
        #expect(controller.phase == .selectingPhone)
        #expect(asked == 1, "the app shows the phone picker")
        #expect(controller.phones.map(\.id) == ["phone"])

        controller.refresh()
        #expect(controller.phase == .selectingPhone, "nothing to reconnect before a phone is selected")

        controller.selectPhone("phone")
        await controller.pendingStart?.value
        #expect(controller.phase == .running)
        #expect(engine.calls.withLock { $0.starts } == 1, "the selection starts the phone without a new start")
    }

    @Test func reconnectWhileRunningConnectsNow() async {
        let engine = FakeEngine()
        let controller = EngineController(engine: engine, hub: EngineEventHub())

        controller.start()
        let start = controller.pendingStart
        engine.gate.open(.success(()))
        await start?.value

        controller.refresh()
        #expect(engine.calls.withLock { $0.refreshes } == 1)
        #expect(controller.phase == .running)
    }

    @Test func aResponseBeforeTheStartIsSentOnceTheEngineRuns() async {
        let engine = FakeEngine()
        let controller = EngineController(engine: engine, hub: EngineEventHub())

        controller.notificationResponse(id: "launch", userInfo: [:], response: .activated)
        controller.start()
        let start = controller.pendingStart
        controller.notificationResponse(id: "starting", userInfo: [:], response: .dismissed)
        #expect(engine.calls.withLock { $0.responses }.isEmpty)

        engine.gate.open(.success(()))
        await start?.value
        #expect(engine.calls.withLock { $0.responses } == ["launch", "starting"])

        controller.notificationResponse(id: "running", userInfo: [:], response: .activated)
        #expect(engine.calls.withLock { $0.responses } == ["launch", "starting", "running"])
    }

    @Test func aResponseForARemovedAccountIsDropped() async {
        let engine = FakeEngine()
        let hub = EngineEventHub()
        let controller = EngineController(engine: engine, hub: hub)

        controller.start()
        let start = controller.pendingStart
        engine.gate.open(.success(()))
        await start?.value

        hub.post(.accountRemoved)
        #expect(controller.phase == .accountRemoved)

        controller.notificationResponse(id: "old", userInfo: [:], response: .activated)
        controller.start()
        let restart = controller.pendingStart
        engine.gate.open(.success(()))
        await restart?.value
        #expect(engine.calls.withLock { $0.responses }.isEmpty)
    }

    @Test func aStartThatFindsTheAccountRemovedAsksToSignInAgain() async {
        let engine = FakeEngine()
        let hub = EngineEventHub()
        let controller = EngineController(engine: engine, hub: hub)

        controller.start()
        let start = controller.pendingStart
        hub.post(.accountRemoved)
        engine.gate.open(.failure(EngineError.Setup(message: "the account was removed")))
        await start?.value
        #expect(controller.phase == .accountRemoved)
    }

    @Test func aFailedConnectTheUserStartedShowsUntilOneSucceeds() async {
        let engine = FakeEngine()
        let hub = EngineEventHub()
        let controller = EngineController(engine: engine, hub: hub)

        controller.start()
        let start = controller.pendingStart
        engine.gate.open(.success(()))
        await start?.value

        hub.post(.connectCompleted(immediate: false, outcome: .timedOut))
        #expect(controller.connectFailure == nil, "a background connect is retried without the user")
        hub.post(.connectCompleted(immediate: true, outcome: .timedOut))
        #expect(controller.connectFailure == UserText.failure(.timedOut))
        hub.post(.connectCompleted(immediate: false, outcome: .success))
        #expect(controller.connectFailure == nil)
        hub.post(.connectCompleted(immediate: true, outcome: .connectionFailed(reason: "refused")))
        #expect(controller.connectFailure != nil)
        hub.post(.phoneSession(connected: true, failure: nil))
        #expect(controller.connectFailure == nil)
    }

    @Test func aWebSignInRequestShowsTheSignInUntilTheStartEnds() async {
        let engine = FakeEngine()
        let hub = EngineEventHub()
        let controller = EngineController(engine: engine, hub: hub)
        var started = 0
        hub.onStart { started += 1 }

        controller.start()
        let start = controller.pendingStart
        let request = WebSignInRequest(
            kind: .login,
            url: "https://login.live.com/ppsecure/InlineLogin.srf",
            headers: [:],
            userAgent: "ua",
            bridgeScript: "",
            endUrlPrefix: nil
        )
        hub.post(.webSignInRequested(requestId: "req-1", request: request))
        guard case let .webSignIn(signIn) = controller.phase else {
            Issue.record("the sign-in page is shown")
            return
        }
        #expect(signIn.requestId == "req-1")

        // The web view posts the result: the engine is answered and the start goes on.
        controller.completeWebSignIn(json: "{\"K\":\"PUID\",\"sDAToken\":\"t\"}")
        #expect(engine.calls.withLock { $0.webSignInCompletions } == 1)
        #expect(controller.phase == .starting)

        engine.gate.open(.success(()))
        await start?.value
        #expect(controller.phase == .running)
        #expect(started == 1)
        #expect(controller.phone?.name == "Test Phone")
    }

    @Test func cancellingAWebSignInTellsTheEngine() async {
        let engine = FakeEngine()
        let hub = EngineEventHub()
        let controller = EngineController(engine: engine, hub: hub)

        controller.start()
        let start = controller.pendingStart
        let request = WebSignInRequest(
            kind: .login, url: "https://login.live.com", headers: [:], userAgent: "ua", bridgeScript: "", endUrlPrefix: nil
        )
        hub.post(.webSignInRequested(requestId: "req-2", request: request))
        controller.cancelWebSignIn(declined: true)
        #expect(engine.calls.withLock { $0.webSignInCancels } == 1)

        engine.gate.open(.failure(FakeError.cancelled))
        await start?.value
        if case .failed = controller.phase {} else {
            Issue.record("a declined sign-in fails the start")
        }
    }

    @Test func signingOutForgetsThePhoneAndWaitsForTheUserToSignIn() async throws {
        let engine = FakeEngine()
        let hub = EngineEventHub()
        let controller = EngineController(engine: engine, hub: hub)
        var removals = 0
        hub.onEvent { if case .accountRemoved = $0 { removals += 1 } }

        controller.start()
        let start = controller.pendingStart
        #expect(!controller.canSignOut, "not while a start is under way")
        engine.gate.open(.success(()))
        await start?.value
        #expect(controller.phone != nil && controller.canSignOut)

        try await controller.signOut()
        #expect(engine.calls.withLock { $0.signOuts } == 1)
        #expect(controller.phase == .signedOut)
        #expect(controller.phone == nil && controller.phones.isEmpty && !controller.connected)
        #expect(removals == 1, "the models drop what they show")
        #expect(!controller.canSignOut)

        controller.refresh()
        #expect(controller.phase == .signedOut, "reconnecting does not sign in by itself")

        controller.start()
        #expect(controller.phase == .starting)
        let again = controller.pendingStart
        engine.gate.open(.success(()))
        await again?.value
        #expect(engine.calls.withLock { $0.starts } == 2)
        #expect(controller.phase == .running)
    }
}

/// Finishes the engine's start when the test says so, whichever comes first.
private final class StartGate: Sendable {
    private struct State {
        var waiting: CheckedContinuation<Void, Error>?
        var result: Result<Void, Error>?
    }

    private let state = OSAllocatedUnfairLock(uncheckedState: State())

    func wait() async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            let result = state.withLockUnchecked { state -> Result<Void, Error>? in
                guard let result = state.result else {
                    state.waiting = continuation
                    return nil
                }
                state.result = nil
                return result
            }

            if let result {
                continuation.resume(with: result)
            }
        }
    }

    func open(_ result: Result<Void, Error>) {
        let waiting = state.withLockUnchecked { state -> CheckedContinuation<Void, Error>? in
            guard let waiting = state.waiting else {
                state.result = result
                return nil
            }
            state.waiting = nil
            return waiting
        }

        waiting?.resume(with: result)
    }
}

/// The engine's surface the controller uses. The feature objects have no Rust object behind them
/// and must not be called; the phone object answers `info()` itself.
private final class FakeEngine: EngineProtocol, Sendable {
    struct Calls: Equatable {
        var starts = 0
        var refreshes = 0
        var signOuts = 0
        var responses: [String] = []
        var webSignInCompletions = 0
        var webSignInCancels = 0
    }

    let gate = StartGate()
    let calls = OSAllocatedUnfairLock(initialState: Calls())
    let needsPhoneSelection: Bool

    init(needsPhoneSelection: Bool = false) {
        self.needsPhoneSelection = needsPhoneSelection
    }

    func clipboard() -> ClipboardApi { ClipboardApi(noPointer: .init()) }
    func contacts() -> ContactsApi { ContactsApi(noPointer: .init()) }
    func calling() -> CallingApi { CallingApi(noPointer: .init()) }
    func deviceStatus() -> DeviceStatusApi { DeviceStatusApi(noPointer: .init()) }
    func messaging() -> MessagingApi { MessagingApi(noPointer: .init()) }
    func notifications() -> NotificationsApi { NotificationsApi(noPointer: .init()) }
    func phone() -> PhoneApi { FakePhoneApi(noPointer: .init()) }
    func photos() -> PhotosApi { PhotosApi(noPointer: .init()) }
    func remoteControl() -> RemoteControlApi { RemoteControlApi(noPointer: .init()) }
    func audio() -> AudioApi { AudioApi(noPointer: .init()) }
    func hotspot() -> HotspotApi { HotspotApi(noPointer: .init()) }
    func wallpaper() -> WallpaperApi { WallpaperApi(noPointer: .init()) }
    func remoteSnipping() -> RemoteSnippingApi { RemoteSnippingApi(noPointer: .init()) }
    func fileTransfer() -> FileTransferApi { FileTransferApi(noPointer: .init()) }
    func phoneFiles() -> PhoneFilesApi { PhoneFilesApi(noPointer: .init()) }
    func connectedCamera() -> ConnectedCameraApi { ConnectedCameraApi(noPointer: .init()) }
    func phoneApps() -> PhoneAppsApi { PhoneAppsApi(noPointer: .init()) }
    func screenMirroring() -> ScreenMirroringApi { ScreenMirroringApi(noPointer: .init()) }
    func pairing() -> PairingApi { PairingApi(noPointer: .init()) }

    func notificationResponse(id: String, userInfo: [String: String], response: NotificationResponse) throws {
        calls.withLock { $0.responses.append(id) }
    }

    func refresh() throws {
        calls.withLock { $0.refreshes += 1 }
    }

    func setForeground(foreground: Bool) {}

    func start() async throws -> EngineStatus {
        calls.withLock { $0.starts += 1 }
        try await gate.wait()
        return status()
    }

    func status() -> EngineStatus {
        EngineStatus(
            ownDeviceId: "pc",
            phoneId: needsPhoneSelection ? nil : "phone",
            phoneConnected: !needsPhoneSelection,
            needsPhoneSelection: needsPhoneSelection
        )
    }

    func signOut() async throws {
        calls.withLock { $0.signOuts += 1 }
    }

    func completeWebSignIn(requestId: String, resultJson: String) {
        calls.withLock { $0.webSignInCompletions += 1 }
    }

    func cancelWebSignIn(requestId: String, declined: Bool) {
        calls.withLock { $0.webSignInCancels += 1 }
    }

    func deviceRegistration() -> DeviceRegistration? {
        DeviceRegistration(puid: "test-puid", distinguishedDeviceIds: [])
    }

    func resetDeviceRegistration() async throws {
        calls.withLock { $0.signOuts += 1 }
    }

    func stop() async throws {}
    func systemEvent(event: SystemEvent) {}
    func version() -> String { "test" }
}

private enum FakeError: Error { case cancelled }

private final class FakePhoneApi: PhoneApi, @unchecked Sendable {
    override func info() throws -> PhoneInfo {
        PhoneInfo(
            id: "phone",
            name: "Test Phone",
            manufacturer: nil,
            model: nil,
            osVersion: nil,
            connected: true,
            allFeaturesDisabled: false
        )
    }
    override func phones() throws -> [PhoneChoice] {
        [PhoneChoice(id: "phone", name: "Test Phone", model: nil, lastSeen: 0, selected: true)]
    }

    override func select(id: String) async throws {}
}
