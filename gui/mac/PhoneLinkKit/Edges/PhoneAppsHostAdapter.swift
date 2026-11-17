import AppKit
import Foundation
import PhoneLinkEngine

/// What the phone apps ask of this Mac.
///
/// - Whether an installed application opens a URI: Launch Services' default application for it
///   (`NSWorkspace.urlForApplication(toOpen:)`); a string that is no URL with a scheme opens nothing.
///   A recent app's document link is kept only when one does.
/// - The order of app names: Foundation's comparison in the current locale, ignoring case,
///   diacritics and width, after both names are turned to katakana so hiragana and katakana compare
///   equal; equal names keep their given order.
public final class PhoneAppsHostAdapter: PhoneAppsHost {
    private let locale: @Sendable () -> Locale
    private let defaultApplication: @Sendable (URL) -> URL?

    public convenience init() {
        self.init(locale: { Locale.current }, defaultApplication: { NSWorkspace.shared.urlForApplication(toOpen: $0) })
    }

    init(locale: @escaping @Sendable () -> Locale, defaultApplication: @escaping @Sendable (URL) -> URL?) {
        self.locale = locale
        self.defaultApplication = defaultApplication
    }

    public func canOpenUri(uri: String) -> Bool {
        guard let url = URL(string: uri), let scheme = url.scheme, !scheme.isEmpty else { return false }
        return defaultApplication(url) != nil
    }

    public func nameOrder(names: [String]) -> [UInt32] {
        let locale = locale()
        let keys = names.map { $0.applyingTransform(.hiraganaToKatakana, reverse: false) ?? $0 }
        let order = names.indices.sorted { a, b in
            switch keys[a].compare(keys[b], options: [.caseInsensitive, .diacriticInsensitive, .widthInsensitive], locale: locale) {
            case .orderedAscending: true
            case .orderedDescending: false
            case .orderedSame: a < b
            }
        }
        return order.map { UInt32($0) }
    }
}
