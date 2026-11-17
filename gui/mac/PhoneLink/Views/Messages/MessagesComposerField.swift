import AppKit
import PhoneLinkKit
import SwiftUI
import UniformTypeIdentifiers

private let log = AppLog(category: "messages")

/// The composer's text field. It is an `NSTextView` because a SwiftUI `TextField` cannot take a pasted
/// image: its field editor owns Paste. Return sends, Option-Return starts a new line, Escape goes to
/// `onEscape` when there is one, and the field grows to five lines before it scrolls. Dropped files are
/// left to the conversation's drop destination, which attaches images.
///
/// One text view serves every draft, so its undo is `MessagesComposerUndo`'s: a stack of its own,
/// cleared whenever the text is replaced in code.
struct MessagesComposerField: NSViewRepresentable {
    let text: String
    let placeholder: String
    let isEditable: Bool
    let onChange: (String) -> Void
    let onSubmit: () -> Void
    /// Escape outside an input method's composition; `true` when it was handled.
    let onEscape: (() -> Bool)?
    /// Takes pasted images; `nil` where images are not offered (Paste then pastes text only).
    let onPasteImages: (([MessagesAttachment]) -> Void)?

    private static let maxLines = 5
    private static let idealWidth: CGFloat = 240

    func makeCoordinator() -> Coordinator {
        Coordinator(parent: self)
    }

    func makeNSView(context: Context) -> NSScrollView {
        let textView = MessagesComposerTextView(usingTextLayoutManager: false)
        textView.delegate = context.coordinator
        textView.isRichText = false
        textView.importsGraphics = false
        textView.allowsUndo = true
        textView.isContinuousSpellCheckingEnabled = true
        textView.drawsBackground = false
        textView.font = MessagesComposerTextView.baseFont
        textView.textContainerInset = NSSize(width: 4, height: 6)
        textView.minSize = .zero
        textView.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: .greatestFiniteMagnitude)
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = false
        textView.autoresizingMask = .width
        textView.textContainer?.widthTracksTextView = true
        textView.setAccessibilityLabel(String(localized: "Message"))
        textView.updateDragTypeRegistration()

        let scrollView = NSScrollView()
        scrollView.drawsBackground = false
        scrollView.hasVerticalScroller = true
        scrollView.autohidesScrollers = true
        scrollView.documentView = textView
        return scrollView
    }

    func updateNSView(_ scrollView: NSScrollView, context: Context) {
        context.coordinator.parent = self
        guard let textView = scrollView.documentView as? MessagesComposerTextView else { return }

        if textView.string != text {
            context.coordinator.undo.replaceText(of: textView, with: text)
            textView.restoreTypingFontIfEmpty()
            textView.needsDisplay = true
        }

        // Only on a change: restyling the text on every keystroke would disturb an input method's
        // marked text.
        if textView.isEditable != isEditable {
            textView.isEditable = isEditable
            textView.textColor = isEditable ? .textColor : .disabledControlTextColor
        }

        textView.placeholder = placeholder
        textView.setAccessibilityPlaceholderValue(placeholder)
        textView.onPasteImages = onPasteImages
    }

    /// As wide as offered; one line high, growing with `text` up to `maxLines`. The text is measured in
    /// its own layout objects, so a width SwiftUI only asks about never reaches the shown field.
    func sizeThatFits(_ proposal: ProposedViewSize, nsView: NSScrollView, context: Context) -> CGSize? {
        guard let textView = nsView.documentView as? NSTextView else { return nil }
        let width = proposal.width ?? Self.idealWidth
        let inset = textView.textContainerInset
        let measure = NSLayoutManager()
        let font = MessagesComposerTextView.baseFont
        var textHeight = measure.defaultLineHeight(for: font)

        if width.isFinite, width > inset.width * 2 {
            let container = NSTextContainer(size: NSSize(width: width - inset.width * 2, height: .greatestFiniteMagnitude))
            container.lineFragmentPadding = textView.textContainer?.lineFragmentPadding ?? container.lineFragmentPadding
            let storage = NSTextStorage(string: text, attributes: [.font: font])
            // Fallback fonts (Hangul, emoji) set their own line heights, as in the shown field.
            storage.fixAttributes(in: NSRange(location: 0, length: storage.length))
            storage.addLayoutManager(measure)
            measure.addTextContainer(container)
            measure.ensureLayout(for: container)

            var lineBottoms: [CGFloat] = []
            measure.enumerateLineFragments(forGlyphRange: measure.glyphRange(for: container)) { rect, _, _, _, stop in
                lineBottoms.append(rect.maxY)
                if lineBottoms.count == Self.maxLines { stop.pointee = true }
            }

            // Fewer lines than the most: all of them, with the empty line after a final newline.
            textHeight = lineBottoms.count == Self.maxLines
                ? lineBottoms[Self.maxLines - 1]
                : max(measure.usedRect(for: container).height, textHeight)
        }
        return CGSize(width: width, height: textHeight + inset.height * 2)
    }

    @MainActor
    final class Coordinator: NSObject, NSTextViewDelegate {
        var parent: MessagesComposerField
        let undo = MessagesComposerUndo()

        init(parent: MessagesComposerField) {
            self.parent = parent
        }

        func undoManager(for view: NSTextView) -> UndoManager? {
            undo.undoManager
        }

        func textDidChange(_ notification: Notification) {
            guard let textView = notification.object as? NSTextView else { return }
            parent.onChange(textView.string)
        }

        func textView(_ textView: NSTextView, doCommandBy selector: Selector) -> Bool {
            // Return or Escape while an input method composes goes to the composition.
            guard !textView.hasMarkedText() else { return false }
            switch selector {
            case #selector(NSResponder.insertNewline(_:)):
                parent.onSubmit()
                return true
            case #selector(NSResponder.cancelOperation(_:)):
                return parent.onEscape?() ?? false
            default:
                return false
            }
        }
    }
}

