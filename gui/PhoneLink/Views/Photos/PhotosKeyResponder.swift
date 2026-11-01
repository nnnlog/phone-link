import AppKit
import Carbon.HIToolbox
import PhoneLinkKit
import Quartz
import SwiftUI

/// What the grid's cells share with the AppKit view behind them: which photo the pointer is over
/// (the subject of a right-click) and the view to make first responder when a photo is clicked.
@MainActor
final class PhotosGridKeyboard {
    fileprivate weak var view: PhotosKeyView?

    /// The window the grid is in.
    var window: NSWindow? { view?.window }

    /// Keys and Edit menu commands go to the gallery from now on.
    func focus() {
        guard let view, let window = view.window else { return }
        window.makeFirstResponder(view)
    }

    /// The pointer entered or left a photo's cell; a cell that disappears reports leaving, as a cell
    /// removed under a resting pointer gets no hover exit.
    func pointer(over id: Int64, inside: Bool) {
        guard let view else { return }
        if inside {
            view.pointedID = id
        } else if view.pointedID == id {
            view.pointedID = nil
        }
    }
}

/// The AppKit view behind the grid: the gallery's first responder for the keyboard and the Edit menu,
/// Quick Look's controller in the responder chain, and the observer of right-clicks and of its
/// window closing.
struct PhotosKeyResponder: NSViewRepresentable {
    let model: PhotosModel
    let keyboard: PhotosGridKeyboard
    let columns: Int
    /// Read by the grid's body, so a change reaches `updateNSView`.
    let preview: PhotosPreview?

    func makeNSView(context: Context) -> PhotosKeyView {
        let view = PhotosKeyView(model: model)
        keyboard.view = view
        return view
    }

    func updateNSView(_ view: PhotosKeyView, context: Context) {
        view.columns = columns
        // Panel changes wait for the end of the view update: closing the panel ends its control,
        // which reports the viewer closed to the model.
        DispatchQueue.main.async { [preview] in
            view.show(preview)
        }
    }

    static func dismantleNSView(_ view: PhotosKeyView, coordinator: ()) {
        view.detach()
    }
}

final class PhotosKeyView: NSView {
    private let model: PhotosModel
    /// Columns of the grid, for the up and down arrows.
    var columns = 1
    /// The photo under the pointer.
    var pointedID: Int64?
    /// What the Quick Look panel shows while this view controls it.
    private var shown: PhotosPreview?
    private var mouseMonitor: Any?
    private var windowObservers: [NSObjectProtocol] = []

    init(model: PhotosModel) {
        self.model = model
        super.init(frame: .zero)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        preconditionFailure("PhotosKeyView is not archived")
    }

    override var acceptsFirstResponder: Bool { true }

