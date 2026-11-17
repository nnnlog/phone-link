import Foundation

/// A Semantic Version (`1.2.3`, `1.2.3-beta.1`), ordered by Semantic Versioning precedence: the
/// numbers, then a release above its pre-releases, then the pre-release's dot-separated identifiers,
/// numeric ones by value and below alphanumeric ones. Build metadata (`+…`) is not part of the app's
/// versions and does not parse.
public struct AppVersion: Comparable, Hashable, Sendable, CustomStringConvertible {
    public let major: Int
    public let minor: Int
    public let patch: Int
    /// The pre-release identifiers; empty for a release.
    public let preRelease: [String]

    public var isPreRelease: Bool { !preRelease.isEmpty }

    public var description: String {
        let core = "\(major).\(minor).\(patch)"
        return preRelease.isEmpty ? core : core + "-" + preRelease.joined(separator: ".")
    }

    /// `nil` unless `text` is exactly `X.Y.Z` or `X.Y.Z-<identifiers>` without leading zeros.
    public init?(_ text: String) {
        let (core, pre): (Substring, Substring?) = {
            guard let dash = text.firstIndex(of: "-") else { return (text[...], nil) }
            return (text[..<dash], text[text.index(after: dash)...])
        }()

        let numbers = core.split(separator: ".", omittingEmptySubsequences: false)
        guard numbers.count == 3 else { return nil }
        var parsed: [Int] = []
        for number in numbers {
            guard Self.isNumeric(number), let value = Int(number) else { return nil }
            parsed.append(value)
        }

        var identifiers: [String] = []
        if let pre {
            for identifier in pre.split(separator: ".", omittingEmptySubsequences: false) {
                guard !identifier.isEmpty,
                      identifier.allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "-") }),
                      !identifier.allSatisfy(\.isNumber) || Self.isNumeric(identifier)
                else { return nil }
                identifiers.append(String(identifier))
            }
        }

        major = parsed[0]
        minor = parsed[1]
        patch = parsed[2]
        preRelease = identifiers
    }

    /// The version of a release tag: `v` and the version.
    public init?(tag: String) {
        guard tag.hasPrefix("v") else { return nil }
        self.init(String(tag.dropFirst()))
    }

    /// Digits without a leading zero (`0` itself is fine).
    private static func isNumeric(_ s: Substring) -> Bool {
        !s.isEmpty && s.allSatisfy { $0.isASCII && $0.isNumber } && (s == "0" || s.first != "0")
    }

    public static func < (lhs: AppVersion, rhs: AppVersion) -> Bool {
        if (lhs.major, lhs.minor, lhs.patch) != (rhs.major, rhs.minor, rhs.patch) {
            return (lhs.major, lhs.minor, lhs.patch) < (rhs.major, rhs.minor, rhs.patch)
        }

        switch (lhs.preRelease.isEmpty, rhs.preRelease.isEmpty) {
        case (true, true), (true, false): return false
        case (false, true): return true
        case (false, false): break
        }

        for (l, r) in zip(lhs.preRelease, rhs.preRelease) where l != r {
            switch (Int(l), Int(r)) {
            case let (l?, r?): return l < r
            case (.some, nil): return true
            case (nil, .some): return false
            case (nil, nil): return l < r
            }
        }
        return lhs.preRelease.count < rhs.preRelease.count
    }
}
