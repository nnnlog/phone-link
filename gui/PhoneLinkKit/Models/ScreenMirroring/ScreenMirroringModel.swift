import Foundation
import Observation
import os
import PhoneLinkEngine
import UniformTypeIdentifiers

private let log = AppLog(category: "mirroring")

/// Screen mirroring of the selected phone: its state and settings, its windows (one per session),
/// and what the user does in them.
///
/// The engine decides which windows exist: one opens for every session it reports opened — those
/// the user asked for and those the phone started — and closes when it reports the session closed.
/// The app shows the windows through `presentWindow`, `dismissWindow` and `raiseWindow`; a window
/// the user closes reports it with `windowClosed`. Every value shown is the engine's, read again on
/// every mirroring event.
@MainActor
@Observable
public final class ScreenMirroringModel {
    /// `nil` before the engine starts and while no phone is selected.
    public private(set) var status: MirroringStatusView?
    /// The windows' sessions, in the order they opened.
    public private(set) var sessions: [MirrorSessionView] = []
    /// Windows telling the user that their app does not let its audio be captured, for
    /// `captureNoticeLength`.
    public private(set) var captureNotices: Set<String> = []

    /// Opens the window of a session (set by the app).
    @ObservationIgnored public var presentWindow: (String) -> Void = { _ in }
    /// Closes the window of a session that ended (set by the app).
    @ObservationIgnored public var dismissWindow: (String) -> Void = { _ in }
    /// Brings the window of a session to the front (set by the app).
    @ObservationIgnored public var raiseWindow: (String) -> Void = { _ in }
    /// Brings the main window forward, for a launch the phone asked for that opens no window of its
    /// own (set by the app).
    @ObservationIgnored public var showMainWindow: () -> Void = {}
    /// Shows the main window's Apps section, for a launch the phone asked for (set by the app).
    @ObservationIgnored public var showApps: () -> Void = {}
    /// Tells the user an action failed (set by the app).
    @ObservationIgnored public var presentFailure: (SettingsFailure) -> Void = { _ in }
    /// Posts the drag notices as system notifications (set by the app).
    @ObservationIgnored public var notifier: (any SystemNotifier)?
    /// The user asks window `id` for screen layout `layout`, before the phone is asked (set by the app:
    /// the window keeps its frame for its layout and takes the one it last had in the other).
    @ObservationIgnored public var willSwitchLayout: (String, Int32) -> Void = { _, _ in }

    /// Sessions with an open window.
    @ObservationIgnored private var windows: Set<String> = []
    /// Each session's window state as last read.
    @ObservationIgnored private var lastState: [String: MirrorWindowState] = [:]
    /// Sessions whose window stayed connected for `backSettling`: their Back goes out at once.
    @ObservationIgnored private var backSettled: Set<String> = []
    /// The wait of each window that turned connected, until it settles or its state changes.
    @ObservationIgnored private var settlingWaits: [String: Task<Void, Never>] = [:]
    /// Sessions with a Back press waiting for their window to settle.
    @ObservationIgnored private var queuedBack: Set<String> = []
    /// How long a window must stay connected before its Back reaches the phone.
    public static let backSettling: Duration = .seconds(2)
    /// Whether each window's app lets its audio be captured, as last read.
    @ObservationIgnored private var captureAllowed: [String: Bool] = [:]
    /// The end of each window's capture notice.
    @ObservationIgnored private var captureNoticeEnds: [String: Task<Void, Never>] = [:]
    /// Windows that have been the active window.
    @ObservationIgnored private var activated: Set<String> = []
    /// The window whose drop of files over the limits is being told to the user.
    @ObservationIgnored private var overLimitNotice: String?
    /// How long a window tells the user its app does not let its audio be captured.
    public static let captureNoticeLength: Duration = .seconds(3)

    private let api: any ScreenMirroringApiProtocol
    /// A phone app's name from the phone's app list.
    private let appName: @MainActor (String) -> String?
    /// The selected phone's name.
    private let phoneName: @MainActor () -> String?

    public init(
        api: any ScreenMirroringApiProtocol,
        hub: EngineEventHub,
        appName: @escaping @MainActor (String) -> String?,
        phoneName: @escaping @MainActor () -> String?
    ) {
        self.api = api
        self.appName = appName
        self.phoneName = phoneName

        hub.onEvent { [weak self] event in
            self?.handle(event)
        }
        hub.onStart { [weak self] in
            self?.reload()
        }
    }

