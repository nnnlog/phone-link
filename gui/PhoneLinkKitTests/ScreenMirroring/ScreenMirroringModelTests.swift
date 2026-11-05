import Foundation
import PhoneLinkEngine
import Testing
@testable import PhoneLinkKit

@MainActor
struct ScreenMirroringModelTests {
    /// A model on `api` whose window calls are recorded in `windows`.
    private func model(_ api: FakeScreenMirroring, hub: EngineEventHub = EngineEventHub(), windows: MirrorWindowLog) -> ScreenMirroringModel {
        let model = ScreenMirroringModel(
            api: api,
            hub: hub,
            appName: { $0 == "com.app" ? "App" : nil },
            phoneName: { "Pixel" }
        )

        model.presentWindow = { windows.calls.append("open \($0)") }
        model.dismissWindow = { windows.calls.append("close \($0)") }
        model.raiseWindow = { windows.calls.append("raise \($0)") }
        model.presentFailure = { windows.calls.append("failure \($0.title)") }
        model.showMainWindow = { windows.calls.append("main window") }
        model.showApps = { windows.calls.append("apps") }

        return model
    }

    @Test func aWindowOpensForEverySessionTheEngineOpens() {
        let api = FakeScreenMirroring()
        let hub = EngineEventHub()
        let windows = MirrorWindowLog()
        let model = model(api, hub: hub, windows: windows)

        hub.postStarted()
        #expect(model.isOffered)
        #expect(windows.calls.isEmpty)

        api.sessionList = [mirrorSession("s1", phoneScreen: true)]
        hub.post(.screenMirroring(change: .sessionOpened(sessionId: "s1", packageName: "yourphone.phonescreen", phoneInitiated: false)))
        #expect(windows.calls == ["open s1"])
        #expect(model.session("s1")?.phoneScreen == true)
        #expect(model.title(for: model.session("s1")!) == "Pixel")

        // A session the phone started opens a window too.
        api.sessionList.append(mirrorSession("s2", package: "com.app"))
        hub.post(.screenMirroring(change: .sessionOpened(sessionId: "s2", packageName: "com.app", phoneInitiated: true)))
        #expect(windows.calls == ["open s1", "open s2"])
        #expect(model.title(for: model.session("s2")!) == "App")

        hub.post(.screenMirroring(change: .bringToFront(sessionId: "s2")))
        #expect(windows.calls.last == "raise s2")

        api.sessionList.removeFirst()
        hub.post(.screenMirroring(change: .sessionClosed(sessionId: "s1")))
        #expect(windows.calls.last == "close s1")
        #expect(model.sessions.map(\.id) == ["s2"])
    }

    @Test func aLaunchThePhoneAskedForShowsTheMainWindowOrItsApps() {
        let hub = EngineEventHub()
        let windows = MirrorWindowLog()
        let model = model(FakeScreenMirroring(), hub: hub, windows: windows)
        hub.postStarted()
        hub.post(.screenMirroring(change: .showApps))
        hub.post(.screenMirroring(change: .showMainWindow))
        #expect(windows.calls == ["apps", "main window"])
        withExtendedLifetime(model) {}
    }

