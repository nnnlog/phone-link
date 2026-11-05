import AppKit
import Observation
import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// The mirroring windows: one per session the engine opened, each attached to the presenter as the
/// session's video sink, reporting its focus and its closing, titled and shaped after its session.
/// Each window keeps its frame for its app's layout when the app quits.
@MainActor
final class MirrorWindows {
    private let model: ScreenMirroringModel
    private let presenter: MirrorPresenterAdapter
    private let content: (MirrorWindowView) -> AnyView
    private var controllers: [String: MirrorWindowController] = [:]
    private var terminating: NSObjectProtocol?

    init(model: ScreenMirroringModel, presenter: MirrorPresenterAdapter, content: @escaping (MirrorWindowView) -> AnyView) {
        self.model = model
        self.presenter = presenter
        self.content = content

        model.presentWindow = { [weak self] id in self?.open(id) }
        model.dismissWindow = { [weak self] id in self?.close(id) }
        model.raiseWindow = { [weak self] id in self?.raise(id) }
        model.willSwitchLayout = { [weak self] id, layout in self?.controllers[id]?.willSwitchLayout(to: layout) }

        terminating = NotificationCenter.default.addObserver(
            forName: NSApplication.willTerminateNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                for controller in self.controllers.values {
                    controller.saveFrame()
                }
            }
        }
    }

    private func open(_ id: String) {
        if controllers[id] != nil {
            raise(id)
            return
        }

        let video = MirrorVideoLayer()
        let controller = MirrorWindowController(
            sessionId: id,
            model: model,
            content: content(MirrorWindowView(sessionId: id, video: video)),
            onClose: { [weak self] in self?.windowClosed(id) },
            peers: { [weak self] in
                guard let self else { return [] }
                return controllers.filter { $0.key != id }.compactMap { $0.value.window }
            }
        )

        controllers[id] = controller
        presenter.attach(video, to: id)
        controller.show()
        model.windowReady(id)
    }

    private func close(_ id: String) {
        guard let controller = controllers.removeValue(forKey: id) else { return }
        presenter.detach(id)
        controller.closeFromEngine()
    }

    private func raise(_ id: String) {
        controllers[id]?.show()
    }

    /// The user closed the window.
    private func windowClosed(_ id: String) {
        guard controllers.removeValue(forKey: id) != nil else { return }
        presenter.detach(id)
        model.windowClosed(id)
    }
}

/// One mirroring window, sized and placed by `MirrorWindowSizing` and the frames its app's windows kept
/// (`MirrorWindowFrames`).
///
/// It opens in the frame its app's window last had in the app's last layout, as kept, when that
/// overlaps a screen; else as a new window. Either way it then steps away from another mirroring window
/// at its place, and reshapes for the video's aspect once that is known. Its frame is kept for the
/// layout it shows when it closes, when the user ends moving or resizing it, when the user switches its
/// layout, and when the app quits.
@MainActor
final class MirrorWindowController: NSWindowController, NSWindowDelegate {
    let sessionId: String
    private let model: ScreenMirroringModel
    private let onClose: () -> Void
    /// The other mirroring windows.
    private let peers: () -> [NSWindow]
    private let context: MirrorWindowContext
    private let frames = MirrorWindowFrames()
    /// The app whose frames the window keeps.
    private let package: String?
    private var closingFromEngine = false

    /// The video's aspect as last given to the window (the one it opened with before that).
    private var aspect = MirrorWindowSizing.defaultAspect
    /// The frame the window had before its last reshape, or the one it last had in the layout the user
    /// switched to.
    private var previous: MirrorWindowSizing.Placement?
    /// The layout the window last showed: what its frame is kept under.
    private var layout: Int32?
    /// The app's last layout on this Mac, as the engine last gave it: what the frame is kept under
    /// while the window shows no layout of its own.
    private var lastLayout: Int32?
    /// The title bar and toolbar's height, as last measured out of full screen.
    private var titleChrome: CGFloat = 0
    /// The navigation bar shows under the video; the window's height has taken it in; a change of it
    /// waits for the user to end resizing the window, or for the video's aspect.
    private var navigationBar = false
    private var navigationBarApplied = false
    private var navigationBarPending = false
    /// A reshape waits for the window to come back from the Dock, or for the user to end resizing it.
    private var reshapePending = false
    /// The size the user's resizing started from, and the edge they drag.
    private var resizeStart: CGSize?
    private var resizeEdge: MirrorWindowSizing.ResizeEdge?
    /// The user drags the window by its title bar.
    private var userMoving = false
    /// The window's size changed since its shape was last checked on a move.
    private var checkOnMove = true
    private var lastSize = CGSize.zero
    /// The controller is setting the window's frame, so the move and resize notifications that follow
    /// are not the user's.
    private var settingFrame = false
    /// The frame the window returns to from full screen.
    private var outOfFullScreen: CGRect?
    /// A new window's frame as placed, the aspect and visible area it was placed for: placed again when
    /// the toolbar arrives while the window still has it.
    private var placedAsNew: (frame: CGRect, aspect: CGFloat, visible: CGRect)?
    private var chromeObservation: NSKeyValueObservation?