    // MARK: - What the views show

    /// Mirroring is offered: the phone screen and its apps can open on this Mac.
    public var isOffered: Bool {
        guard let status else { return false }
        return status.remotingSupported && status.phoneScreenApps && status.appsFeatureType != .disabled
    }

    public func session(_ id: String) -> MirrorSessionView? {
        sessions.first { $0.id == id }
    }

    /// The selected phone's name, for the windows' texts.
    public var phoneDisplayName: String {
        phoneName() ?? String(localized: "Your phone")
    }

    /// A window's title: the phone's name for the phone screen, else the app's name.
    public func title(for session: MirrorSessionView) -> String {
        if session.phoneScreen { return phoneDisplayName }
        return appName(session.packageName) ?? session.packageName
    }

    /// What a window shows over its video.
    public func overlay(for session: MirrorSessionView) -> MirrorOverlay? {
        MirrorOverlay.of(session, name: title(for: session), phoneName: phoneDisplayName)
    }

    /// The window takes input: its video shows and the phone accepts input.
    public func takesInput(_ session: MirrorSessionView) -> Bool {
        status?.inputSupported == true && session.state == .connected && session.videoReady
    }

    /// The audio button of the window's title bar, as the engine gives it: the volume once the app
    /// plays sound or is muted (whether or not this Mac streams audio), *Enable audio* while the
    /// connected window's audio is turned off on the phone, else none. None when the phone streams
    /// no audio.
    public func audioButton(_ session: MirrorSessionView) -> MirrorAudioButton {
        status?.audioSupported == true ? session.audioButton : .none
    }

    /// The window tells the user that its app does not let its audio be captured.
    public func showsCaptureNotice(_ id: String) -> Bool {
        captureNotices.contains(id)
    }

    /// The window offers the other layout: the phone has layouts, and the connected app window shows
    /// one of the two.
    public func offersLayouts(_ session: MirrorSessionView) -> Bool {
        status?.adaptiveLayout == true && !session.phoneScreen && session.state == .connected
            && (session.screenLayout == 1 || session.screenLayout == 2)
    }

    /// The layout the window's layout button switches to: Compact (1) from Expanded (2), else Expanded.
    public func otherLayout(_ session: MirrorSessionView) -> Int32 {
        session.screenLayout == 2 ? 1 : 2
    }

    /// The window shows Back: its phone screen or app is shown.
    public func showsBack(_ session: MirrorSessionView) -> Bool {
        session.state == .connected || session.state == .secureContent
    }

    /// The window shows its navigation bar (Recent Apps, Home, Back) under the video: the phone
    /// navigates by gestures. It shows whatever the window's state; its buttons act only while the
    /// window is connected (`navigationBarButton`).
    public func showsNavigationBar(_ session: MirrorSessionView) -> Bool {
        session.navigationButtons
    }

    /// A button of the navigation bar: the phone's key, sent only while the window is connected.
    public func navigationBarButton(_ id: String, _ button: MirrorNavButton) {
        guard session(id)?.state == .connected else {
            log.debug("navigation bar button not sent; the window is not connected")
            return
        }
        navButton(id, button)
    }

    /// The window's Back: the phone's Back key once the window has stayed connected for
    /// `backSettling`. A press before that waits for it (several make one) and is lost when the
    /// window's state changes first; protected content reached before it keeps them waiting.
    public func back(_ id: String) {
        guard let current = session(id), showsBack(current) else { return }
        if backSettled.contains(id) {
            navButton(id, .back)
        } else {
            queuedBack.insert(id)
        }
    }

    /// "Open on phone" for a notification of the list: a notification that opens on the phone (not
    /// a message or a call, which open here), while the phone screen and its apps open on this Mac.
    public func canOpen(_ notification: PhoneNotification) -> Bool {
        isOffered && notification.launchSupported && notification.openTarget == .notifications
    }

    // MARK: - Opening

    public func openPhoneScreen() {
        perform("Couldn't Open Your Phone's Screen", "phone screen not opened") { _ = try $0.openPhoneScreen() }
    }

