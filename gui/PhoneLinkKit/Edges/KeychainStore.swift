import Foundation
import os
import PhoneLinkEngine
import Security

/// The engine's `SecureStore` in the login Keychain: one generic-password item (the given service,
/// account `engine-store`) holding every key as a binary property list of `[String: Data]`.
///
/// One item rather than one per key because an ad-hoc signed build (`Signing.xcconfig`'s default)
/// changes its signature with every build, and the Keychain asks the user once per item whose access list names the old one.
/// The item lives in the legacy file-based keychain, as the data protection keychain needs a
/// team-signed entitlement. The map is read on first use and kept; every change writes the whole
/// item through.
public final class KeychainStore: SecureStore {
    private static let account = "engine-store"

    private let service: String
    /// The item's map once read (`nil` until the first call). Holding the lock across the Keychain
    /// calls also serializes the read-modify-write of `store` and `delete`.
    private let entries = OSAllocatedUnfairLock<[String: Data]?>(initialState: nil)

    public init(service: String) {
        self.service = service
    }

    public func load(key: String) throws -> Data? {
        try entries.withLock { cached in
            try self.map(&cached)[key]
        }
    }

    public func store(key: String, value: Data) throws {
        try entries.withLock { cached in
            var map = try self.map(&cached)
            map[key] = value
            try self.write(map)
            cached = map
        }
    }

    public func delete(key: String) throws {
        try entries.withLock { cached in
            var map = try self.map(&cached)
            guard map.removeValue(forKey: key) != nil else { return }
            try self.write(map)
            cached = map
        }
    }

    public func keysWithPrefix(prefix: String) throws -> [String] {
        try entries.withLock { cached in
            try self.map(&cached).keys.filter { $0.hasPrefix(prefix) }
        }
    }

    /// The cached map, read from the Keychain when there is none yet; no item is an empty map.
    private func map(_ cached: inout [String: Data]?) throws -> [String: Data] {
        if let cached {
            return cached
        }

        var query = itemQuery
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)

        let map: [String: Data]
        switch status {
        case errSecSuccess:
            guard let data = result as? Data else {
                throw EdgeError.Store(message: "the Keychain item \(service) returned no data")
            }
            do {
                map = try PropertyListDecoder().decode([String: Data].self, from: data)
            } catch {
                throw EdgeError.Store(message: "the Keychain item \(service) is not a key map: \(error)")
            }
        case errSecItemNotFound:
            map = [:]
        default:
            throw Self.failure(status, "reading the Keychain item \(service)")
        }

        cached = map
        return map
    }

    /// Replaces the item's data with `map`, adding the item when there is none.
    private func write(_ map: [String: Data]) throws {
        let encoder = PropertyListEncoder()
        encoder.outputFormat = .binary
        let data: Data
        do {
            data = try encoder.encode(map)
        } catch {
            throw EdgeError.Store(message: "encoding the key map failed: \(error)")
        }

        let status = SecItemUpdate(itemQuery as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        switch status {
        case errSecSuccess:
            return
        case errSecItemNotFound:
            var item = itemQuery
            item[kSecValueData as String] = data
            let added = SecItemAdd(item as CFDictionary, nil)
            guard added == errSecSuccess else {
                throw Self.failure(added, "adding the Keychain item \(service)")
            }
        default:
            throw Self.failure(status, "updating the Keychain item \(service)")
        }
    }

    private var itemQuery: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: Self.account,
        ]
    }

    private static func failure(_ status: OSStatus, _ doing: String) -> EdgeError {
        let message = SecCopyErrorMessageString(status, nil) as String? ?? "no message"
        return EdgeError.Store(message: "\(doing) failed: \(message) (OSStatus \(status))")
    }
}