    @Test func dragsPassTheFilesMIMETypesAndTellTheUserOfARefusal() {
        let api = FakeScreenMirroring()
        let hub = EngineEventHub()
        let windows = MirrorWindowLog()
        let model = model(api, hub: hub, windows: windows)

        let files = [URL(fileURLWithPath: "/tmp/a.png"), URL(fileURLWithPath: "/tmp/b.unknownext")]
        #expect(model.dragEnter("s1", files: files) == .accepted)
        #expect(api.calls == [#"drag enter ["/tmp/a.png", "/tmp/b.unknownext"] ["image/png", "-"]"#])
        model.dragDrop("s1")
        #expect(api.calls.last == "drag drop")

        model.dragOverLimitDropped("s1")
        #expect(windows.calls == ["failure Can't Send These Files"])

        // The phone app refusing the files is a notification naming an app window's app.
        let notifier = NoticeLog()
        model.notifier = notifier
        api.sessionList = [mirrorSession("s1", package: "com.app"), mirrorSession("s2", phoneScreen: true)]
        hub.postStarted()
        hub.post(.screenMirroring(change: .dragNotAccepted(sessionId: "s1")))
        hub.post(.screenMirroring(change: .dragNotAccepted(sessionId: "s2")))
        hub.post(.screenMirroring(change: .dragEnded(sessionId: "s1")))

        #expect(notifier.bodies == [
            "App can't receive files from this Mac.",
            "This app can't receive files from this Mac.",
        ])
    }

    @Test func aSessionChangeIsReadAgain() {
        let api = FakeScreenMirroring()
        let hub = EngineEventHub()
        let windows = MirrorWindowLog()
        let model = model(api, hub: hub, windows: windows)

        api.sessionList = [mirrorSession("s1")]
        hub.postStarted()
        #expect(windows.calls == ["open s1"], "a start opens the windows of the sessions there are")

        guard case let .loading(text, _) = model.overlay(for: model.session("s1")!) else {
            Issue.record("a loading window shows the wait")
            return
        }
        #expect(text == "Opening yourphone.phonescreen…")

        guard case let .loading(phoneText, _) = model.overlay(for: mirrorSession("p", phoneScreen: true)) else {
            Issue.record("a loading window shows the wait")
            return
        }
        #expect(phoneText == "Connecting to Pixel…")

        api.sessionList = [mirrorSession("s1", state: .connected, videoReady: true)]
        hub.post(.screenMirroring(change: .sessionChanged(sessionId: "s1")))
        #expect(model.overlay(for: model.session("s1")!) == nil)
        #expect(model.takesInput(model.session("s1")!))

        // A session the engine no longer has closes its window.
        api.sessionList = []
        hub.post(.screenMirroring(change: .sessionChanged(sessionId: "s1")))
        #expect(windows.calls.last == "close s1")
    }

    @Test func closingAWindowClosesItsSessionOnce() {
        let api = FakeScreenMirroring()
        let hub = EngineEventHub()
        let windows = MirrorWindowLog()
        let model = model(api, hub: hub, windows: windows)
        api.sessionList = [mirrorSession("s1"), mirrorSession("s2")]
        hub.postStarted()

        model.windowClosed("s1")
        model.windowClosed("s1")
        #expect(api.calls == ["close s1"])

        // The engine's closing afterwards does not close the window again.
        api.sessionList = [mirrorSession("s2")]
        hub.post(.screenMirroring(change: .sessionClosed(sessionId: "s1")))
        #expect(!windows.calls.contains("close s1"))

        // A takeover's Close closes the session and the window.
        model.perform(.close, in: "s2")
        #expect(api.calls == ["close s1", "close s2"])
        #expect(windows.calls.last == "close s2")

        model.windowClosed("s2")
        #expect(api.calls.count == 2)
    }

    @Test func theAccountLeavingClosesEveryWindow() {
        let api = FakeScreenMirroring()
        let hub = EngineEventHub()
        let windows = MirrorWindowLog()
        let model = model(api, hub: hub, windows: windows)
        api.sessionList = [mirrorSession("s1")]
        hub.postStarted()

        hub.post(.phoneUnlinked)
        #expect(windows.calls == ["open s1", "close s1"])
        #expect(model.status == nil && model.sessions.isEmpty && !model.isOffered)
    }

    @Test func launchesGoToTheEngine() {
        let api = FakeScreenMirroring()
        let windows = MirrorWindowLog()
        let model = model(api, windows: windows)
        model.reload()

        model.openPhoneScreen()
        model.launch(.app(packageName: "com.app"))
        model.launch(.task(packageName: "com.app", taskId: 7))
        #expect(api.calls == ["phone screen", "launch com.app nil", "launch com.app 7"])

        api.fail = true
        model.launch(.app(packageName: "com.app"))
        #expect(windows.calls == ["failure Couldn't Open the App"])
    }

    @Test func allowAndRetryTurnsAppsOnFirst() {
        let api = FakeScreenMirroring()
        api.settings.allowApps = false
        let model = model(api, windows: MirrorWindowLog())
        model.reload()
        model.perform(.allowAndRetry, in: "s1")
        #expect(api.calls == ["settings allowApps=true", "retry"])
    }

    @Test func settingsChangeThroughTheEngine() {
        let api = FakeScreenMirroring()
        let model = model(api, windows: MirrorWindowLog())
        model.reload()
        model.updateSettings { $0.blackScreen = false }
        #expect(api.settings.blackScreen == false)
        #expect(model.status?.settings.blackScreen == false)
    }

    @Test func aNotificationOpensOnThePhoneOnlyWhereItsTargetIsThePhone() async {
        let api = FakeScreenMirroring()
        let hub = EngineEventHub()
        let windows = MirrorWindowLog()
        let model = model(api, hub: hub, windows: windows)
        model.reload()

        #expect(model.canOpen(notification(target: .notifications, launch: true)))
        #expect(!model.canOpen(notification(target: .notifications, launch: false)))
        #expect(!model.canOpen(notification(target: .messaging, launch: true)))

        api.phoneScreenApps = false
        model.reload()
        #expect(!model.canOpen(notification(target: .notifications, launch: true)))

        await model.open(notification(target: .notifications, launch: true)).value
        #expect(api.calls == ["notification n -1"])
        hub.post(.screenMirroring(change: .notificationLaunched(key: "n", opened: false)))
        #expect(windows.calls == ["failure Couldn't Open on Your Phone"])
    }

    @Test func errorsShowTheirButton() {
        let api = FakeScreenMirroring()
        let model = model(api, windows: MirrorWindowLog())
        model.reload()

        let connect = MirrorError(code: 81, kind: .disconnected, action: .connect)
        var view = mirrorSession("s1", state: .error)
        view.error = connect
        guard case let .message(_, title, _, buttons) = model.overlay(for: view) else {
            Issue.record("an error shows a message")
            return
        }
        #expect(title == "Disconnected")
        #expect(buttons == [MirrorOverlayButton("Connect", .retry)])

        view.state = .dismissibleError
        view.error = MirrorError(code: 34, kind: .appNotAvailable, action: .close)
        guard case let .message(_, _, _, dismiss) = model.overlay(for: view) else {
            Issue.record("an error shows a message")
            return
        }
        #expect(dismiss == [MirrorOverlayButton("Dismiss", .dismiss)])

        view.state = .error
        view.error = MirrorError(code: 38, kind: .powerSaving, action: .relaunch)
        guard case let .message(_, _, _, relaunch) = model.overlay(for: view) else {
            Issue.record("an error shows a message")
            return
        }
        #expect(relaunch == [MirrorOverlayButton("Open Again", .reopen)], "power saving launches again")

        view.state = .restart
        view.restartLayoutSwitch = true
        #expect(model.overlay(for: view) == .layoutSwitch)

        view.restartLayoutSwitch = false
        view.restartTaskTransfer = true
        #expect(model.overlay(for: view) == .moveToMac(taskTransfer: true))
    }
}

