import AppKit
import PhoneLinkEngine
import PhoneLinkKit
import QuartzCore
import UniformTypeIdentifiers

/// A mirroring window's video and input: shows the session's video layer laid out by its display
/// state, and sends the mouse, the scroll wheel, a tablet's pen and the keyboard to the phone.
///
/// The video is scaled the same way both ways to fit the view and centred (`MirrorDisplay.screenRect`).
/// A new video size or rotation after the first is shown behind a fade (`MirrorVideoTransitions`): the
/// video fades out, stays hidden a moment, takes its new layout, and fades in; until then positions
/// map to the layout shown.
///
/// Positions are normalized to the video as drawn, outside it included; the engine scales them to the
/// phone's resolution and decides what goes (a point outside the video only while a press that began
/// inside it is held). Keys go as Mac key codes with the
/// event's modifier flags, which the engine maps to what the phone receives. A key pressed with Command
/// is the Mac's: neither it nor its release is sent, so the Mac's own shortcuts (⌘W, ⌘Q) keep working
/// (Phone Link on Windows sends keys pressed while the Windows key is held). Releases are always
/// passed on, so a press the phone got is released even after the window stopped taking input; the
/// engine drops a release whose press it did not send.
///
/// Files dragged from the Mac onto the view go to the phone app under the drop, while the window is
/// connected and not telling the user of a drop over the limits (other drags are refused, and one in
/// progress leaves); files the phone drags past the video's edge become a Mac drag of file promises,
/// written where they are dropped once they arrived from the phone.
final class MirrorInputView: NSView {
    let sessionId: String
    let video: MirrorVideoLayer
    weak var model: ScreenMirroringModel?

    /// The session's display state, as last given.
    var display = MirrorDisplay() {
        didSet {
            guard display != oldValue else { return }
            switch transitions.receive(display.transform) {
            case .apply:
                show(display)
            case .fade:
                fadeToDisplay()
            }
        }
    }

    /// The display state the video is laid out by, and positions mapped with.
    private var shown = MirrorDisplay()
    private var transitions = MirrorVideoTransitions()
    private var fade: Task<Void, Never>?

    /// Input reaches the phone (the video shows and the phone takes input). Each change takes the input
    /// focus anew or drops it, releasing what is held.
    var inputEnabled = false {
        didSet {
            guard inputEnabled != oldValue else { return }
            model?.inputFocus(sessionId, focused: inputEnabled, capsLock: NSEvent.modifierFlags.contains(.capsLock))
        }
    }

    private var scroll = MirrorScrollAccumulator()
    /// Where the tablet pen last was.
    private var penPoint = CGPoint.zero
    private var trackingArea: NSTrackingArea?
    /// The drag over the view was refused for being over the limits.
    private var dragOverLimit = false
    /// The drag over the view was dropped on it.
    private var dragDropped = false
    /// The drag over the view entered while the window took drops; it left when the window stopped
    /// taking them.
    private var dragInside = false
    /// The file promises of the phone's files being dragged out (a promise holds its delegate weakly).
    private lazy var promises = MirrorPhoneDragPromises { [weak self, sessionId] dataId, url in
        guard let model = await self?.model else { throw CocoaError(.userCancelled) }
        try await model.writePhoneDragItem(sessionId, dataId: dataId, to: url)
    }

    init(sessionId: String, video: MirrorVideoLayer) {
        self.sessionId = sessionId
        self.video = video
        super.init(frame: .zero)

        wantsLayer = true
        layer?.backgroundColor = NSColor.black.cgColor
        layer?.masksToBounds = true
        layer?.addSublayer(video.layer)
        registerForDraggedTypes([.fileURL])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("not used from a nib")
    }

    // MARK: - Layout

    override func layout() {
        super.layout()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        defer { CATransaction.commit() }

        guard let rect = shown.screenRect(in: bounds.size) else {
            video.layer.isHidden = true
            return
        }
        video.layer.isHidden = false

        // The layer's own size is the video's unrotated one; the transform turns it into `rect`.
        let size = shown.swapsSides ? CGSize(width: rect.height, height: rect.width) : rect.size
        video.layer.bounds = CGRect(origin: .zero, size: size)
        // `rect` has its origin at the top; this view's y axis points up.
        video.layer.position = CGPoint(x: rect.midX, y: bounds.height - rect.midY)
        video.layer.setAffineTransform(CGAffineTransform(rotationAngle: shown.layerRotation))
    }

    private func show(_ display: MirrorDisplay) {
        shown = display
        needsLayout = true
    }