    /// Opens a phone app, or a recent task of one, in a window.
    public func launch(_ target: PhoneAppLaunchTarget) {
        switch target {
        case let .app(packageName):
            perform("Couldn't Open the App", "phone app not opened") { _ = try $0.launchApp(packageName: packageName, transferTaskId: nil) }
        case let .task(packageName, taskId):
            perform("Couldn't Open the App", "phone app not opened") { _ = try $0.launchApp(packageName: packageName, transferTaskId: taskId) }
        }
    }

    /// Opens a notification on the phone, shown in its window.
    @discardableResult
    public func open(_ notification: PhoneNotification) -> Task<Void, Never> {
        let key = notification.key
        return Task { [api] in
            do {
                try await api.launchNotification(key: key, actionIndex: -1)
            } catch {
                self.fail("Couldn't Open on Your Phone", "notification not opened on the phone", error)
            }
        }
    }

    // MARK: - Windows

    /// The window of `id` can present video now.
    public func windowReady(_ id: String) {
        call("mirroring window ready not passed on") { try $0.windowReady(sessionId: id) }
    }

    /// The window of `id` gained or lost the keyboard focus, or was minimized or hidden;
    /// `capsLock` is the Mac's Caps Lock state now.
    public func focusChanged(_ id: String, _ focus: MirrorFocus, capsLock: Bool) {
        call("mirroring focus change not passed on") { try $0.focusChanged(sessionId: id, focus: focus, capsLock: capsLock) }
    }

    /// The window of `id` became the active window: a paused app opens again when the phone can
    /// resume it, and nothing is shown when it cannot. The first time, a capture notice shown since
    /// before then starts its time again.
    public func windowActivated(_ id: String) {
        if activated.insert(id).inserted, captureNotices.contains(id) {
            showCaptureNotice(id)
        }

        guard session(id)?.state == .suspended else { return }
        Task { [api] in
            do {
                let resumed = try await api.resume(sessionId: id)
                log.debug("paused mirroring window activated; resumed \(resumed)")
            } catch {
                log.notice("paused mirroring window not resumed: \(error: error); it stays paused")
            }
        }
    }

    /// The user closed the window of `id`.
    public func windowClosed(_ id: String) {
        guard windows.remove(id) != nil else { return }
        guard session(id) != nil else { return }
        call("mirroring window close not passed on") { try $0.closeSession(sessionId: id) }
    }

    /// A takeover's button.
    public func perform(_ action: MirrorOverlayAction, in id: String) {
        switch action {
        case .close:
            windows.remove(id)
            call("mirroring window close not passed on") { try $0.closeSession(sessionId: id) }
            dismissWindow(id)
        case .dismiss:
            perform("Couldn't Dismiss the Error", "mirroring error not dismissed") { try $0.dismissError(sessionId: id) }
        case .retry:
            perform("Couldn't Connect", "mirroring not retried") { try $0.retry() }
        case .allowAndRetry:
            guard var settings = status?.settings else { return }
            settings.allowApps = true
            perform("Couldn't Connect", "mirroring not retried with apps allowed") {
                try $0.setSettings(settings: settings)
                try $0.retry()
            }
        case .wake:
            perform("Couldn't Wake Your Phone", "phone not woken") { try $0.wake() }
        case .resendPermission:
            perform("Couldn't Ask Your Phone", "mirroring permission not asked again") { try $0.resendPermission() }
        case .resume:
            // In a paused window, going on past the touch confirmation launches the app again past
            // the phone's launch checks, which is what Resume does.
            perform("Couldn't Resume the App", "paused app not resumed") { try $0.continueTrueTouch(sessionId: id) }
        case .reopen:
            perform("Couldn't Reopen the App", "app not reopened") { try $0.reopen(sessionId: id) }
        case .continueTrueTouch:
            continueTrueTouch(id, doNotShowAgain: false)
        case let .audioPermission(openSettings):
            Task { [api] in
                do {
                    try await api.requestAudioPermission(openSettings: openSettings)
                } catch {
                    self.fail("Couldn't Ask Your Phone", "audio permission not requested", error)
                }
            }
        case .continueWithoutAudio:
            perform("Couldn't Continue", "mirroring not continued without audio") { try $0.continueWithoutAudio(sessionId: id) }
        }
    }