@MainActor
struct MirrorWindowStateTests {
    private func model(_ api: FakeScreenMirroring) -> ScreenMirroringModel {
        let model = ScreenMirroringModel(api: api, hub: EngineEventHub(), appName: { _ in nil }, phoneName: { "Pixel" })
        model.reload()
        return model
    }

    /// The windows' takeovers: Resume alone for a suspended app, Open Again for power saving, the touch
    /// confirmation with its own window.
    @Test func takeoversOfferWhatTheStateCallsFor() {
        let model = model(FakeScreenMirroring())
        guard case let .message(_, _, _, suspended) = model.overlay(for: mirrorSession("s", state: .suspended)) else {
            Issue.record("a message")
            return
        }
        #expect(suspended == [MirrorOverlayButton("Resume", .resume)])

        guard case let .message(_, _, _, power) = model.overlay(for: mirrorSession("s", state: .powerSavingMode)) else {
            Issue.record("a message")
            return
        }
        #expect(power == [MirrorOverlayButton("Open Again", .reopen)])
        #expect(model.overlay(for: mirrorSession("s", state: .trueTouchWarning)) == .trueTouch)

        guard case let .message(_, _, _, overheat) = model.overlay(for: mirrorSession("s", state: .deviceOverheat)) else {
            Issue.record("a message")
            return
        }
        #expect(overheat == [MirrorOverlayButton("Close", .close)])
    }