    /// Fades the video out, keeps it hidden, lays it out by the display state given last, and fades
    /// it in. A new transform meanwhile starts again from the video's opacity then.
    private func fadeToDisplay() {
        fade?.cancel()
        fade = Task { [weak self] in
            await self?.fadeVideo(to: 0)
            guard !Task.isCancelled else { return }
            try? await Task.sleep(for: MirrorVideoTransitions.hiddenLength)
            guard !Task.isCancelled, let self else { return }

            show(display)
            await fadeVideo(to: 1)
            if !Task.isCancelled {
                fade = nil
            }
        }
    }

    /// Animates the video's opacity from where it is to `opacity`, evenly over the fade's length.
    private func fadeVideo(to opacity: Float) async {
        let layer = video.layer
        let animation = CABasicAnimation(keyPath: "opacity")
        animation.fromValue = layer.presentation()?.opacity ?? layer.opacity
        animation.toValue = opacity
        animation.duration = MirrorVideoTransitions.fadeSeconds
        animation.timingFunction = CAMediaTimingFunction(name: .linear)
        layer.opacity = opacity
        layer.add(animation, forKey: "fade")
        try? await Task.sleep(for: MirrorVideoTransitions.fadeLength)
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let trackingArea { removeTrackingArea(trackingArea) }
        let area = NSTrackingArea(rect: .zero, options: [.mouseMoved, .activeInKeyWindow, .inVisibleRect], owner: self)
        addTrackingArea(area)
        trackingArea = area
    }

    // MARK: - Focus

    override var acceptsFirstResponder: Bool { true }