    init(
        sessionId: String,
        model: ScreenMirroringModel,
        content: AnyView,
        onClose: @escaping () -> Void,
        peers: @escaping () -> [NSWindow] = { [] }
    ) {
        self.sessionId = sessionId
        self.model = model
        self.onClose = onClose
        self.peers = peers

        let session = model.session(sessionId)
        package = session?.packageName
        let volume = MirrorVolumePopup(
            state: MirrorVolumePopup.State(volume: session?.volume ?? 50, muted: session?.muted ?? false)
        ) { [weak model] change in
            guard let model, let session = model.session(sessionId) else { return }
            model.setVolume(session, volume: change.volume, muted: change.muted)
        }
        context = MirrorWindowContext(volume: volume)

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 400, height: 800),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        let hosting = NSHostingController(rootView: AnyView(content.environment(context)))
        // The window sets its size and its smallest size; the content fits whatever it is given.
        hosting.sizingOptions = []
        window.contentViewController = hosting
        window.isReleasedWhenClosed = false
        window.tabbingMode = .disallowed
        window.collectionBehavior.insert(.fullScreenPrimary)

        super.init(window: window)
        window.delegate = self
        // The content's first layout installs its toolbar, when it does so at once, before the
        // window's chrome is measured.
        window.contentView?.layoutSubtreeIfNeeded()
        place(window)
        follow()

        // A toolbar that arrives after the window was placed refits it.
        chromeObservation = window.observe(\.contentLayoutRect) { [weak self] _, _ in
            DispatchQueue.main.async {
                MainActor.assumeIsolated { self?.chromeChanged() }
            }
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("not used from a nib")
    }

    func show() {
        NSApp.setActivationPolicy(.regular)
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate()
        chromeChanged()
    }

    /// The engine ended the session.
    func closeFromEngine() {
        closingFromEngine = true
        window?.close()
    }

    // MARK: - Opening

    /// The opening frame: the kept one when it overlaps a screen, reshaped for the video when its
    /// aspect is known; else a new window's for the video's aspect, the phone's resolution's, or a
    /// phone's usual one. Then away from another mirroring window at its place.
    private func place(_ window: NSWindow) {
        let session = model.session(sessionId)
        let display = session.map(MirrorDisplay.init) ?? MirrorDisplay()
        titleChrome = window.frame.height - window.contentLayoutRect.height
        layout = session?.screenLayout
        lastLayout = session?.lastScreenLayout

        // The layout the window shows when the phone already said it, else the app's last one.
        let openingLayout = layout ?? lastLayout ?? MirrorWindowFrames.defaultLayout
        let screens = NSScreen.screens.map(\.frame)
        if let package,
           let saved = frames.openingFrame(package: package, lastLayout: openingLayout),
           let restored = MirrorWindowSizing.restoredFrame(saved, screens: screens) {
            setFrame(cascaded(restored))
            if let video = display.rotatedVideoSize {
                reshape(window, toVideo: video)
            }
        } else {
            let opening = display.openingSize.map { $0.width / $0.height } ?? MirrorWindowSizing.defaultAspect
            let visible = (NSScreen.main ?? NSScreen.screens.first)?.visibleFrame ?? window.frame
            let frame = MirrorWindowSizing.defaultFrame(aspect: opening, visible: visible, nonClient: nonClient(of: window))
            setFrame(cascaded(frame))
            placedAsNew = (window.frame, opening, visible)
        }

        aspect = MirrorWindowSizing.contentAspect(window.frame, nonClient: nonClient(of: window)) ?? aspect
        lastSize = window.frame.size
        constrain(window)
    }