    /// The title bar's audio button is the engine's, whatever the Mac's audio-streaming setting;
    /// *Enable audio* asks the phone again.
    @Test func theAudioButtonIsTheEngines() async throws {
        let api = FakeScreenMirroring()
        api.settings.audioStreaming = false
        let model = model(api)
        model.reload()

        var view = mirrorSession("s", phoneScreen: true, state: .connected, videoReady: true)
        #expect(model.audioButton(view) == .none)
        view.audioButton = .volume
        #expect(model.audioButton(view) == .volume, "not gated on the audio-streaming setting")
        view.audioButton = .enableAudio
        #expect(model.audioButton(view) == .enableAudio)

        model.enableAudio("s")
        try await Task.sleep(for: .milliseconds(200))
        #expect(api.calls == ["enable audio s"])
    }

    /// A window takes drops of Mac files only while connected, and not while it tells the user of a
    /// drop over the limits.
    @Test func dropsNeedAConnectedWindow() {
        let api = FakeScreenMirroring()
        api.sessionList = [mirrorSession("s", state: .loading), mirrorSession("t", state: .connected, videoReady: true)]
        let model = model(api)

        #expect(!model.acceptsDrops("s"), "not connected")
        #expect(model.acceptsDrops("t"))
        #expect(!model.acceptsDrops("u"), "no such window")

        var during: [Bool] = []
        model.presentFailure = { _ in during = [model.acceptsDrops("t"), model.acceptsDrops("s")] }
        model.dragOverLimitDropped("t")
        #expect(during == [false, false], "the window telling of it takes none meanwhile")
        #expect(model.acceptsDrops("t"), "and takes them again once told")
    }

    /// An app that stops letting its audio be captured is told in its open window for a moment;
    /// allowing it again hides that, the window losing the focus does not. Told before the window was
    /// first active, the moment starts again when it first is.
    @Test func aRefusedCaptureIsShownForAMoment() async throws {
        let api = FakeScreenMirroring()
        let hub = EngineEventHub()
        let model = ScreenMirroringModel(api: api, hub: hub, appName: { _ in nil }, phoneName: { "Pixel" })
        api.sessionList = [mirrorSession("s", package: "com.app", state: .connected, videoReady: true)]
        hub.postStarted()
        #expect(!model.showsCaptureNotice("s"))

        func capture(_ allowed: Bool) {
            api.sessionList[0].audioCaptureAllowed = allowed
            hub.post(.screenMirroring(change: .sessionChanged(sessionId: "s")))
        }
        capture(false)
        #expect(model.showsCaptureNotice("s"))

        capture(false)
        capture(true)
        #expect(!model.showsCaptureNotice("s"), "allowed again")

        capture(false)
        model.focusChanged("s", .notInFocus, capsLock: false)
        #expect(model.showsCaptureNotice("s"), "the window losing the focus keeps it")

        // First active 2 s in: the notice shows 3 s from then.
        try await Task.sleep(for: .seconds(2))
        model.windowActivated("s")
        try await Task.sleep(for: .seconds(2))
        #expect(model.showsCaptureNotice("s"), "started again at the first activation")
        try await Task.sleep(for: .milliseconds(1300))
        #expect(!model.showsCaptureNotice("s"), "after a moment")

        // Later activations do not start it again.
        capture(true)
        capture(false)
        try await Task.sleep(for: .seconds(2))
        model.windowActivated("s")
        try await Task.sleep(for: .milliseconds(1300))
        #expect(!model.showsCaptureNotice("s"))

        // A window first read already refused tells it too: it started allowed.
        api.sessionList.append(mirrorSession("t", package: "com.app"))
        api.sessionList[1].audioCaptureAllowed = false
        hub.post(.screenMirroring(change: .sessionOpened(sessionId: "t", packageName: "com.app", phoneInitiated: false)))
        #expect(model.showsCaptureNotice("t"))
    }