    /// The click that activates the window reaches the phone too.
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        window?.makeFirstResponder(self)
    }

    /// The window lost the keyboard focus: the engine released what was held.
    func focusLost() {
        scroll.reset()
    }

    // MARK: - Mouse

    /// `event`'s position normalized to the video as drawn (0–1 inside it).
    private func position(_ event: NSEvent) -> CGPoint? {
        let point = convert(event.locationInWindow, from: nil)
        return shown.normalized(CGPoint(x: point.x, y: bounds.height - point.y), in: bounds.size)
    }

    private func isPen(_ event: NSEvent) -> Bool {
        event.subtype == .tabletPoint
    }

    private func move(_ event: NSEvent) {
        startPhoneDrag(event)

        if isPen(event) {
            pen(event, inContact: event.type == .leftMouseDragged)
            return
        }

        guard inputEnabled, let model, let p = position(event) else { return }
        model.mouseMove(sessionId, x: p.x, y: p.y)
    }

    private func button(_ event: NSEvent, _ button: MirrorMouseButton, down: Bool) {
        // The pen's tip is its contact; its side buttons are not sent.
        if isPen(event) {
            if button == .left { pen(event, inContact: down) }
            return
        }

        guard let model, let p = position(event) else { return }
        // A press is sent only while the phone takes input; a release always, for the engine to match.
        if down, !inputEnabled { return }
        model.mouseButton(sessionId, button, down: down, x: p.x, y: p.y)
    }

    private func otherButton(_ event: NSEvent) -> MirrorMouseButton {
        event.buttonNumber == 2 ? .middle : .other
    }

    override func mouseMoved(with event: NSEvent) { move(event) }
    override func mouseDragged(with event: NSEvent) { move(event) }
    override func rightMouseDragged(with event: NSEvent) { move(event) }
    override func otherMouseDragged(with event: NSEvent) { move(event) }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        button(event, .left, down: true)
    }

    override func mouseUp(with event: NSEvent) { button(event, .left, down: false) }
    override func rightMouseDown(with event: NSEvent) { button(event, .right, down: true) }
    override func rightMouseUp(with event: NSEvent) { button(event, .right, down: false) }
    override func otherMouseDown(with event: NSEvent) { button(event, otherButton(event), down: true) }
    override func otherMouseUp(with event: NSEvent) { button(event, otherButton(event), down: false) }

    /// A scroll is one wheel event at the pointer. The momentum that follows a trackpad swipe after
    /// the fingers lift is not sent, so the phone stops scrolling when the fingers lift.
    override func scrollWheel(with event: NSEvent) {
        guard inputEnabled, let model, let p = position(event) else { return }
        guard event.momentumPhase.isEmpty else { return }

        if event.phase == .began || event.phase == .mayBegin {
            scroll.reset()
        }

        guard let wheel = scroll.add(
            deltaX: Double(event.scrollingDeltaX),
            deltaY: Double(event.scrollingDeltaY),
            precise: event.hasPreciseScrollingDeltas,
            pointsPerLine: Self.pointsPerLine
        ) else { return }
        model.mouseWheel(sessionId, x: p.x, y: p.y, deltaX: wheel.x, deltaY: wheel.y)
    }

    /// The system's points per scroll line (Quartz documents 10 as its default).
    private static var pointsPerLine: Double {
        CGEventSource(stateID: .hidSystemState).map { Double($0.pixelsPerLine) } ?? 10
    }

    // MARK: - Tablet pen

    override func tabletProximity(with event: NSEvent) {
        guard let model else { return }
        if event.isEnteringProximity {
            guard inputEnabled else { return }
            model.pen(sessionId, x: penPoint.x, y: penPoint.y, pressure: 0, rotation: 0, inRange: true, inContact: false)
        } else {
            model.pen(sessionId, x: penPoint.x, y: penPoint.y, pressure: 0, rotation: 0, inRange: false, inContact: false)
        }
    }

    /// A tablet event as the pen: its position, pressure and rotation, in range while near the
    /// tablet (a tablet event implies it), in contact while its tip is down.
    private func pen(_ event: NSEvent, inContact: Bool) {
        guard let model, let p = position(event) else { return }
        penPoint = p
        if inContact, !inputEnabled { return }

        let rotation = Double(event.rotation).truncatingRemainder(dividingBy: 360)
        model.pen(
            sessionId,
            x: p.x,
            y: p.y,
            pressure: Double(event.pressure),
            rotation: rotation < 0 ? rotation + 360 : rotation,
            inRange: true,
            inContact: inContact
        )
    }

    // MARK: - Drag and drop: Mac files onto the phone

    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
        dragOverLimit = false
        dragDropped = false
        dragInside = false
        return enter(sender)
    }

    /// The drag enters a window that takes drops.
    private func enter(_ sender: NSDraggingInfo) -> NSDragOperation {
        guard let model, model.acceptsDrops(sessionId) else { return [] }
        dragInside = true

        let urls = sender.draggingPasteboard.readObjects(
            forClasses: [NSURL.self],
            options: [.urlReadingFileURLsOnly: true]
        ) as? [URL] ?? []
        guard !urls.isEmpty else { return [] }

        switch model.dragEnter(sessionId, files: urls) {
        case .accepted:
            return dragOver(sender)
        case .overLimit:
            dragOverLimit = true
            return []
        case .refused:
            return []
        }
    }

    /// A window that stops taking drops makes the drag over it leave; one that takes them again makes
    /// it enter anew.
    override func draggingUpdated(_ sender: NSDraggingInfo) -> NSDragOperation {
        guard let model else { return [] }
        let takes = model.acceptsDrops(sessionId)
        if dragInside, !takes {
            dragInside = false
            model.dragLeave(sessionId)
            return []
        }
        if !dragInside {
            return takes ? enter(sender) : []
        }
        return dragOver(sender)
    }

    /// The drag's position goes to the phone as a mouse move; outside the video (or before it shows)
    /// as a point the engine does not send.
    private func dragOver(_ sender: NSDraggingInfo) -> NSDragOperation {
        guard let model else { return [] }
        let point = convert(sender.draggingLocation, from: nil)
        let p = shown.normalized(CGPoint(x: point.x, y: bounds.height - point.y), in: bounds.size) ?? CGPoint(x: -1, y: -1)
        return model.dragOver(sessionId, x: p.x, y: p.y) ? .copy : []
    }

    override func draggingExited(_ sender: NSDraggingInfo?) {
        guard dragInside else { return }
        dragInside = false
        model?.dragLeave(sessionId)
    }

    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        guard dragInside else { return false }
        dragDropped = true
        model?.dragDrop(sessionId)
        return true
    }

    /// A drag over the limits dropped on the view tells the user why nothing happened.
    override func draggingEnded(_ sender: NSDraggingInfo) {
        defer {
            dragOverLimit = false
            dragInside = false
        }
        guard dragOverLimit, !dragDropped else { return }
        if bounds.contains(convert(sender.draggingLocation, from: nil)) {
            model?.dragOverLimitDropped(sessionId)
        }
    }

    // MARK: - Drag and drop: phone files onto the Mac

    /// Every pointer move first asks whether the phone handed a drag over; with one, a Mac drag of its
    /// files starts from this move (a move without a button held ends it at once in the engine).
    private func startPhoneDrag(_ event: NSEvent) {
        guard let model else { return }
        let held = [.leftMouseDragged, .rightMouseDragged, .otherMouseDragged].contains(event.type)
        guard let items = model.takePhoneDrag(sessionId, buttonHeld: held) else { return }
        guard !items.isEmpty else {
            model.phoneDragEnded(sessionId, dropped: false)
            return
        }

        let types = items.map { item in
            UTType(mimeType: item.mimeType)
                ?? UTType(filenameExtension: (item.name as NSString).pathExtension)
                ?? .data
        }

        // One 128-point image centred on the pointer: the file type's icon for one file, a stack of
        // documents for several; the other items carry no image of their own.
        let origin = convert(event.locationInWindow, from: nil)
        let frame = NSRect(x: origin.x - 64, y: origin.y - 64, width: 128, height: 128)
        let image = items.count == 1
            ? NSWorkspace.shared.icon(for: types[0])
            : NSImage(named: NSImage.multipleDocumentsName) ?? NSWorkspace.shared.icon(for: .data)

        let dragging = zip(items, types).enumerated().map { index, pair in
            let provider = NSFilePromiseProvider(fileType: pair.1.identifier, delegate: promises)
            provider.userInfo = pair.0
            let draggingItem = NSDraggingItem(pasteboardWriter: provider)
            draggingItem.setDraggingFrame(frame, contents: index == 0 ? image : nil)
            return draggingItem
        }

        beginDraggingSession(with: dragging, event: event, source: self)
    }

    // MARK: - Keyboard

    override func keyDown(with event: NSEvent) {
        guard inputEnabled, let model else {
            // A takeover's buttons answer Return and Escape.
            super.keyDown(with: event)
            return
        }

        // Command shortcuts are the Mac's: a key pressed with Command is not sent (its release then
        // finds no press in the engine and is dropped).
        guard !event.modifierFlags.contains(.command) else { return }
        model.key(sessionId, keyCode: event.keyCode, event.isARepeat ? .repeat : .down, flags: event.modifierFlags.rawValue)
    }

    override func keyUp(with event: NSEvent) {
        model?.key(sessionId, keyCode: event.keyCode, .up, flags: event.modifierFlags.rawValue)
    }

    override func flagsChanged(with event: NSEvent) {
        guard let model else { return }
        let code = event.keyCode
        let flags = event.modifierFlags.rawValue

        switch MirrorModifierKeys.change(keyCode: code, flags: flags) {
        case .down?:
            guard inputEnabled else { return }
            model.key(sessionId, keyCode: code, .down, flags: flags)
        case .up?:
            model.key(sessionId, keyCode: code, .up, flags: flags)
        case .toggle?:
            guard inputEnabled else { return }
            model.key(sessionId, keyCode: code, .down, flags: flags)
            model.key(sessionId, keyCode: code, .up, flags: flags)
        case nil:
            break
        }
    }
}