    /// Continue past the phone's touch confirmation; `doNotShowAgain` skips it from now on.
    public func continueTrueTouch(_ id: String, doNotShowAgain: Bool) {
        perform("Couldn't Continue", "touch confirmation not passed") { api in
            if doNotShowAgain, var settings = status?.settings {
                settings.doNotShowTrueTouch = true
                try api.setSettings(settings: settings)
            }
            try api.continueTrueTouch(sessionId: id)
        }
    }

    /// The answer to the phone's request to move the app to this Mac.
    public func answerMoveToMac(_ id: String, accept: Bool, doNotAsk: Bool) {
        perform("Couldn't Respond to Your Phone", "move-to-mac answer not sent") { try $0.answerMoveToVirtualDisplay(sessionId: id, accept: accept, doNotAsk: doNotAsk) }
    }

    /// Asks the phone for another layout of the app in window `id` (1 Compact, 2 Expanded, 0 the
    /// phone's default).
    public func switchLayout(_ id: String, to layout: Int32) {
        willSwitchLayout(id, layout)
        perform("Couldn't Change the Layout", "layout not switched") { try $0.switchLayout(sessionId: id, layout: layout) }
    }

    public func confirmLayoutSwitch(_ id: String, doNotAsk: Bool) {
        perform("Couldn't Change the Layout", "layout switch not confirmed") { try $0.confirmLayoutSwitch(sessionId: id, doNotAsk: doNotAsk) }
    }

    /// The title bar's *Enable audio*: the phone is asked again for the window's audio; the window
    /// then shows what the phone answered.
    public func enableAudio(_ id: String) {
        Task { [api] in
            do {
                try await api.enableAudio(sessionId: id)
            } catch {
                self.fail("Couldn't Ask Your Phone", "window audio not requested", error)
            }
        }
    }

    /// The app's volume (0–100) and mute; the phone applies it.
    public func setVolume(_ session: MirrorSessionView, volume: Int32, muted: Bool) {
        perform("Couldn't Change the Volume", "app volume not changed") {
            try $0.setVolume(packageName: session.packageName, volume: min(max(volume, 0), 100), muted: muted)
        }
    }

    /// The phone's keyboard settings, where its input language is chosen.
    public func requestKeyboardSettings() {
        Task { [api] in
            do {
                try await api.requestKeyboardSettings()
            } catch {
                self.fail("Couldn't Open Keyboard Settings", "phone keyboard settings not opened", error)
            }
        }
    }

    // MARK: - The nearby check

    public func nearbyRequestPinCode() {
        perform("Couldn't Ask Your Phone", "nearby code not requested") { try $0.nearbyRequestPinCode() }
    }

    public func nearbySendPinCode(_ pin: String) {
        let pin = pin.trimmingCharacters(in: .whitespaces)
        guard !pin.isEmpty else { return }
        perform("Couldn't Send the Code", "nearby code not sent") { try $0.nearbySendPinCode(pinCode: pin) }
    }

    public func nearbySkipUnlock() {
        perform("Couldn't Continue", "nearby unlock not skipped") { try $0.nearbySkipUnlock() }
    }

    public func nearbySkipVerification() {
        perform("Couldn't Continue", "nearby verification not skipped") { try $0.nearbySkipVerification() }
    }

    /// The prompt's code was shown again after it expired.
    public func nearbyCodeRefreshed() {
        call("nearby code refresh not passed on") { try $0.nearbyCodeRefreshed() }
    }

    // MARK: - Input

    public func mouseMove(_ id: String, x: Double, y: Double) {
        input { try $0.mouseMove(sessionId: id, x: x, y: y) }
    }

    public func mouseButton(_ id: String, _ button: MirrorMouseButton, down: Bool, x: Double, y: Double) {
        input { try $0.mouseButton(sessionId: id, button: button, down: down, x: x, y: y) }
    }

    /// One scroll at (`x`, `y`) normalized to the video, in wheel units (±120 per notch).
    public func mouseWheel(_ id: String, x: Double, y: Double, deltaX: Int32, deltaY: Int32) {
        input { try $0.mouseWheel(sessionId: id, x: x, y: y, deltaX: deltaX, deltaY: deltaY) }
    }

