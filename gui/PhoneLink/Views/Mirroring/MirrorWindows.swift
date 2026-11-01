import AppKit
import Observation
import PhoneLinkEngine
import PhoneLinkKit
import SwiftUI

/// The mirroring windows: one per session the engine opened, each attached to the presenter as the
/// session's video sink, reporting its focus and its closing, titled and shaped after its session.
@MainActor
final class MirrorWindows {
    private let model: ScreenMirroringModel
    private let presenter: MirrorPresenterAdapter
    private let content: (MirrorWindowView) -> AnyView
    private var controllers: [String: MirrorWindowController] = [:]

    init(model: ScreenMirroringModel, presenter: MirrorPresenterAdapter, content: @escaping (MirrorWindowView) -> AnyView) {
        self.model = model
        self.presenter = presenter
        self.content = content

        model.presentWindow = { [weak self] id in self?.open(id) }
        model.dismissWindow = { [weak self] id in self?.close(id) }
        model.raiseWindow = { [weak self] id in self?.raise(id) }
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
            onClose: { [weak self] in self?.windowClosed(id) }
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

/// One mirroring window. Its content keeps the phone video's aspect (`MirrorWindowSizing`): the window
/// is placed when it opens — where the app's window was when it last closed, else three quarters of
/// the screen in the shape of the phone's resolution — and reshaped whenever the video's aspect
/// changes; until the video's size is known it keeps the shape it opened with.
@MainActor
final class MirrorWindowController: NSWindowController, NSWindowDelegate {
    let sessionId: String
    private let model: ScreenMirroringModel
    private let onClose: () -> Void
    private var closingFromEngine = false
    private var aspect: CGFloat?
    /// The window opened without knowing its shape: the first one known places it as a new window.
    private var needsInitialSize = false
    /// The frames the window had before its last reshapes, the latest last.
    private var earlierFrames: [CGRect] = []
    /// Where the window's frame is kept between windows of the same app.
    private let frameKey: String?
    private var observers: [NSObjectProtocol] = []

    init(sessionId: String, model: ScreenMirroringModel, content: AnyView, onClose: @escaping () -> Void) {
        self.sessionId = sessionId
        self.model = model
        self.onClose = onClose
        frameKey = model.session(sessionId).map { "MirrorWindowFrame.\($0.packageName)" }

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 400, height: 800),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.contentViewController = NSHostingController(rootView: content)
        window.isReleasedWhenClosed = false
        window.tabbingMode = .disallowed
        window.collectionBehavior.insert(.fullScreenPrimary)

        super.init(window: window)
        window.delegate = self
        place(window)
        follow()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("not used from a nib")
    }

    func show() {
        NSApp.setActivationPolicy(.regular)
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate()
        refit()
    }

    /// The content keeps its width and takes the aspect's height again, the top edge staying: the
    /// toolbar arrives with the content and takes part of the height it was placed with.
    private func refit() {
        guard let window, let aspect, !window.styleMask.contains(.fullScreen) else { return }
        let content = window.contentLayoutRect.size
        guard content.width > 0, abs(content.width / max(1, content.height) - aspect) > MirrorWindowSizing.aspectTolerance else { return }

        let chrome = Self.chrome(of: window)
        let height = (content.width / aspect).rounded() + chrome.height
        var frame = window.frame
        frame.origin.y = frame.maxY - height
        frame.size.height = height
        window.setFrame(frame, display: true)
    }

    /// The engine ended the session.
    func closeFromEngine() {
        closingFromEngine = true
        window?.close()
    }

    // MARK: - Size and place

    /// What the window adds around its content.
    private static func chrome(of window: NSWindow) -> CGSize {
        CGSize(width: window.frame.width - window.contentLayoutRect.width, height: window.frame.height - window.contentLayoutRect.height)
    }

    private static func visibleArea(of window: NSWindow) -> CGRect {
        (window.screen ?? NSScreen.main)?.visibleFrame ?? CGRect(x: 0, y: 0, width: 1440, height: 900)
    }

    /// The opening frame: the app's last one moved onto the screen it overlaps (unless its shape
    /// differs from the phone's resolution), else three quarters of the screen with the aspect known
    /// so far (the phone's resolution, else a phone's usual one).
    private func place(_ window: NSWindow) {
        let display = model.session(sessionId).map(MirrorDisplay.init)
        let initial = display?.aspectSize.map { $0.width / $0.height }
        let shape = initial ?? MirrorWindowSizing.defaultAspect
        let chrome = Self.chrome(of: window)
        let screens = NSScreen.screens.map { MirrorWindowSizing.Screen(frame: $0.frame, visible: $0.visibleFrame) }

        if let frameKey, let saved = UserDefaults.standard.string(forKey: frameKey).map(NSRectFromString),
           let restored = MirrorWindowSizing.restoredFrame(
               saved,
               screens: screens,
               chrome: chrome,
               deviceAspect: display?.deviceSize.map { $0.width / $0.height },
               rightToLeft: NSApp.userInterfaceLayoutDirection == .rightToLeft
           ) {
            window.setFrame(restored, display: false)
            aspect = (restored.width - chrome.width) / max(1, restored.height - chrome.height)
        } else {
            window.setFrame(MirrorWindowSizing.initialFrame(aspect: shape, visible: Self.visibleArea(of: window), chrome: chrome), display: false)
            aspect = shape
            needsInitialSize = initial == nil
        }

        constrain(window, to: aspect ?? shape)
    }

    /// The content keeps `aspect` while the user resizes the window, down to the smallest window
    /// for the screen it is on.
    private func constrain(_ window: NSWindow, to aspect: CGFloat) {
        let chrome = Self.chrome(of: window)
        window.contentAspectRatio = NSSize(width: aspect, height: 1)
        window.contentMinSize = MirrorWindowSizing.minimumContent(aspect: aspect, visible: Self.visibleArea(of: window), chrome: chrome)
    }

    // MARK: - Following the session

    /// Applies the session's title and shape now and on every change.
    private func follow() {
        withObservationTracking {
            apply()
        } onChange: { [weak self] in
            DispatchQueue.main.async {
                MainActor.assumeIsolated { self?.follow() }
            }
        }
    }

    private func apply() {
        guard let window, let session = model.session(sessionId) else { return }
        window.title = model.title(for: session)

        let display = MirrorDisplay(session)
        // Before the video's size is known the window keeps the shape it opened with: the phone's
        // resolution changing then does not reshape it.
        guard display.rotatedVideoSize != nil, let size = display.aspectSize, size.width > 0, size.height > 0 else { return }

        let shape = size.width / size.height
        if let aspect, abs(aspect - shape) <= MirrorWindowSizing.aspectTolerance { return }
        aspect = shape
        constrain(window, to: shape)

        guard !window.styleMask.contains(.fullScreen) else { return }
        let current = window.frame
        if needsInitialSize {
            needsInitialSize = false
            window.setFrame(
                MirrorWindowSizing.initialFrame(aspect: shape, visible: Self.visibleArea(of: window), chrome: Self.chrome(of: window)),
                display: true
            )
            return
        }

        let frame = MirrorWindowSizing.resizedFrame(
            aspect: shape,
            current: current,
            earlier: earlierFrames,
            visible: Self.visibleArea(of: window),
            chrome: Self.chrome(of: window)
        )
        earlierFrames = Array((earlierFrames + [current]).suffix(2))
        window.setFrame(frame, display: true)
    }

    // MARK: - NSWindowDelegate

    func windowDidBecomeKey(_ notification: Notification) {
        focus(.inFocus)
        model.windowActivated(sessionId)
    }

    /// On another screen the smallest window follows that screen's shape.
    func windowDidChangeScreen(_ notification: Notification) {
        guard let window, let aspect else { return }
        constrain(window, to: aspect)
    }

    /// Minimizing the window or hiding the app takes its key status: the phone hears "not in focus".
    func windowDidResignKey(_ notification: Notification) {
        focus(.notInFocus)
        inputView?.focusLost()
    }

    /// Zooming makes the window as large as the screen allows with the content's aspect.
    func windowWillUseStandardFrame(_ window: NSWindow, defaultFrame newFrame: NSRect) -> NSRect {
        guard let aspect else { return newFrame }
        return MirrorWindowSizing.zoomedFrame(aspect: aspect, visible: Self.visibleArea(of: window), chrome: Self.chrome(of: window))
    }

    func windowWillClose(_ notification: Notification) {
        for observer in observers {
            NotificationCenter.default.removeObserver(observer)
        }
        observers = []

        if let frameKey, let window, !window.styleMask.contains(.fullScreen) {
            UserDefaults.standard.set(NSStringFromRect(window.frame), forKey: frameKey)
        }

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