    /// `frame` stepped away from the other mirroring windows on its screen.
    private func cascaded(_ frame: CGRect) -> CGRect {
        let screens = NSScreen.screens.map { MirrorWindowSizing.Screen(frame: $0.frame, visible: $0.visibleFrame) }
        return MirrorWindowSizing.cascaded(frame, screens: screens, peers: peers().map(\.frame))
    }

    // MARK: - Size and place

    /// What the window adds to its video area: the title bar and toolbar, and the navigation bar while
    /// it shows (`bar` stands for whether it does).
    private func nonClient(of window: NSWindow, bar: Bool? = nil) -> CGSize {
        let navigation = (bar ?? navigationBar) ? MirrorWindowSizing.navigationBarHeight : 0
        let width = window.styleMask.contains(.fullScreen) ? 0 : window.frame.width - window.contentLayoutRect.width
        return CGSize(width: width, height: titleChrome + navigation)
    }

    private func visibleArea(of window: NSWindow) -> CGRect {
        (window.screen ?? NSScreen.main)?.visibleFrame ?? CGRect(x: 0, y: 0, width: 1440, height: 900)
    }

    /// The smallest window the user can resize it to, for its aspect and its screen.
    private func constrain(_ window: NSWindow) {
        window.minSize = MirrorWindowSizing.minimumSize(aspect: aspect, visible: visibleArea(of: window), nonClient: nonClient(of: window))
    }

    private func setFrame(_ frame: CGRect) {
        guard let window else { return }
        settingFrame = true
        window.setFrame(frame, display: true)
        settingFrame = false

        if window.frame.size != lastSize {
            lastSize = window.frame.size
            checkOnMove = true
        }
    }

    /// Reshapes the window for the session's video, when its size is known.
    private func reshapeForVideo() {
        guard let window, let session = model.session(sessionId), let video = MirrorDisplay(session).rotatedVideoSize else { return }
        reshape(window, toVideo: video)
    }

    /// Reshapes the window for a video of `pixels` (turned), each pixel a backing pixel. Waits while
    /// the window is in the Dock or the user resizes it; in full screen, reshapes the frame the window
    /// returns to.
    private func reshape(_ window: NSWindow, toVideo pixels: CGSize) {
        guard pixels.width > 0, pixels.height > 0 else { return }
        if window.isMiniaturized || window.inLiveResize {
            reshapePending = true
            return
        }
        reshapePending = false

        let fullScreen = window.styleMask.contains(.fullScreen)
        let current = fullScreen ? (outOfFullScreen ?? window.frame) : window.frame
        let shown = aspect
        aspect = pixels.width / pixels.height

        let scale = window.backingScaleFactor
        let target = MirrorWindowSizing.reshaped(
            content: CGSize(width: pixels.width / scale, height: pixels.height / scale),
            current: current,
            fallbackAspect: shown,
            previous: previous,
            visible: visibleArea(of: window),
            nonClient: nonClient(of: window)
        )
        constrain(window)

        if let target {
            previous = MirrorWindowSizing.Placement(aspect: shown, frame: current)
            if target.size != current.size {
                if fullScreen {
                    outOfFullScreen = target
                } else {
                    setFrame(target)
                }
            }
        }
        applyNavigationBar()
    }

    /// The title bar and toolbar changed height. A new window that still has the frame it was placed
    /// with is placed again, so that it fits the visible area as a new window does; otherwise the
    /// video area takes the video's aspect again, the window's width and top edge kept.
    private func chromeChanged() {
        guard let window, !window.styleMask.contains(.fullScreen), !window.isMiniaturized else { return }
        let chrome = window.frame.height - window.contentLayoutRect.height
        guard chrome != titleChrome else { return }
        titleChrome = chrome
        constrain(window)
        guard !window.inLiveResize else { return }

        let nonClient = nonClient(of: window)
        if let placed = placedAsNew, placed.frame == window.frame {
            let frame = MirrorWindowSizing.defaultFrame(aspect: placed.aspect, visible: placed.visible, nonClient: nonClient)
            setFrame(cascaded(frame))
            placedAsNew = (window.frame, placed.aspect, placed.visible)
            aspect = MirrorWindowSizing.contentAspect(window.frame, nonClient: nonClient) ?? aspect
            constrain(window)
            return
        }

        guard let shown = MirrorWindowSizing.contentAspect(window.frame, nonClient: nonClient),
              !MirrorWindowSizing.sameAspect(shown, aspect)
        else { return }

        let height = ((window.frame.width - nonClient.width) / aspect).rounded() + nonClient.height
        setFrame(MirrorWindowSizing.heightChanged(
            window.frame,
            by: height - window.frame.height,
            aspect: aspect,
            visible: visibleArea(of: window),
            nonClient: nonClient
        ))
    }