    /// Window `id` started or stopped taking input to the phone.
    public func inputFocus(_ id: String, focused: Bool, capsLock: Bool) {
        input { try $0.inputFocus(sessionId: id, focused: focused, capsLock: capsLock) }
    }

    /// A tablet pen at (`x`, `y`) normalized to the video: pressure 0–1, rotation in degrees.
    public func pen(_ id: String, x: Double, y: Double, pressure: Double, rotation: Double, inRange: Bool, inContact: Bool) {
        input {
            try $0.pen(sessionId: id, x: x, y: y, pressure: pressure, rotation: rotation, inRange: inRange, inContact: inContact)
        }
    }

    /// A Mac key; `flags` are the event's modifier flags after it (`NSEvent.ModifierFlags.rawValue`).
    public func key(_ id: String, keyCode: UInt16, _ action: MirrorKeyAction, flags: UInt) {
        input { try $0.key(sessionId: id, keyCode: keyCode, action: action, flags: UInt64(flags)) }
    }

    public func navButton(_ id: String, _ button: MirrorNavButton) {
        input { try $0.navButton(sessionId: id, button: button) }
    }

    // MARK: - Drag and drop

    /// Window `id` takes drops of Mac files: it is connected, and is not telling the user of a drop
    /// over the limits.
    public func acceptsDrops(_ id: String) -> Bool {
        session(id)?.state == .connected && overLimitNotice != id
    }

    /// A drag of the Mac files at `urls` entered window `id`: whether the window takes it.
    public func dragEnter(_ id: String, files urls: [URL]) -> MirrorDragEnter {
        let files = urls.map {
            MirrorDragFile(path: $0.path, mimeType: UTType(filenameExtension: $0.pathExtension)?.preferredMIMEType)
        }

        do {
            return try api.dragEnter(sessionId: id, files: files)
        } catch {
            log.debug("mirroring drag entry not passed on: \(error: error); the drag is refused")
            return .refused
        }
    }

    /// The drag is over window `id` at (`x`, `y`) normalized to the video: `true` while it shows copy.
    public func dragOver(_ id: String, x: Double, y: Double) -> Bool {
        (try? api.dragOver(sessionId: id, x: x, y: y)) ?? false
    }

    public func dragLeave(_ id: String) {
        input { try $0.dragLeave(sessionId: id) }
    }

    public func dragDrop(_ id: String) {
        input { try $0.dragDrop(sessionId: id) }
    }

    /// A refused drag over the limits was dropped on window `id`: the user is told, and the window
    /// takes no drops until the notice is gone.
    public func dragOverLimitDropped(_ id: String) {
        log.debug("dropped files refused; they are over the limits")
        overLimitNotice = id
        defer { overLimitNotice = nil }

        presentFailure(SettingsFailure(
            title: String(localized: "Can't Send These Files"),
            message: String(localized: "You can send up to 100 files at a time, each up to 500 MB and of a supported type.")
        ))
    }

    /// The phone app of window `id` took none of the files dropped on it: a notification names the app
    /// (an app window's), or says "this app".
    private func dragNotAccepted(_ id: String) {
        let app = session(id).flatMap { $0.phoneScreen ? nil : appName($0.packageName) }
        let body = app.map { String(localized: "\($0) can't receive files from this Mac.") }
            ?? String(localized: "This app can't receive files from this Mac.")

        let notification = SystemNotification(
            id: "screen_mirroring/dragNotAccepted/\(id)",
            threadId: "screen_mirroring/drag",
            title: String(localized: "Phone Link"),
            subtitle: nil,
            body: body,
            alert: true,
            actions: [],
            userInfo: [:],
            image: nil
        )

        do {
            try notifier?.post(notification: notification)
        } catch {
            log.error("drag-not-accepted notice not posted: \(error: error)")
        }
    }

    /// On every pointer move in window `id`: the phone's files to start a Mac drag of, when the phone
    /// handed its drag over. `buttonHeld` = a mouse button is down.
    public func takePhoneDrag(_ id: String, buttonHeld: Bool) -> [MirrorPhoneDragItem]? {
        do {
            return try api.takePhoneDrag(sessionId: id, buttonHeld: buttonHeld)
        } catch {
            log.debug("phone drag not read: \(error: error)")
            return nil
        }
    }

