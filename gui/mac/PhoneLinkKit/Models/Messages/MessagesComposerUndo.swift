import AppKit

/// The composer text view's undo. One text view serves every draft, so it keeps an undo stack of its
/// own (not the window's, which other views share), and that stack is cleared whenever the text is
/// replaced in code — another draft shown, a draft sent or restored: an undo never reaches into text
/// the view no longer holds.
@MainActor
public final class MessagesComposerUndo {
    /// What the text view's delegate hands it (`undoManager(for:)`).
    public let undoManager = UndoManager()

    public init() {}

    /// Shows `text` in `textView` in place of what it held; the typing undone so far belonged to that.
    public func replaceText(of textView: NSTextView, with text: String) {
        textView.string = text
        undoManager.removeAllActions()
    }
}