    /// Back shows while the video does; the navigation bar for gesture navigation, whatever the
    /// state; the layout button only for an app window in one of the two layouts, switching to the
    /// other.
    @Test func toolbarButtonsFollowTheWindow() {
        let model = model(FakeScreenMirroring())
        var view = mirrorSession("s", package: "com.app", state: .connected, videoReady: true)
        #expect(model.showsBack(view))
        #expect(!model.showsNavigationBar(view))
        view.navigationButtons = true
        #expect(model.showsNavigationBar(view))

        view.state = .loading
        #expect(!model.showsBack(view))
        #expect(model.showsNavigationBar(view))

        view.state = .connected
        #expect(!model.offersLayouts(view), "the layout is not known")
        view.screenLayout = 2
        #expect(model.offersLayouts(view) && model.otherLayout(view) == 1)
        view.screenLayout = 1
        #expect(model.otherLayout(view) == 2)
    }

    /// The navigation bar's buttons reach the phone only while the window is connected, at once.
    @Test func navigationBarButtonsNeedAConnectedWindow() {
        let api = FakeScreenMirroring()
        api.sessionList = [mirrorSession("s", state: .loading)]
        let model = model(api)
        model.reload()

        model.navigationBarButton("s", .home)
        #expect(api.calls.isEmpty)

        api.sessionList = [mirrorSession("s", state: .connected, videoReady: true)]
        model.reload()
        model.navigationBarButton("s", .recents)
        model.navigationBarButton("s", .back)
        #expect(api.calls == ["nav recents", "nav back"], "Back without the title bar's wait")
    }

    /// A layout switch tells the window before the phone is asked.
    @Test func aLayoutSwitchTellsTheWindowFirst() {
        let api = FakeScreenMirroring()
        let model = model(api)
        var told: [String] = []
        model.willSwitchLayout = { id, layout in told.append("\(id) \(layout) \(api.calls.count)") }
        model.switchLayout("s", to: 2)
        #expect(told == ["s 2 0"])
        #expect(api.calls == ["switch s 2"])
    }

    /// Back in the first moments of a connection waits, and several presses then make one.
    @Test func backWaitsForTheConnectionToSettle() async throws {
        let api = FakeScreenMirroring()
        api.sessionList = [mirrorSession("s", state: .connected, videoReady: true)]
        let model = model(api)

        model.back("s")
        model.back("s")
        #expect(api.calls.filter { $0 == "nav back" }.isEmpty)
        try await Task.sleep(for: ScreenMirroringModel.backSettling + .milliseconds(300))
        #expect(api.calls.filter { $0 == "nav back" }.count == 1)

        model.back("s")
        #expect(api.calls.filter { $0 == "nav back" }.count == 2, "later, at once")
    }

    /// A state change before the window settles loses the waiting press; protected content reached
    /// before then keeps its presses waiting, and once settled Back there goes out at once.
    @Test func backBeforeTheWindowSettlesIsLostOnAStateChange() async throws {
        let api = FakeScreenMirroring()
        let hub = EngineEventHub()
        api.sessionList = [mirrorSession("s", state: .connected, videoReady: true)]
        let model = ScreenMirroringModel(api: api, hub: hub, appName: { _ in nil }, phoneName: { "Pixel" })
        model.reload()

        model.back("s")
        api.sessionList = [mirrorSession("s", state: .secureContent, videoReady: true)]
        hub.post(.screenMirroring(change: .sessionChanged(sessionId: "s")))
        model.back("s")
        try await Task.sleep(for: ScreenMirroringModel.backSettling + .milliseconds(300))
        #expect(api.calls.filter { $0 == "nav back" }.isEmpty)

        let settled = FakeScreenMirroring()
        let settledHub = EngineEventHub()
        settled.sessionList = [mirrorSession("t", state: .connected, videoReady: true)]
        let other = ScreenMirroringModel(api: settled, hub: settledHub, appName: { _ in nil }, phoneName: { "Pixel" })
        other.reload()
        try await Task.sleep(for: ScreenMirroringModel.backSettling + .milliseconds(300))

        settled.sessionList = [mirrorSession("t", state: .secureContent, videoReady: true)]
        settledHub.post(.screenMirroring(change: .sessionChanged(sessionId: "t")))
        other.back("t")
        #expect(settled.calls.filter { $0 == "nav back" }.count == 1, "settled before the protected content")
    }