    /// Clicks belong to the grid above.
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        stopWatching()
        if let window {
            watch(window)
        } else {
            releasePanel()
        }
    }

    /// The grid is gone: nothing may keep pointing at this view.
    func detach() {
        stopWatching()
        releasePanel()
    }

    // MARK: Keyboard and Edit menu

    override func keyDown(with event: NSEvent) {
        let modifiers = event.modifierFlags
            .intersection(.deviceIndependentFlagsMask)
            .subtracting([.numericPad, .function])

        switch Int(event.keyCode) {
        case kVK_Space where modifiers.isEmpty:
            model.toggleViewer()
        case kVK_LeftArrow where modifiers.isEmpty:
            model.moveSelection(by: -1)
        case kVK_RightArrow where modifiers.isEmpty:
            model.moveSelection(by: 1)
        case kVK_UpArrow where modifiers.isEmpty:
            model.moveSelection(by: -columns)
        case kVK_DownArrow where modifiers.isEmpty:
            model.moveSelection(by: columns)
        case kVK_ANSI_A where modifiers == .command:
            // Reaches here only when no Edit menu item answers Command-A.
            selectAll(nil)
        case kVK_Delete where modifiers.subtracting(.command).isEmpty,
             kVK_ForwardDelete where modifiers.subtracting(.command).isEmpty:
            delete(nil)
        default:
            super.keyDown(with: event)
        }
    }

    @objc func copy(_ sender: Any?) {
        let ids = model.selectedIDs
        if !ids.isEmpty {
            model.copy(ids)
        }
    }

    @objc func delete(_ sender: Any?) {
        let ids = model.deletable(model.selectedIDs)
        if !ids.isEmpty {
            model.delete(ids)
        }
    }

    @objc override func selectAll(_ sender: Any?) {
        model.selectAll()
    }

    // MARK: Right-click and window

    private func watch(_ window: NSWindow) {
        mouseMonitor = NSEvent.addLocalMonitorForEvents(matching: [.rightMouseDown, .leftMouseDown]) {
            [weak self] event in
            MainActor.assumeIsolated {
                self?.contextClick(event)
            }
            return event
        }

        let center = NotificationCenter.default
        windowObservers = [
            center.addObserver(forName: NSWindow.willCloseNotification, object: window, queue: .main) {
                [weak self] _ in
                MainActor.assumeIsolated {
                    self?.model.galleryLeft()
                }
            },
            center.addObserver(forName: NSWindow.didBecomeKeyNotification, object: window, queue: .main) {
                [weak self] _ in
                MainActor.assumeIsolated {
                    self?.model.galleryEntered()
                }
            },
        ]
    }

    private func stopWatching() {
        if let mouseMonitor {
            NSEvent.removeMonitor(mouseMonitor)
        }
        mouseMonitor = nil
        windowObservers.forEach(NotificationCenter.default.removeObserver)
        windowObservers = []
    }

    /// A right-click (or Control-click) on a photo is about to open its context menu: a photo outside
    /// the selection becomes the selection (one inside keeps it, and the menu acts on all of it), and
    /// the engine fetches its full image ahead of an action. The model ignores a photo that left the
    /// gallery under the pointer.
    private func contextClick(_ event: NSEvent) {
        guard event.window === window,
              event.type == .rightMouseDown || event.modifierFlags.contains(.control),
              let id = pointedID
        else {
            return
        }

        window?.makeFirstResponder(self)
        if !model.selection.contains(id) {
            model.select(id)
        }
        model.contextMenuOpened(id)
    }

    // MARK: Quick Look

    /// Brings the panel in line with the model when the preview changed: opens it on a new preview,
    /// reloads it for another photo or a new file of the same one, closes it when the preview is gone.
    func show(_ preview: PhotosPreview?) {
        guard preview != shown else { return }
        let previous = shown
        shown = preview

        guard let preview else {
            controlledPanel?.orderOut(nil)
            return
        }

        if let panel = controlledPanel, panel.isVisible {
            if previous?.id == preview.id {
                panel.refreshCurrentPreviewItem()
            } else {
                panel.reloadData()
            }
            return
        }

        guard let window, let panel = QLPreviewPanel.shared() else { return }
        // The panel takes the first responder of the key window that accepts to control it. It
        // notices key and main window changes itself, a new first responder only when told.
        window.makeFirstResponder(self)
        panel.updateController()
        panel.makeKeyAndOrderFront(nil)
        if (panel.currentController as AnyObject?) === self {
            panel.reloadData()
        }
    }

    /// The shared panel while this view controls it.
    private var controlledPanel: QLPreviewPanel? {
        guard QLPreviewPanel.sharedPreviewPanelExists(), let panel = QLPreviewPanel.shared(),
              (panel.currentController as AnyObject?) === self
        else {
            return nil
        }
        return panel
    }

    /// Lets go of the panel as the grid leaves its window: the panel holds its data source and
    /// delegate unretained, so neither may outlive this view. Deferred past the view update that
    /// removes the grid, because closing the panel reports the viewer closed to the model.
    private func releasePanel() {
        shown = nil
        DispatchQueue.main.async { [self] in
            guard let panel = controlledPanel else { return }
            panel.orderOut(nil)
            if (panel.dataSource as AnyObject?) === self {
                panel.dataSource = nil
                panel.delegate = nil
            }
        }
    }

    nonisolated override func acceptsPreviewPanelControl(_ panel: QLPreviewPanel!) -> Bool {
        MainActor.assumeIsolated { shown != nil }
    }

    nonisolated override func beginPreviewPanelControl(_ panel: QLPreviewPanel!) {
        MainActor.assumeIsolated {
            panel.dataSource = self
            panel.delegate = self
        }
    }

    nonisolated override func endPreviewPanelControl(_ panel: QLPreviewPanel!) {
        MainActor.assumeIsolated {
            panel.dataSource = nil
            panel.delegate = nil
            shown = nil
            model.closeViewer()
        }
    }
}

extension PhotosKeyView: @MainActor QLPreviewPanelDataSource, @MainActor QLPreviewPanelDelegate {
    func numberOfPreviewItems(in panel: QLPreviewPanel!) -> Int {
        shown == nil ? 0 : 1
    }

    func previewPanel(_ panel: QLPreviewPanel!, previewItemAt index: Int) -> (any QLPreviewItem)! {
        guard let shown else { return nil }
        return PhotosPreviewItem(url: shown.file, title: shown.title)
    }

    /// The arrow keys move the focus, and the panel follows it to the neighbouring photo.
    func previewPanel(_ panel: QLPreviewPanel!, handle event: NSEvent!) -> Bool {
        guard event.type == .keyDown,
              [kVK_LeftArrow, kVK_RightArrow, kVK_UpArrow, kVK_DownArrow].contains(Int(event.keyCode))
        else {
            return false
        }
        keyDown(with: event)
        return true
    }
}

extension PhotosKeyView: NSMenuItemValidation {
    func validateMenuItem(_ menuItem: NSMenuItem) -> Bool {
        switch menuItem.action {
        case #selector(copy(_:)):
            !model.selection.isEmpty
        case #selector(delete(_:)):
            !model.deletable(model.selectedIDs).isEmpty
        case #selector(selectAll(_:)):
            !model.items.isEmpty
        default:
            true
        }
    }
}

/// A photo for the Quick Look panel: its file (none while nothing is cached; the panel then shows
/// its loading state) and the photo's file name as the title. The panel's own controls (Share, Open
/// With, where macOS shows them) hand the file on without a file action's fetch, which is why the
/// model names it after the image its bytes hold.
private final class PhotosPreviewItem: NSObject, QLPreviewItem {
    let previewItemURL: URL?
    let previewItemTitle: String?

    init(url: URL?, title: String) {
        previewItemURL = url
        previewItemTitle = title
    }
}