    // MARK: - The navigation bar

    private func setNavigationBar(_ shown: Bool) {
        guard navigationBar != shown else { return }
        navigationBar = shown
        applyNavigationBar()
    }

    /// The window's height takes in, or gives back, the navigation bar's — not while the user resizes
    /// the window or it is in the Dock, nor for a bar that shows before the phone's screen size is
    /// known, nor when the window already holds it. A window in full screen keeps its frame.
    private func applyNavigationBar() {
        guard let window, navigationBarPending || navigationBarApplied != navigationBar else { return }
        if window.inLiveResize || window.isMiniaturized {
            navigationBarPending = true
            return
        }

        let bar = MirrorWindowSizing.navigationBarHeight
        var before = navigationBarApplied ? bar : 0
        let after = navigationBar ? bar : 0
        if after > before {
            guard let screen = model.session(sessionId).flatMap({ MirrorDisplay($0).shownSize }) else {
                navigationBarPending = true
                return
            }
            if MirrorWindowSizing.holdsNavigationBar(
                window.frame,
                nonClientWithoutBar: nonClient(of: window, bar: false),
                aspect: screen.width / screen.height
            ) {
                before = after
            }
        }

        navigationBarPending = false
        navigationBarApplied = navigationBar
        constrain(window)
        guard after != before, !window.styleMask.contains(.fullScreen) else { return }

        setFrame(MirrorWindowSizing.heightChanged(
            window.frame,
            by: after - before,
            aspect: aspect,
            visible: visibleArea(of: window),
            nonClient: nonClient(of: window)
        ))
    }

    // MARK: - Kept frames

    /// Keeps the window's frame (out of full screen) for the layout it shows.
    func saveFrame() {
        guard let window, let package else { return }
        let frame = window.styleMask.contains(.fullScreen) ? (outOfFullScreen ?? window.frame) : window.frame

        let layout = MirrorWindowFrames.saveLayout(current: layout, last: lastLayout)
        frames.save(frame, package: package, slot: MirrorWindowFrames.slot(layout))
    }

    /// The user asks for screen layout `target`: the frame kept for it becomes the one a reshape to its
    /// aspect returns to, and the window's frame is kept for the layout it shows now.
    func willSwitchLayout(to target: Int32) {
        guard let window, let package else { return }

        let nonClient = nonClient(of: window)
        if let kept = frames.frame(package: package, slot: MirrorWindowFrames.slot(target)),
           kept.width - nonClient.width > 0,
           let keptAspect = MirrorWindowSizing.contentAspect(kept, nonClient: nonClient),
           previous.map({ !MirrorWindowSizing.sameAspect($0.aspect, keptAspect) }) ?? true {
            previous = MirrorWindowSizing.Placement(aspect: keptAspect, frame: kept)
        }
        saveFrame()
    }

    // MARK: - Following the session

    /// Applies the session's title, navigation bar, volume and video shape now and on every change.
    private func follow() {
        let followed = withObservationTracking {
            model.session(sessionId).map { (session: $0, title: model.title(for: $0)) }
        } onChange: { [weak self] in
            DispatchQueue.main.async {
                MainActor.assumeIsolated { self?.follow() }
            }
        }

        guard let window, let (session, title) = followed else { return }
        window.title = title
        if let shown = session.screenLayout {
            layout = shown
        }
        lastLayout = session.lastScreenLayout
        context.volume.receive(MirrorVolumePopup.State(volume: session.volume, muted: session.muted))
        setNavigationBar(model.showsNavigationBar(session))
        reshapeForVideo()
    }

