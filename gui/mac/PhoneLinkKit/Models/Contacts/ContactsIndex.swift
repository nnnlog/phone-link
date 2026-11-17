import Foundation
import PhoneLinkEngine

/// One group of the contacts list: the contacts whose shown name starts under its index letter.
public struct ContactsSection: Identifiable, Equatable, Sendable {
    /// `A`–`Z`, a Hangul initial consonant `ㄱ`–`ㅎ`, or `#`.
    public let title: String
    public let contacts: [ContactListItem]

    public var id: String { title }
}

/// The contacts list's grouping, order and search: groups by the shown name's first character — a
/// Latin letter without its diacritics, a Hangul initial consonant (a tense one under its plain one),
/// else `#` — in the order Latin `A`–`Z`, Hangul `ㄱ`–`ㅎ`, `#`; within a group by the name in the
/// Finder's order, then by contact id.
public enum ContactsIndex {
    /// The group of contacts without a letter: no name, a digit, another script.
    public static let otherGroup = "#"

    private static let latin = (UnicodeScalar("A").value...UnicodeScalar("Z").value).map { String(UnicodeScalar($0)!) }
    private static let hangul = ["ㄱ", "ㄴ", "ㄷ", "ㄹ", "ㅁ", "ㅂ", "ㅅ", "ㅇ", "ㅈ", "ㅊ", "ㅋ", "ㅌ", "ㅍ", "ㅎ"]
    /// The 19 initial consonants of a Hangul syllable, in Unicode order, each as its plain consonant.
    private static let initials = ["ㄱ", "ㄱ", "ㄴ", "ㄷ", "ㄷ", "ㄹ", "ㅁ", "ㅂ", "ㅂ", "ㅅ", "ㅅ", "ㅇ", "ㅈ", "ㅈ", "ㅊ", "ㅋ", "ㅌ", "ㅍ", "ㅎ"]
    /// Compatibility jamo that are tense consonants, by their plain one.
    private static let tense: [String: String] = ["ㄲ": "ㄱ", "ㄸ": "ㄷ", "ㅃ": "ㅂ", "ㅆ": "ㅅ", "ㅉ": "ㅈ"]
    private static let order = latin + hangul + [otherGroup]

    /// The group a shown name goes under.
    public static func group(of name: String) -> String {
        guard let first = name.trimmingCharacters(in: .whitespacesAndNewlines).first,
              let scalar = String(first).unicodeScalars.first else { return otherGroup }

        let value = scalar.value
        if (0xAC00...0xD7A3).contains(value) {
            return initials[Int((value - 0xAC00) / 588)]
        }
        if (0x1100...0x1112).contains(value) {
            return initials[Int(value - 0x1100)]
        }

        let jamo = String(first)
        if hangul.contains(jamo) { return jamo }
        if let plain = tense[jamo] { return plain }

        let folded = String(first).folding(options: [.diacriticInsensitive, .caseInsensitive, .widthInsensitive], locale: nil)
        if let letter = folded.uppercased().first.map(String.init), latin.contains(letter) {
            return letter
        }
        return otherGroup
    }

    /// The list's groups, each sorted, in index order; empty groups are left out.
    public static func sections(_ contacts: [ContactListItem]) -> [ContactsSection] {
        let grouped = Dictionary(grouping: contacts) { group(of: $0.name) }
        return order.compactMap { title in
            guard let members = grouped[title] else { return nil }
            return ContactsSection(title: title, contacts: members.sorted(by: precedes))
        }
    }

    /// Name order (the Finder's comparison), then contact id.
    static func precedes(_ a: ContactListItem, _ b: ContactListItem) -> Bool {
        switch a.name.localizedStandardCompare(b.name) {
        case .orderedAscending: true
        case .orderedDescending: false
        case .orderedSame: a.contactId < b.contactId
        }
    }

    /// Every white-space-separated token of the query is found, case- and diacritic-insensitively, in
    /// the name or one of the search terms; a token of digits alone also matches a term whose digits
    /// contain it ("0101234" finds "010-1234-5678"). An empty query matches every contact.
    public static func matches(_ contact: ContactListItem, query: String) -> Bool {
        let tokens = query.split(whereSeparator: \.isWhitespace).map(String.init)
        let fields = [contact.name] + contact.searchTerms
        return tokens.allSatisfy { token in
            if fields.contains(where: { $0.localizedStandardContains(token) }) { return true }
            guard token.allSatisfy(\.isASCIIDigit) else { return false }
            return contact.searchTerms.contains { digits($0).contains(token) }
        }
    }

    private static func digits(_ s: String) -> String {
        String(s.filter(\.isASCIIDigit))
    }
}

private extension Character {
    var isASCIIDigit: Bool { isASCII && isNumber }
}