extension MirrorInputView: NSDraggingSource {
    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        .copy
    }

    func draggingSession(_ session: NSDraggingSession, endedAt screenPoint: NSPoint, operation: NSDragOperation) {
        model?.phoneDragEnded(sessionId, dropped: !operation.isEmpty)
    }
}

/// The file promises of a drag of phone files: each file's name, and its bytes put where the
/// destination asks once they arrived from the phone. A promise's `userInfo` is its
/// `MirrorPhoneDragItem`.
final class MirrorPhoneDragPromises: NSObject, NSFilePromiseProviderDelegate {
    private let write: @Sendable (String, URL) async throws -> Void
    private let queue: OperationQueue = {
        let queue = OperationQueue()
        queue.qualityOfService = .userInitiated
        return queue
    }()

    /// `write(dataId, url)` puts phone file `dataId` at `url`.
    init(write: @escaping @Sendable (String, URL) async throws -> Void) {
        self.write = write
    }

    func filePromiseProvider(_ filePromiseProvider: NSFilePromiseProvider, fileNameForType fileType: String) -> String {
        (filePromiseProvider.userInfo as? MirrorPhoneDragItem)?.name ?? "_"
    }

    func filePromiseProvider(
        _ filePromiseProvider: NSFilePromiseProvider,
        writePromiseTo url: URL,
        completionHandler: @escaping (Error?) -> Void
    ) {
        guard let item = filePromiseProvider.userInfo as? MirrorPhoneDragItem else {
            completionHandler(CocoaError(.fileWriteUnknown))
            return
        }

        // The queue's thread waits for the write: the destination reports the file once it exists.
        let done = DispatchSemaphore(value: 0)
        let failure = MirrorWriteFailure()
        let write = self.write
        Task.detached {
            do {
                try await write(item.dataId, url)
            } catch {
                failure.error = error
            }
            done.signal()
        }

        done.wait()
        completionHandler(failure.error)
    }

    func operationQueue(for filePromiseProvider: NSFilePromiseProvider) -> OperationQueue {
        queue
    }
}

/// A write's error, handed from its task to the waiting queue thread.
private final class MirrorWriteFailure: @unchecked Sendable {
    var error: Error?
}
