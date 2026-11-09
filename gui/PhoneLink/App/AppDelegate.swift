import AppKit
import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

private let log = AppLog(category: "app")

/// Builds the app at launch — the OS edges, the engine, its controller and the feature models — and
/// owns what SwiftUI scenes cannot: the main window (opened at launch unless the user turned that off,
/// and on demand), the sign-in and send-files windows (on demand), the Dock icon and main menu (shown
/// only while a window is open), files opened with the app, the Mac's lock, unlock, sleep and wake
/// notifications, the app's active state, and a bounded stop of the phone session when it quits.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let hub: EngineEventHub
    let navigator: AppNavigator
    let edges: SystemEdges
    let controller: EngineController
    let notifications: NotificationsModel
    let deviceStatus: DeviceStatusModel
    let messages: MessagesModel
    let contacts: ContactsModel
    let calls: CallsModel
    let callControl: CallControlModel
    let photos: PhotosModel
    let wallpaper: WallpaperModel
    let remoteSnipping: RemoteSnippingModel
    let fileTransfer: FileTransferModel
    let phoneFilesDomains: PhoneFilesDomains
    let phoneFiles: PhoneFilesModel
    let settings: SettingsModel
    let remoteControl: RemoteControlModel
    let mediaControls: MediaControlsModel
    let hotspot: HotspotModel
    let connectedCamera: ConnectedCameraModel
    let pairing: PairingModel
    let phoneApps: PhoneAppsModel
    let mirroring: ScreenMirroringModel
    let updates: UpdateModel

    private var mainWindow: NSWindow?
    private var signInWindow: NSWindow?
    private var sendFilesWindow: NSWindow?
    private var pairingWindow: NSWindow?
    private var cameraWindow: NSPanel?
    private var incomingCallPanel: NSPanel?
    private var callWindow: NSPanel?
    private var updateWindow: NSWindow?
    private var mirrorWindows: MirrorWindows?
    private var observers: [(NotificationCenter, NSObjectProtocol)] = []
    /// Quitting was answered `terminateLater`; the reply is sent once.
    private var terminationRequested = false
    private var terminationReplied = false

    /// How long quitting waits for the phone session to end.
    private static let stopTimeout: Duration = .seconds(3)

    override init() {
        guard let bundleID = Bundle.main.bundleIdentifier else {
            preconditionFailure("the app runs from its bundle, which has an identifier")
        }

        Self.installLog(bundleID: bundleID)

        let hub = EngineEventHub()
        let navigator = AppNavigator()
        let edges = SystemEdges.live(keychainService: bundleID)
        let engine = Engine(
            store: edges.store,
            lock: edges.lock,
            host: edges.host,
            reachability: edges.reachability,
            events: EventBridge(hub: hub),
            notifier: edges.notifier,
            pasteboard: edges.pasteboard,
            imageCodec: edges.imageCodec,
            pcStatus: edges.pcStatus,
            attachmentImaging: edges.attachmentImaging,
            fileTypes: edges.fileTypes,
            proximity: edges.proximity,
            pcPower: edges.pcPower,
            pcWallpaper: edges.pcWallpaper,
            phoneBluetooth: edges.phoneBluetooth,
            hotspotWifi: edges.hotspotWifi,
            phoneCamera: edges.phoneCamera,
            phoneAppsHost: edges.phoneAppsHost,
            mirrorPresenter: edges.mirrorPresenter,
            dataDir: Self.dataDirectory(bundleID: bundleID).path
        )
        let controller = EngineController(engine: engine, hub: hub)

        self.hub = hub
        self.navigator = navigator
        self.edges = edges
        self.controller = controller
        notifications = NotificationsModel(api: controller.notifications, hub: hub, navigator: navigator)
        deviceStatus = DeviceStatusModel(api: controller.deviceStatus, hub: hub)
        messages = MessagesModel(api: controller.messaging, contacts: controller.contacts, hub: hub, navigator: navigator)
        contacts = ContactsModel(api: controller.contacts, hub: hub, navigator: navigator)
        calls = CallsModel(api: controller.calling, hub: hub, navigator: navigator)
        callControl = CallControlModel(
            api: controller.calling,
            messaging: controller.messaging,
            hub: hub,
            navigator: navigator,
            ringer: CallRinger()
        )
        photos = PhotosModel(api: controller.photos, hub: hub)
        wallpaper = WallpaperModel(api: controller.wallpaper, hub: hub)
        remoteSnipping = RemoteSnippingModel(api: controller.remoteSnipping, hub: hub)
        fileTransfer = FileTransferModel(api: controller.fileTransfer, hub: hub)
        let phoneFilesDomains = PhoneFilesDomains(api: controller.phoneFiles, hub: hub)
        self.phoneFilesDomains = phoneFilesDomains
        phoneFiles = PhoneFilesModel(api: controller.phoneFiles, hub: hub, domains: phoneFilesDomains)
        settings = SettingsModel(
            notifications: controller.notifications,
            messaging: controller.messaging,
            contacts: controller.contacts,
            calling: controller.calling,
            photos: controller.photos,
            clipboard: controller.clipboard,
            deviceStatus: controller.deviceStatus,
            phone: controller.phoneApi,
            hub: hub
        )
        remoteControl = RemoteControlModel(api: controller.remoteControl, hub: hub)
        mediaControls = MediaControlsModel(api: controller.audio, hub: hub)
        hotspot = HotspotModel(api: controller.hotspot, hub: hub)
        connectedCamera = ConnectedCameraModel(
            api: controller.connectedCamera,
            hub: hub,
            activator: CameraExtensionActivator(),
            camera: edges.phoneCamera
        )
        pairing = PairingModel(api: controller.pairing, hub: hub)
        let phoneApps = PhoneAppsModel(api: controller.phoneApps, hub: hub)
        self.phoneApps = phoneApps
        mirroring = ScreenMirroringModel(
            api: controller.screenMirroring,
            hub: hub,
            appName: { package in phoneApps.storedName(of: package) },
            phoneName: { controller.phone?.name }
        )
        updates = UpdateModel()

        super.init()

        updates.presentWindow = { [weak self] in self?.showUpdateWindow() }
        // The updater quits from a main-actor task. Quitting waits for applicationShouldTerminate's
        // reply in a nested run loop, and that reply comes from main-actor tasks, which cannot run while
        // the updater's task holds the main actor: the app would hang on "Installing". So the quit runs
        // from the run loop, after that task has returned.
        updates.terminate = { NSApp.perform(#selector(NSApplication.terminate(_:)), with: nil, afterDelay: 0) }

        mirrorWindows = MirrorWindows(model: mirroring, presenter: edges.mirrorPresenter) { [unowned self] view in
            AnyView(self.environments(view))
        }

        phoneApps.launcher = { [weak self] target in self?.mirroring.launch(target) }
        let notificationsApi = controller.notifications
        phoneApps.appNotificationsOn = { package in
            // An unreadable setting counts as on: the tile then shows the count it has.
            do {
                let flags = try notificationsApi.appSetting(packageName: package).flags
                return flags.feed || flags.banner
            } catch {
                log.debug("app notification setting not read: \(error: error); its tile shows the count")
                return true
            }
        }

        mirroring.presentFailure = { [weak self] failure in self?.showFailure(failure) }
        mirroring.notifier = edges.notifier
        mirroring.showMainWindow = { [weak self] in self?.showMainWindow() }
        mirroring.showApps = { [weak self] in self?.navigator.show(.apps) }
        edges.mirrorPresenter.onAudioOutputChanged = { [weak self] in self?.mirroring.audioOutputChanged() }

        pairing.onPaired = { [weak self] phoneId in
            guard let self else { return }
            controller.reloadPhones()
            controller.selectPhone(phoneId)
            pairingWindow?.close()
        }

        connectedCamera.presentControlWindow = { [weak self] shown in self?.showCameraWindow(shown) }
        callControl.presentPopup = { [weak self] shown in self?.showIncomingCallPanel(shown) }
        callControl.presentCallWindow = { [weak self] shown in self?.showCallWindow(shown) }
        callControl.canMessage = { [weak self] in self?.messages.isGated == false }
        hotspot.present = { [weak self] outcome in self?.showHotspotOutcome(outcome) }
        edges.hotspotWifi.onNetworkChanged = { [weak self] in self?.hotspot.reload() }

        navigator.presentWindow = { [weak self] in self?.showMainWindow() }
        navigator.presentSignIn = { [weak self] in self?.showSignInWindow() }
        navigator.presentSendFiles = { [weak self] in self?.showSendFilesWindow() }
        navigator.presentPairing = { [weak self] in self?.showPairingWindow() }
        navigator.presentBluetoothSettings = {
            if let url = URL(string: "x-apple.systempreferences:com.apple.BluetoothSettings") {
                NSWorkspace.shared.open(url)
            }
        }

        controller.onPhoneSelectionNeeded = { [weak self] in self?.showSignInWindow() }
        controller.onBringWindowForward = { [weak self] _ in self?.showMainWindow() }

        hub.onEvent { [weak self] event in
            if case .webSignInRequested = event { self?.showSignInWindow() }
        }
        hub.onStart { [weak self] in self?.signInWindow?.close() }
    }

    /// The app's log, in `~/Library/Logs/<bundle id>/`. A log that cannot be opened is reported to
    /// the unified log and the app runs without one.
    private static func installLog(bundleID: String) {
        do {
            try LogArchive.install(bundleID: bundleID)
        } catch {
            log.error("log file not opened: \(error: error); lines go to the unified log only")
        }
    }

    /// The features' stores: the app's folder in Application Support (the engine creates it 0700).
    private static func dataDirectory(bundleID: String) -> URL {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return support.appendingPathComponent(bundleID, isDirectory: true)
    }

    /// Applies every model and controller the views read.
    func environments<V: View>(_ view: V) -> some View {
        view
            .environment(controller)
            .environment(navigator)
            .environment(notifications)
            .environment(deviceStatus)
            .environment(messages)
            .environment(contacts)
            .environment(calls)
            .environment(callControl)
            .environment(photos)
            .environment(wallpaper)
            .environment(remoteSnipping)
            .environment(fileTransfer)
            .environment(phoneFiles)
            .environment(settings)
            .environment(remoteControl)
            .environment(mediaControls)
            .environment(hotspot)
            .environment(connectedCamera)
            .environment(pairing)
            .environment(phoneApps)
            .environment(mirroring)
            .environment(updates)
    }

    /// An action that failed, in an alert.
    private func showFailure(_ failure: SettingsFailure) {
        NSApp.activate()
        let alert = NSAlert()
        alert.messageText = failure.title
        alert.informativeText = failure.message
        alert.addButton(withTitle: String(localized: "OK"))
        alert.runModal()
    }

    /// An instant hotspot result, in an alert; one that carries the network offers to copy its password.
    private func showHotspotOutcome(_ outcome: HotspotModel.Outcome) {
        NSApp.activate()
        let alert = NSAlert()
        alert.messageText = outcome.title
        alert.informativeText = outcome.message
        alert.addButton(withTitle: String(localized: "OK"))
        if outcome.manualJoin != nil {
            alert.addButton(withTitle: String(localized: "Copy Password"))
        }

        if alert.runModal() == .alertSecondButtonReturn, let join = outcome.manualJoin {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(join.passphrase, forType: .string)
        }
    }

    // MARK: - Lifecycle

    func applicationDidFinishLaunching(_ notification: Notification) {
        edges.notifier.responseHandler = { [weak self] id, userInfo, response in
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    if let key = userInfo["key"] {
                        self?.showMainWindow()
                        self?.navigator.notificationRequest = key
                        self?.notifications.select(key)
                        if case .dismissed = response {
                            if let n = self?.notifications.notifications.first(where: { $0.key == key }) {
                                self?.notifications.dismiss(n)
                            }
                        } else if case let .reply(_, text) = response {
                            if let n = self?.notifications.notifications.first(where: { $0.key == key }) {
                                _ = self?.notifications.sendReply(text, to: n)
                            }
                        }
                    }
                    self?.controller.notificationResponse(id: id, userInfo: userInfo, response: response)
                }
            }
        }

        edges.notifier.activate()
        observeSystemEvents()
        observeWindows()
        connectedCamera.activateExtension()
        controller.start()
        updates.start()

        if UserDefaults.standard.object(forKey: PreferenceKeys.showWindowAtLaunch) as? Bool ?? true {
            showMainWindow()
        }
    }

    func applicationDidBecomeActive(_ notification: Notification) {
        controller.setForeground(true)
    }

    func applicationDidResignActive(_ notification: Notification) {
        controller.setForeground(false)
    }

    /// Files dropped on the app or opened with it go to the phone, reported by notifications; while
    /// the phone is not offered as a target, the send window says why.
    func application(_ application: NSApplication, open urls: [URL]) {
        let files = urls.filter(\.isFileURL)
        guard !files.isEmpty else { return }
        if fileTransfer.canSend {
            fileTransfer.sendFromOutside(files)
        } else {
            showSendFilesWindow()
        }
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        showMainWindow()
        return false
    }

    /// Ends the phone session, but never holds up quitting for long: the phone notices a vanished PC.
    /// The two paths race as separate tasks — a task group would wait for the stop anyway, and the
    /// engine's stop does not end when its Swift task is cancelled.
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard !terminationRequested else { return .terminateLater }
        terminationRequested = true

        Task {
            await controller.stop()
            replyToTermination()
        }
        Task {
            try? await Task.sleep(for: Self.stopTimeout)
            replyToTermination()
        }

        return .terminateLater
    }

    private func replyToTermination() {
        guard !terminationReplied else { return }
        terminationReplied = true
        NSApp.reply(toApplicationShouldTerminate: true)
    }

    private func observeSystemEvents() {
        let workspace = NSWorkspace.shared.notificationCenter
        observe(workspace, NSWorkspace.willSleepNotification) { $0.systemEvent(.willSleep) }
        observe(workspace, NSWorkspace.didWakeNotification) { $0.systemEvent(.didWake) }

        // AppKit suspends distributed notifications while the app is not active — nearly always, for a
        // menu bar app — and the block-based registration coalesces them until it is again; these two
        // must arrive when the screen locks or unlocks.
        let distributed = DistributedNotificationCenter.default()
        distributed.addObserver(
            self,
            selector: #selector(screenLocked(_:)),
            name: Notification.Name("com.apple.screenIsLocked"),
            object: nil,
            suspensionBehavior: .deliverImmediately
        )
        distributed.addObserver(
            self,
            selector: #selector(screenUnlocked(_:)),
            name: Notification.Name("com.apple.screenIsUnlocked"),
            object: nil,
            suspensionBehavior: .deliverImmediately
        )
    }

    @objc private func screenLocked(_ notification: Notification) {
        controller.systemEvent(.screenLocked)
    }

    @objc private func screenUnlocked(_ notification: Notification) {
        controller.systemEvent(.screenUnlocked)
    }

    private func observe(_ center: NotificationCenter, _ name: Notification.Name, _ action: @escaping @MainActor (EngineController) -> Void) {
        let token = center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                action(self.controller)
            }
        }
        observers.append((center, token))
    }

    // MARK: - Windows

    func showMainWindow() {
        let window = mainWindow ?? makeMainWindow()
        mainWindow = window
        present(window)
    }

    private func makeMainWindow() -> NSWindow {
        let window = MainWindowCustomWindow(contentViewController: NSHostingController(rootView: environments(MainWindow())))
        window.title = String(localized: "Phone Link")
        window.styleMask = [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView]
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.toolbar = nil
        window.isMovableByWindowBackground = true
        window.backgroundColor = NSColor(calibratedRed: 0.10, green: 0.09, blue: 0.08, alpha: 1)
        window.isOpaque = true
        window.hasShadow = true
        window.setContentSize(NSSize(width: 1140, height: 740))
        window.contentMinSize = NSSize(width: 900, height: 620)
        window.isReleasedWhenClosed = false
        window.setFrameAutosaveName("PhoneLinkMainWindow")
        window.center()
        return window
    }

    func showSignInWindow() {
        let window = signInWindow ?? makeSignInWindow()
        signInWindow = window
        present(window)
    }

    private func makeSignInWindow() -> NSWindow {
        let window = NSWindow(contentViewController: NSHostingController(rootView: environments(SignInView())))
        window.title = String(localized: "Sign In")
        window.styleMask = [.titled, .closable, .fullSizeContentView]
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.backgroundColor = NSColor(calibratedRed: 0.13, green: 0.12, blue: 0.11, alpha: 1)
        window.isOpaque = true
        window.isReleasedWhenClosed = false
        window.center()
        return window
    }

    /// The window that pairs a new phone (from Settings); closing it ends the session.
    func showPairingWindow() {
        // A finished pairing leaves the shared model on its "paired" page; opening the window for
        // another phone must start a fresh session, not show the previous one as already paired.
        pairing.cancel()
        let window = pairingWindow ?? makePairingWindow()
        pairingWindow = window
        present(window)
    }

    private func makePairingWindow() -> NSWindow {
        let view = PairingView(onCancel: { [weak self] in self?.pairingWindow?.close() })
            .frame(width: 480, height: 560)
            .onDisappear { [weak self] in self?.pairing.cancel() }
        let window = NSWindow(contentViewController: NSHostingController(rootView: environments(view)))
        window.title = String(localized: "Pair a New Phone")
        window.styleMask = [.titled, .closable, .fullSizeContentView]
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.isOpaque = true
        window.backgroundColor = NSColor(calibratedRed: 0.10, green: 0.09, blue: 0.08, alpha: 1)
        window.isReleasedWhenClosed = false
        window.center()
        return window
    }

    func showSendFilesWindow() {
        let window = sendFilesWindow ?? makeSendFilesWindow()
        sendFilesWindow = window
        present(window)
    }

    private func makeSendFilesWindow() -> NSWindow {
        let window = NSWindow(contentViewController: NSHostingController(rootView: environments(SendFilesView())))
        window.title = String(localized: "Send Files")
        window.styleMask = [.titled, .closable]
        window.isReleasedWhenClosed = false
        window.center()
        return window
    }

    /// The update window, opened by a check that found a release and by Check for Updates.
    func showUpdateWindow() {
        let window = updateWindow ?? makeUpdateWindow()
        updateWindow = window
        present(window)
    }

    private func makeUpdateWindow() -> NSWindow {
        let view = UpdateView(close: { [weak self] in self?.updateWindow?.close() })
        let window = NSWindow(contentViewController: NSHostingController(rootView: environments(view)))
        window.title = String(localized: "Software Update")
        window.styleMask = [.titled, .closable]
        window.isReleasedWhenClosed = false
        window.center()
        return window
    }

    /// The camera control window: a floating panel over the app that uses the camera, shown while the
    /// phone's camera streams; closing it closes the stream.
    private func showCameraWindow(_ shown: Bool) {
        guard shown else {
            cameraWindow?.orderOut(nil)
            return
        }

        let panel = cameraWindow ?? makeCameraWindow()
        cameraWindow = panel
        panel.orderFrontRegardless()
    }

    private func makeCameraWindow() -> NSPanel {
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 340, height: 200),
            styleMask: [.titled, .closable, .nonactivatingPanel, .utilityWindow],
            backing: .buffered,
            defer: true
        )
        panel.contentViewController = NSHostingController(rootView: environments(CameraControlView()))
        panel.title = controller.phone.map { String(localized: "\($0.name) Camera") } ?? String(localized: "Phone Camera")
        panel.isFloatingPanel = true
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.setFrameAutosaveName("PhoneLinkCameraWindow")
        panel.center()

        let close = NotificationCenter.default.addObserver(forName: NSWindow.willCloseNotification, object: panel, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.connectedCamera.closeControlWindow()
            }
        }
        observers.append((NotificationCenter.default, close))
        return panel
    }

    /// The incoming-call popup: a borderless panel at the top right of the screen, over every app and
    /// Space, shown while a call rings that the user has not hidden.
    private func showIncomingCallPanel(_ shown: Bool) {
        guard shown else {
            incomingCallPanel?.orderOut(nil)
            return
        }

        let panel = incomingCallPanel ?? makeIncomingCallPanel()
        incomingCallPanel = panel
        placeAtTopRight(panel)
        panel.orderFrontRegardless()
    }

    private func makeIncomingCallPanel() -> NSPanel {
        let panel = KeyablePanel(
            contentRect: NSRect(x: 0, y: 0, width: 376, height: 180),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: true
        )
        let host = NSHostingController(rootView: environments(IncomingCallView()))
        host.sizingOptions = [.preferredContentSize]
        panel.contentViewController = host
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.level = .statusBar
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.title = String(localized: "Incoming Call")

        // A second ringing call grows the panel; it stays at the top right.
        let resize = NotificationCenter.default.addObserver(forName: NSWindow.didResizeNotification, object: panel, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.placeAtTopRight(panel)
            }
        }
        observers.append((NotificationCenter.default, resize))
        return panel
    }

    private func placeAtTopRight(_ panel: NSPanel) {
        guard let screen = NSScreen.main ?? NSScreen.screens.first else { return }
        let visible = screen.visibleFrame
        let size = panel.frame.size
        panel.setFrameOrigin(NSPoint(x: visible.maxX - size.width - 12, y: visible.maxY - size.height - 12))
    }

    /// The call window: a floating panel for the calls placed or answered on this Mac. Closing it
    /// hides it; the calls go on on the phone.
    private func showCallWindow(_ shown: Bool) {
        guard shown else {
            callWindow?.orderOut(nil)
            return
        }

        let panel = callWindow ?? makeCallWindow()
        callWindow = panel
        panel.orderFrontRegardless()
    }

    private func makeCallWindow() -> NSPanel {
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 320, height: 300),
            styleMask: [.titled, .closable, .nonactivatingPanel, .utilityWindow],
            backing: .buffered,
            defer: true
        )
        let host = NSHostingController(rootView: environments(CallWindowView()))
        host.sizingOptions = [.preferredContentSize]
        panel.contentViewController = host
        panel.title = String(localized: "Phone Call")
        panel.isFloatingPanel = true
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.setFrameAutosaveName("PhoneLinkCallWindow")
        if !panel.setFrameUsingName("PhoneLinkCallWindow") {
            panel.center()
        }
        return panel
    }

    private func present(_ window: NSWindow) {
        NSApp.setActivationPolicy(.regular)
        window.makeKeyAndOrderFront(nil)
        NSApp.activate()
    }

    /// The Dock icon and the main menu follow the app's windows — the main, sign-in and Settings
    /// windows, not panels such as alerts: shown when one becomes key, hidden when the last one closes.
    private func observeWindows() {
        let center = NotificationCenter.default
        let key = center.addObserver(forName: NSWindow.didBecomeKeyNotification, object: nil, queue: .main) { note in
            let window = note.object as? NSWindow
            MainActor.assumeIsolated {
                if let window, Self.isAppWindow(window), NSApp.activationPolicy() != .regular {
                    NSApp.setActivationPolicy(.regular)
                }
            }
        }

        let close = center.addObserver(forName: NSWindow.willCloseNotification, object: nil, queue: .main) { note in
            let closing = note.object as? NSWindow
            MainActor.assumeIsolated {
                guard let closing, Self.isAppWindow(closing) else { return }
                let open = NSApp.windows.filter { $0 !== closing && $0.isVisible && Self.isAppWindow($0) }
                if open.isEmpty {
                    NSApp.setActivationPolicy(.accessory)
                }
            }
        }

        observers.append((center, key))
        observers.append((center, close))
    }

    private static func isAppWindow(_ window: NSWindow) -> Bool {
        window.styleMask.contains(.titled) && !(window is NSPanel)
    }
}

/// A borderless panel that takes clicks and keys like a titled one (a borderless window cannot become
/// key on its own), without activating the app.
private final class KeyablePanel: NSPanel {
    override var canBecomeKey: Bool { true }
}

/// The main app window: rejects any NSToolbar attached by child views so the window remains a pure unified
/// glass surface without transparent header artifacts.
private final class MainWindowCustomWindow: NSWindow {
    override var toolbar: NSToolbar? {
        get { nil }
        set { super.toolbar = nil }
    }
}