    /// Resume launches the paused app again past the phone's checks; activating the paused window
    /// asks the phone whether it can resume, and shows nothing when it cannot.
    @Test func resumeLaunchesAgainAndActivationAsks() async throws {
        let api = FakeScreenMirroring()
        api.resumable = false
        api.sessionList = [mirrorSession("s", state: .suspended)]
        let model = model(api)
        let failures = MirrorWindowLog()
        model.presentFailure = { failures.calls.append($0.title) }

        model.perform(.resume, in: "s")
        #expect(api.calls == ["continue s"])

        model.windowActivated("s")
        try await Task.sleep(for: .milliseconds(200))
        #expect(api.calls == ["continue s", "resume s"])
        #expect(failures.calls.isEmpty)

        api.sessionList = [mirrorSession("s", state: .connected, videoReady: true)]
        model.reload()
        model.windowActivated("s")
        try await Task.sleep(for: .milliseconds(200))
        #expect(api.calls == ["continue s", "resume s"], "only a paused window asks")
    }
}

/// The window calls a model made.
@MainActor
final class MirrorWindowLog {
    var calls: [String] = []
}

func mirrorSession(
    _ id: String,
    package: String = "yourphone.phonescreen",
    phoneScreen: Bool = false,
    state: MirrorWindowState = .loading,
    videoReady: Bool = false
) -> MirrorSessionView {
    MirrorSessionView(
        id: id, packageName: package, phoneScreen: phoneScreen, phoneInitiated: false, state: state, error: nil,
        secureLocked: false, unlockFromMac: false, restartTaskTransfer: false, restartLayoutSwitch: false,
        launched: true, videoReady: videoReady, deviceResolution: MirrorSize(width: 1080, height: 2400),
        initialDeviceResolution: MirrorSize(width: 1440, height: 3120), orientation: 0, rotation: 0, videoSize: MirrorSize(width: 720, height: 1600), contentArea: nil,
        navigationButtons: false, buttonOrder: nil, recentTaskId: 0, screenLayout: nil, lastScreenLayout: 0, moveToVirtualDisplay: nil,
        volume: 50, muted: false, silent: false, audioCaptureAllowed: true, audioButton: .none
    )
}

private func notification(target: NotificationOpenTarget, launch: Bool) -> PhoneNotification {
    PhoneNotification(
        key: "n", packageName: "com.app", appName: "App", title: "Title", subtitle: nil, body: "Body",
        conversation: [], timeMs: 0, pinned: false, openTarget: target, launchSupported: launch,
        isReadOnlyGroupMessage: false, reply: nil, actions: []
    )
}

private final class NoticeLog: SystemNotifier, @unchecked Sendable {
    var bodies: [String] = []
    func post(notification: SystemNotification) throws { bodies.append(notification.body) }
    func withdraw(ids: [String]) throws {}
    func isAuthorized() -> Bool { true }
}

private final class FakeScreenMirroring: ScreenMirroringApiProtocol, @unchecked Sendable {
    var sessionList: [MirrorSessionView] = []
    var settings = MirrorSettings(
        allowApps: true, blackScreen: true, audioStreaming: true, alwaysExpanded: false, doNotAskRestart: false,
        doNotAskTaskTransfer: false, doNotShowTrueTouch: false, doNotAskSkipUnlock: false,
        doNotAskSkipVerification: false, doNotAskSwitchLayout: false
    )
    var phoneScreenApps = true
    var resumable = true
    var fail = false
    var calls: [String] = []

    private func record(_ call: String) throws {
        if fail { throw FeatureApiError.NotAvailable(reason: "test") }
        calls.append(call)
    }

    func status() throws -> MirroringStatusView {
        MirroringStatusView(
            remotingSupported: true, phoneScreenApps: phoneScreenApps, appsFeatureType: .multipleApps,
            inputSupported: true, audioSupported: true, adaptiveLayout: true, phoneState: .connected,
            phoneError: nil, nearby: nil, settings: settings, permissionAccepted: true
        )
    }

