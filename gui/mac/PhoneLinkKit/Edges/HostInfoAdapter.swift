import Darwin
import Foundation
import PhoneLinkEngine

/// Host facts: the host name from `gethostname`, the source the CLI's `hostname` reads too (the
/// engine names the PC with the part before the first `.`), and the region of the user's locale.
public final class HostInfoAdapter: HostInfo {
    public init() {}

    public func hostName() -> String {
        var buffer = [CChar](repeating: 0, count: Int(MAXHOSTNAMELEN) + 1)
        // A buffer of MAXHOSTNAMELEN + 1 bytes holds every host name, so the call cannot fail.
        guard gethostname(&buffer, buffer.count) == 0 else {
            preconditionFailure("gethostname failed with errno \(errno)")
        }
        return String(decoding: buffer.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }, as: UTF8.self)
    }

    public func regionCode() -> String? {
        Self.regionCode(identifier: Locale.current.region?.identifier)
    }

    /// The identifier when it is an ISO 3166-1 alpha-2 code; `Locale.Region` also has UN M49
    /// numeric ones (`001`, `419`), which name no country.
    static func regionCode(identifier: String?) -> String? {
        guard let identifier, identifier.utf8.count == 2,
              identifier.utf8.allSatisfy({ (UInt8(ascii: "A")...UInt8(ascii: "Z")).contains($0) })
        else { return nil }
        return identifier
    }
}
