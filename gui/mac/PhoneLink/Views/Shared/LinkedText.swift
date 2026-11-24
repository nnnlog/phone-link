import AppKit
import PhoneLinkKit
import SwiftUI

/// Text the phone sent — a message, a notification — as the user reads it: selectable, its web and
/// e-mail addresses links that open when clicked (`TextLinks`), and a menu of its own.
///
/// It is an `NSTextView` because a selectable SwiftUI `Text` shows the system's text menu, which a
/// `contextMenu` cannot replace. The menu copies the selection, or everything while nothing is
/// selected (`copyText` in its place where the text is part of something larger), selects all, and
/// copies a link's address.
struct LinkedText: NSViewRepresentable {
    let text: String
    var color: NSColor = .labelColor
    var linkColor: NSColor = .linkColor
    /// The selection's background where the system's would not show (on the accent colour).
    var selectionColor: NSColor?
    /// What Copy takes while nothing is selected; the text itself when `nil`.
    var copyText: String?

    private static let font = NSFont.systemFont(ofSize: NSFont.systemFontSize)

    func makeCoordinator() -> Coordinator {
        Coordinator()
    }

    func makeNSView(context: Context) -> LinkedTextView {
        let textView = LinkedTextView(usingTextLayoutManager: false)
        textView.isEditable = false
        textView.isSelectable = true
        textView.drawsBackground = false
        textView.textContainerInset = .zero
        textView.textContainer?.lineFragmentPadding = 0
        textView.textContainer?.widthTracksTextView = true
        textView.isVerticallyResizable = false
        textView.isHorizontallyResizable = false
        textView.isAutomaticLinkDetectionEnabled = false
        return textView
    }

    func updateNSView(_ textView: LinkedTextView, context: Context) {
        let shown = Coordinator.Shown(text: text, color: color, linkColor: linkColor, selectionColor: selectionColor)
        if context.coordinator.shown != shown {
            context.coordinator.show(shown, font: Self.font)
            textView.textStorage?.setAttributedString(context.coordinator.attributed)
            textView.linkTextAttributes = [
                .foregroundColor: linkColor,
                .underlineStyle: NSUnderlineStyle.single.rawValue,
                .cursor: NSCursor.pointingHand,
            ]
            textView.selectedTextAttributes = [.backgroundColor: selectionColor ?? NSColor.selectedTextBackgroundColor]
        }

        textView.links = context.coordinator.links
        textView.copyText = copyText
    }

    /// As wide as its longest line within the width offered, and as high as its lines there. The text
    /// is measured in layout objects of its own, so a width SwiftUI only asks about never reaches the
    /// shown view.
    func sizeThatFits(_ proposal: ProposedViewSize, nsView: LinkedTextView, context: Context) -> CGSize? {
        let offered = proposal.width.map { max($0, 1) } ?? .greatestFiniteMagnitude
        return context.coordinator.size(within: offered.isFinite ? offered : .greatestFiniteMagnitude)
    }

    @MainActor
    final class Coordinator {
        struct Shown: Equatable {
            var text = ""
            var color = NSColor.labelColor
            var linkColor = NSColor.linkColor
            var selectionColor: NSColor?
        }

        private(set) var shown = Shown()
        private(set) var attributed = NSAttributedString()
        private(set) var links: [TextLinks.Link] = []
        private var sizes: [CGFloat: CGSize] = [:]

        func show(_ shown: Shown, font: NSFont) {
            self.shown = shown
            links = TextLinks.links(in: shown.text)
            sizes = [:]

            let text = NSMutableAttributedString(string: shown.text, attributes: [.font: font, .foregroundColor: shown.color])
            for link in links {
                text.addAttribute(.link, value: link.url, range: link.range)
            }
            attributed = text
        }

        func size(within width: CGFloat) -> CGSize {
            if let size = sizes[width] {
                return size
            }

            let storage = NSTextStorage(attributedString: attributed)
            let measure = NSLayoutManager()
            let container = NSTextContainer(size: NSSize(width: width, height: .greatestFiniteMagnitude))
            container.lineFragmentPadding = 0
            storage.addLayoutManager(measure)
            measure.addTextContainer(container)
            measure.ensureLayout(for: container)

            let used = measure.usedRect(for: container)
            let size = CGSize(width: min(ceil(used.width), width), height: ceil(used.height))
            sizes[width] = size
            return size
        }
    }
}

/// `LinkedText`'s text view: its own menu, and no selection left behind when the user clicks elsewhere.
final class LinkedTextView: NSTextView {
    var links: [TextLinks.Link] = []
    var copyText: String?

    /// The menu is built here, not by the text view, which would first select the word under the
    /// pointer: with nothing selected, Copy takes everything.
    override func menu(for event: NSEvent) -> NSMenu? {
        let menu = NSMenu()
        let selection = selectedRange()
        let all = (string as NSString).length

        let copied = selection.length > 0 ? (string as NSString).substring(with: selection) : copyText ?? string
        menu.addItem(copyItem(String(localized: "Copy"), copied))
        if selection.length < all {
            let selectAll = menu.addItem(withTitle: String(localized: "Select All"), action: #selector(selectAll(_:)), keyEquivalent: "")
            selectAll.target = self
        }

        let addresses = TextLinks.copyValues(of: links, in: selection.length > 0 ? selection : nil)
        if !addresses.isEmpty {
            menu.addItem(.separator())
        }
        if addresses.count == 1 {
            menu.addItem(copyItem(String(localized: "Copy Link"), addresses[0]))
        } else {
            for address in addresses {
                menu.addItem(copyItem(String(localized: "Copy \(TextLinks.menuValue(address))", comment: "Menu item; the placeholder is a link's address"), address))
            }
        }
        return menu
    }

    private func copyItem(_ title: String, _ value: String) -> NSMenuItem {
        let item = NSMenuItem(title: title, action: #selector(copyValue(_:)), keyEquivalent: "")
        item.target = self
        item.representedObject = value
        return item
    }

    @objc private func copyValue(_ sender: NSMenuItem) {
        guard let value = sender.representedObject as? String else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(value, forType: .string)
    }

    override func resignFirstResponder() -> Bool {
        setSelectedRange(NSRange(location: selectedRange().location, length: 0))
        return super.resignFirstResponder()
    }
}