    func sessions() throws -> [MirrorSessionView] { sessionList }
    func session(sessionId: String) throws -> MirrorSessionView? { sessionList.first { $0.id == sessionId } }
    func openPhoneScreen() throws -> String {
        try record("phone screen")
        return "s"
    }

    func launchApp(packageName: String, transferTaskId: Int64?) throws -> String {
        try record("launch \(packageName) \(transferTaskId.map(String.init) ?? "nil")")
        return "s"
    }

    func launchNotification(key: String, actionIndex: Int32) async throws { try record("notification \(key) \(actionIndex)") }
    func closeSession(sessionId: String) throws { try record("close \(sessionId)") }
    func retry() throws { try record("retry") }
    func setSettings(settings: MirrorSettings) throws {
        try record("settings allowApps=\(settings.allowApps)")
        self.settings = settings
    }

    func answerMoveToVirtualDisplay(sessionId: String, accept: Bool, doNotAsk: Bool) throws {}
    func audioOutputChanged() throws {}
    func confirmLayoutSwitch(sessionId: String, doNotAsk: Bool) throws {}
    func continueTrueTouch(sessionId: String) throws { try record("continue \(sessionId)") }
    func continueWithoutAudio(sessionId: String) throws {}
    func enableAudio(sessionId: String) async throws { try record("enable audio \(sessionId)") }
    func dismissError(sessionId: String) throws {}
    func focusChanged(sessionId: String, focus: MirrorFocus, capsLock: Bool) throws {}
    func inputFocus(sessionId: String, focused: Bool, capsLock: Bool) throws {}
    func key(sessionId: String, keyCode: UInt16, action: MirrorKeyAction, flags: UInt64) throws {}
    func mouseButton(sessionId: String, button: MirrorMouseButton, down: Bool, x: Double, y: Double) throws {}
    func mouseMove(sessionId: String, x: Double, y: Double) throws {}
    func dragEnter(sessionId: String, files: [MirrorDragFile]) throws -> MirrorDragEnter {
        try record("drag enter \(files.map(\.path)) \(files.map { $0.mimeType ?? "-" })")
        return .accepted
    }
    func dragOver(sessionId: String, x: Double, y: Double) throws -> Bool { true }
    func dragLeave(sessionId: String) throws { try record("drag leave") }
    func dragDrop(sessionId: String) throws { try record("drag drop") }
    func takePhoneDrag(sessionId: String, buttonHeld: Bool) throws -> [MirrorPhoneDragItem]? { nil }
    func phoneDragEnded(sessionId: String, dropped: Bool) throws {}
    func writePhoneDragItem(sessionId: String, dataId: String, path: String) async throws {}
    func mouseWheel(sessionId: String, x: Double, y: Double, deltaX: Int32, deltaY: Int32) throws {}
    func navButton(sessionId: String, button: MirrorNavButton) throws {
        switch button {
        case .back: try record("nav back")
        case .home: try record("nav home")
        case .recents: try record("nav recents")
        }
    }
    func nearbyCodeRefreshed() throws {}
    func nearbyRequestPinCode() throws {}
    func nearbySendPinCode(pinCode: String) throws {}
    func nearbySkipUnlock() throws {}
    func nearbySkipVerification() throws {}
    func pen(sessionId: String, x: Double, y: Double, pressure: Double, rotation: Double, inRange: Bool, inContact: Bool) throws {}
    func reopen(sessionId: String) throws {}
    func requestAudioPermission(openSettings: Bool) async throws {}
    func requestKeyboardSettings() async throws {}
    func resendPermission() throws {}
    func resume(sessionId: String) async throws -> Bool {
        try record("resume \(sessionId)")
        return resumable
    }
    func setVolume(packageName: String, volume: Int32, muted: Bool) throws {}
    func switchLayout(sessionId: String, layout: Int32) throws { try record("switch \(sessionId) \(layout)") }
    func touch(sessionId: String, pointer: UInt64, phase: MirrorTouchPhase, x: Double, y: Double, pressure: Double, orientation: Double, width: Double, height: Double) throws {}
    func wake() throws {}
    func windowReady(sessionId: String) throws {}
}