    // MARK: - NSWindowDelegate

    func windowDidBecomeKey(_ notification: Notification) {
        focus(.inFocus)
        model.windowActivated(sessionId)
        if reshapePending {
            reshapeForVideo()
        }
    }

    /// Minimizing the window or hiding the app takes its key status: the phone hears "not in focus".
    func windowDidResignKey(_ notification: Notification) {
        focus(.notInFocus)
        inputView?.focusLost()
    }

    /// Back from the Dock, the window takes the video's shape, and the navigation bar's height.
    func windowDidDeminiaturize(_ notification: Notification) {
        reshapeForVideo()
        applyNavigationBar()
    }

    /// On another screen the smallest window follows that screen.
    func windowDidChangeScreen(_ notification: Notification) {
        guard let window else { return }
        constrain(window)
    }

    func windowWillStartLiveResize(_ notification: Notification) {
        resizeStart = window?.frame.size
        resizeEdge = nil
        context.liveResizing = true
    }

    /// The user's resizing keeps the aspect: the side that follows the dragged one is recomputed, and
    /// the window stays within the visible area.
    func windowWillResize(_ sender: NSWindow, to frameSize: NSSize) -> NSSize {
        guard sender.inLiveResize, let start = resizeStart, !sender.styleMask.contains(.fullScreen) else { return frameSize }
        resizeEdge = MirrorWindowSizing.resizeEdge(proposed: frameSize, start: start, current: sender.frame.size, known: resizeEdge)
        return MirrorWindowSizing.liveResized(
            frameSize,
            edge: resizeEdge,
            aspect: aspect,
            visible: visibleArea(of: sender),
            nonClient: nonClient(of: sender)
        )
    }

    func windowDidEndLiveResize(_ notification: Notification) {
        resizeStart = nil
        resizeEdge = nil
        context.liveResizing = false
        if reshapePending {
            reshapeForVideo()
        }
        applyNavigationBar()
        saveFrame()
    }

    func windowDidResize(_ notification: Notification) {
        guard let window, !settingFrame, window.frame.size != lastSize else { return }
        lastSize = window.frame.size
        checkOnMove = true
    }

    func windowWillMove(_ notification: Notification) {
        userMoving = true
    }

    /// The first move after a size change checks the window's shape against the video; the end of the
    /// user's move keeps its frame.
    func windowDidMove(_ notification: Notification) {
        guard let window, !settingFrame else { return }
        if checkOnMove, window.frame.size == lastSize, !window.inLiveResize {
            checkOnMove = false
            reshapeForVideo()
        }

        guard userMoving else { return }
        saveFrame()
        if NSEvent.pressedMouseButtons == 0 {
            userMoving = false
        }
    }

    func windowWillEnterFullScreen(_ notification: Notification) {
        outOfFullScreen = window?.frame
    }

    /// Leaving full screen, the window takes the frame a reshape gave it meanwhile.
    func windowDidExitFullScreen(_ notification: Notification) {
        guard let window, let frame = outOfFullScreen else { return }
        outOfFullScreen = nil
        if frame != window.frame {
            setFrame(frame)
        }
        reshapeForVideo()
    }

    /// Zooming makes the window as large as the screen allows with the video's aspect.
    func windowWillUseStandardFrame(_ window: NSWindow, defaultFrame newFrame: NSRect) -> NSRect {
        MirrorWindowSizing.zoomedFrame(aspect: aspect, visible: visibleArea(of: window), nonClient: nonClient(of: window))
    }

    /// Closing drops a volume change not yet committed, and keeps the window's frame.
    func windowWillClose(_ notification: Notification) {
        context.volume.cancel()
        chromeObservation = nil
        saveFrame()

        if !closingFromEngine {
            onClose()
        }
    }

    private func focus(_ focus: MirrorFocus) {
        model.focusChanged(sessionId, focus, capsLock: NSEvent.modifierFlags.contains(.capsLock))
    }

    private var inputView: MirrorInputView? {
        func find(_ view: NSView) -> MirrorInputView? {
            if let view = view as? MirrorInputView { return view }
            for sub in view.subviews {
                if let found = find(sub) { return found }
            }
            return nil
        }
        return window?.contentView.flatMap(find)
    }
}