    /// The Mac drag of window `id`'s phone files ended; `dropped` when a destination took it.
    public func phoneDragEnded(_ id: String, dropped: Bool) {
        input { try $0.phoneDragEnded(sessionId: id, dropped: dropped) }
    }

    /// Puts the phone file `dataId` of window `id`'s drag at `url` once it arrived from the phone.
    public nonisolated func writePhoneDragItem(_ id: String, dataId: String, to url: URL) async throws {
        do {
            try await api.writePhoneDragItem(sessionId: id, dataId: dataId, path: url.path)
        } catch {
            // The drag's destination gets the error; it is logged here.
            log.error("dragged phone file not written: \(error: error)")
            throw error
        }
    }

    /// The Mac's audio output devices changed.
    public func audioOutputChanged() {
        call("audio output change not passed on") { try $0.audioOutputChanged() }
    }

    // MARK: - Settings

    /// Changes the phone's mirroring settings on this Mac.
    public func updateSettings(_ change: (inout MirrorSettings) -> Void) {
        guard var settings = status?.settings else { return }
        change(&settings)
        perform("Couldn't Change the Setting", "mirroring settings not changed") { try $0.setSettings(settings: settings) }
        reloadStatus()
    }

    // MARK: - Reading

    public func reload() {
        reloadStatus()
        reloadSessions()
    }

    private func reloadStatus() {
        do {
            status = try api.status()
        } catch let error as FeatureApiError where error == .NotStarted || error == .NoPhone {
            status = nil
        } catch {
            log.error("mirroring status not read: \(error: error)")
        }
    }

    /// Reads every session and matches the windows to them: a window for each session, none for a
    /// session that ended.
    private func reloadSessions() {
        let list: [MirrorSessionView]
        do {
            list = try api.sessions()
        } catch let error as FeatureApiError where error == .NotStarted || error == .NoPhone {
            list = []
        } catch {
            log.error("mirroring sessions not read: \(error: error)")
            return
        }

        sessions = list
        for session in list {
            noteState(session)
            noteCaptureAllowed(session)
        }

        let ids = Set(list.map(\.id))
        for id in Set(lastState.keys).union(captureAllowed.keys).subtracting(ids) {
            forget(id)
        }

        for id in windows.subtracting(ids) {
            windows.remove(id)
            dismissWindow(id)
        }

        for session in list where !windows.contains(session.id) {
            openWindow(session.id)
        }
    }

    private func reloadSession(_ id: String) {
        let view: MirrorSessionView?
        do {
            view = try api.session(sessionId: id)
        } catch {
            log.error("mirroring session not read: \(error: error)")
            return
        }

        if let view {
            noteState(view)
            noteCaptureAllowed(view)
            if let index = sessions.firstIndex(where: { $0.id == id }) {
                sessions[index] = view
            } else {
                sessions.append(view)
            }
        } else {
            sessionClosed(id)
        }
    }

    /// Follows the window's state for Back: any change ends the wait; turning connected starts it
    /// again, forgetting a waiting press, and when it ends the press waiting then goes out.
    private func noteState(_ view: MirrorSessionView) {
        let id = view.id
        guard lastState[id] != view.state else { return }
        lastState[id] = view.state
        settlingWaits.removeValue(forKey: id)?.cancel()

        guard view.state == .connected else { return }
        backSettled.remove(id)
        queuedBack.remove(id)

        settlingWaits[id] = Task { [weak self] in
            try? await Task.sleep(for: Self.backSettling)
            guard !Task.isCancelled, let self else { return }
            settlingWaits[id] = nil
            backSettled.insert(id)
            if queuedBack.remove(id) != nil {
                navButton(id, .back)
            }
        }
    }

    /// A window's app stops letting its audio be captured: the window says so for
    /// `captureNoticeLength`; allowing it again hides that. A window starts with it allowed.
    private func noteCaptureAllowed(_ view: MirrorSessionView) {
        let id = view.id
        let allowed = view.audioCaptureAllowed
        guard (captureAllowed[id] ?? true) != allowed else {
            captureAllowed[id] = allowed
            return
        }

        captureAllowed[id] = allowed
        guard !allowed else {
            hideCaptureNotice(id)
            return
        }

        showCaptureNotice(id)
    }