/// The composer's text view: a placeholder, image paste, and no file drops of its own.
final class MessagesComposerTextView: NSTextView {
    /// The field's font. `font` reports the first character's instead — for Hangul, a fallback font
    /// with shorter lines.
    static let baseFont = NSFont.systemFont(ofSize: NSFont.systemFontSize)

    var placeholder = "" {
        didSet {
            if placeholder != oldValue { needsDisplay = true }
        }
    }

    var onPasteImages: (([MessagesAttachment]) -> Void)?

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)
        guard string.isEmpty, !placeholder.isEmpty else { return }

        let font = Self.baseFont
        let padding = textContainer?.lineFragmentPadding ?? 0
        let line = layoutManager?.defaultLineHeight(for: font) ?? font.boundingRectForFont.height
        let rect = NSRect(
            x: textContainerInset.width + padding,
            y: textContainerInset.height,
            width: bounds.width - (textContainerInset.width + padding) * 2,
            height: line
        )

        NSAttributedString(string: placeholder, attributes: [.font: font, .foregroundColor: NSColor.placeholderTextColor])
            .draw(with: rect, options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
    }

    override func didChangeText() {
        super.didChangeText()
        restoreTypingFontIfEmpty()
        // The placeholder comes and goes with the first character.
        needsDisplay = true
    }

    /// Emptied text leaves the deleted characters' font — a fallback font after Hangul — as the typing
    /// font; the next characters take the field's font again.
    func restoreTypingFontIfEmpty() {
        if string.isEmpty {
            typingAttributes[.font] = Self.baseFont
        }
    }

    override func paste(_ sender: Any?) {
        if let onPasteImages {
            do {
                if let images = try MessagesPasteboardImages.read(from: .general) {
                    onPasteImages(images)
                    return
                }
            } catch {
                log.error("pasted images not added: \(error: error)")
                _ = presentError(error)
                return
            }
        }
        super.paste(sender)
    }

    override func validateUserInterfaceItem(_ item: any NSValidatedUserInterfaceItem) -> Bool {
        if item.action == #selector(paste(_:)), isEditable, onPasteImages != nil, MessagesPasteboardImages.available(on: .general) {
            return true
        }
        return super.validateUserInterfaceItem(item)
    }

    override var acceptableDragTypes: [NSPasteboard.PasteboardType] {
        super.acceptableDragTypes.filter { type in
            type != .fileURL
                && type.rawValue != "NSFilenamesPboardType"
                && UTType(type.rawValue)?.conforms(to: .image) != true
        }
    }
}

/// Images on a pasteboard as the composer takes them: copied image files, else image data when the
/// pasteboard holds no text (copied text pastes as text).
@MainActor
enum MessagesPasteboardImages {
    private static let dataTypes: [(type: NSPasteboard.PasteboardType, mime: String)] = [
        (.png, "image/png"),
        (NSPasteboard.PasteboardType(UTType.jpeg.identifier), "image/jpeg"),
        (NSPasteboard.PasteboardType(UTType.gif.identifier), "image/gif"),
        (NSPasteboard.PasteboardType(UTType.heic.identifier), "image/heic"),
        (.tiff, "image/tiff"),
    ]

    private static var imageFiles: [NSPasteboard.ReadingOptionKey: Any] {
        [.urlReadingFileURLsOnly: true, .urlReadingContentsConformToTypes: [UTType.image.identifier]]
    }

    static func available(on pasteboard: NSPasteboard) -> Bool {
        pasteboard.canReadObject(forClasses: [NSURL.self], options: imageFiles) || imageDataType(on: pasteboard) != nil
    }

    /// `nil` when the pasteboard holds no image the composer takes.
    static func read(from pasteboard: NSPasteboard) throws -> [MessagesAttachment]? {
        if let urls = pasteboard.readObjects(forClasses: [NSURL.self], options: imageFiles) as? [URL], !urls.isEmpty {
            return try urls.map(MessagesAttachment.init(contentsOf:))
        }
        guard let entry = imageDataType(on: pasteboard), let data = pasteboard.data(forType: entry.type) else { return nil }
        return [MessagesAttachment(name: "", contentType: entry.mime, data: data)]
    }

    private static func imageDataType(on pasteboard: NSPasteboard) -> (type: NSPasteboard.PasteboardType, mime: String)? {
        guard pasteboard.availableType(from: [.string]) == nil,
              let type = pasteboard.availableType(from: dataTypes.map(\.type))
        else { return nil }
        return dataTypes.first { $0.type == type }
    }
}
