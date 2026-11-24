import Foundation

/// The web and e-mail addresses in text the phone sent, as links.
///
/// The system's data detector finds them. Only `http`, `https` and `mailto` links are kept, so a click
/// on text someone else wrote opens a web page or a new e-mail and nothing else (Phone Link on Windows
/// also opens `file:` and `ftp:` addresses). Phone numbers are not links, as on Windows.
public enum TextLinks {
    public struct Link: Equatable, Sendable {
        /// Where the link is in the text, in UTF-16 units.
        public let range: NSRange
        public let url: URL

        public init(range: NSRange, url: URL) {
            self.range = range
            self.url = url
        }
    }

    private static let schemes: Set<String> = ["http", "https", "mailto"]

    /// The longest address a menu title shows before it is cut.
    private static let menuValueLength = 30

    private static let detector: NSDataDetector = {
        do {
            return try NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue)
        } catch {
            fatalError("the data detector refused the link type: \(error)")
        }
    }()

    /// The links of `text`, in order.
    public static func links(in text: String) -> [Link] {
        let whole = NSRange(text.startIndex..., in: text)
        return detector.matches(in: text, range: whole).compactMap { match in
            guard let url = match.url, let scheme = url.scheme?.lowercased(), schemes.contains(scheme) else { return nil }
            return Link(range: match.range, url: url)
        }
    }

    /// What copying a link puts on the pasteboard: its address, an e-mail address without `mailto:`.
    public static func copyValue(of url: URL) -> String {
        let address = url.absoluteString
        guard url.scheme?.lowercased() == "mailto" else { return address }
        return String(address.dropFirst("mailto:".count)).removingPercentEncoding ?? address
    }

    /// The addresses a text's menu offers to copy: each distinct link once, in order; with a
    /// `selection`, only the links that lie inside it.
    public static func copyValues(of links: [Link], in selection: NSRange?) -> [String] {
        var values: [String] = []
        for link in links {
            if let selection, NSIntersectionRange(selection, link.range).length != link.range.length {
                continue
            }

            let value = copyValue(of: link.url)
            if !values.contains(value) {
                values.append(value)
            }
        }
        return values
    }

    /// An address as a menu title shows it: cut to its first characters when it is long.
    public static func menuValue(_ value: String) -> String {
        value.count > menuValueLength ? value.prefix(menuValueLength) + "…" : value
    }
}