    /// Shows window `id`'s capture notice for `captureNoticeLength` from now; the window losing the
    /// focus does not hide it.
    private func showCaptureNotice(_ id: String) {
        captureNotices.insert(id)
        captureNoticeEnds.removeValue(forKey: id)?.cancel()
        captureNoticeEnds[id] = Task { [weak self] in
            try? await Task.sleep(for: Self.captureNoticeLength)
            guard !Task.isCancelled, let self else { return }
            captureNoticeEnds[id] = nil
            captureNotices.remove(id)
        }
    }

    private func hideCaptureNotice(_ id: String) {
        captureNoticeEnds.removeValue(forKey: id)?.cancel()
        captureNotices.remove(id)
    }

    /// Forgets what the model follows of window `id`.
    private func forget(_ id: String) {
        lastState[id] = nil
        settlingWaits.removeValue(forKey: id)?.cancel()
        backSettled.remove(id)
        queuedBack.remove(id)
        captureAllowed[id] = nil
        hideCaptureNotice(id)
        activated.remove(id)
    }

    private func openWindow(_ id: String) {
        windows.insert(id)
        presentWindow(id)
    }

    private func sessionClosed(_ id: String) {
        sessions.removeAll { $0.id == id }
        forget(id)
        if windows.remove(id) != nil {
            dismissWindow(id)
        }
    }

    private func clear() {
        status = nil
        for id in Set(lastState.keys).union(captureAllowed.keys) {
            forget(id)
        }
        sessions = []
        for id in windows {
            dismissWindow(id)
        }
        windows = []
    }

    private func handle(_ event: EngineEvent) {
        switch event {
        case let .screenMirroring(change):
            switch change {
            case .changed:
                reloadStatus()
            case let .sessionOpened(sessionId, _, _):
                reloadSession(sessionId)
                if session(sessionId) != nil, !windows.contains(sessionId) {
                    openWindow(sessionId)
                }
            case let .sessionChanged(sessionId):
                reloadSession(sessionId)
            case let .sessionClosed(sessionId):
                sessionClosed(sessionId)
            case let .bringToFront(sessionId):
                if windows.contains(sessionId) {
                    raiseWindow(sessionId)
                } else {
                    reloadSession(sessionId)
                    if session(sessionId) != nil { openWindow(sessionId) }
                }
            case let .dragNotAccepted(sessionId):
                dragNotAccepted(sessionId)
            case .dragEnded:
                break
            case .showMainWindow:
                showMainWindow()
            case .showApps:
                showApps()
            case let .notificationLaunched(_, opened):
                if !opened {
                    presentFailure(SettingsFailure(
                        title: String(localized: "Couldn't Open on Your Phone"),
                        message: String(localized: "\(phoneDisplayName) couldn't open the notification.")
                    ))
                }
            }
        case .resync(feature: .screenMirroring), .phoneChanged:
            reload()
        case .phoneSession:
            reloadStatus()
        case .accountRemoved, .phoneUnlinked:
            clear()
        default:
            break
        }
    }

    // MARK: - Calls

    /// A user's action: a failure is shown.
    private func perform(_ title: LocalizedStringResource, _ subject: StaticString, _ action: (any ScreenMirroringApiProtocol) throws -> Void) {
        do {
            try action(api)
        } catch {
            fail(title, subject, error)
        }
    }

    /// The app's own bookkeeping call: a failure is logged, nothing is shown.
    private func call(_ subject: StaticString, _ action: (any ScreenMirroringApiProtocol) throws -> Void) {
        do {
            try action(api)
        } catch {
            log.notice("\(subject.description, privacy: .public): \(error: error)")
        }
    }

    /// Input: the engine drops what it cannot send (no focus, no input channel); a failure is only
    /// worth a debug line.
    private func input(_ action: (any ScreenMirroringApiProtocol) throws -> Void) {
        do {
            try action(api)
        } catch {
            log.debug("mirroring input not passed on: \(error: error)")
        }
    }

    private func fail(_ title: LocalizedStringResource, _ subject: StaticString, _ error: Error) {
        log.error("\(subject.description, privacy: .public): \(error: error)")
        presentFailure(SettingsFailure(title: String(localized: title), message: UserText.describe(error)))
    }
}
