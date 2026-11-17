import Foundation
import Security
import Testing
@testable import PhoneLinkKit

/// Each test uses its own test-only service and deletes its item at the end.
struct KeychainStoreTests {
    private static func withService(_ body: (String) throws -> Void) rethrows {
        let service = "\(PhoneLinkIdentity.bundleID).tests.\(UUID().uuidString)"
        defer {
            let query: [String: Any] = [kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: service]
            SecItemDelete(query as CFDictionary)
        }
        try body(service)
    }

    /// The raw item, read past the store.
    private static func itemExists(service: String) -> Bool {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: "engine-store",
        ]
        return SecItemCopyMatching(query as CFDictionary, nil) == errSecSuccess
    }

    @Test func aMissingKeyLoadsNothingAndCreatesNoItem() throws {
        try Self.withService { (service) throws in
            let store = KeychainStore(service: service)
            #expect(try store.load(key: "auth/refresh-token") == nil)

            try store.delete(key: "auth/refresh-token")
            #expect(!Self.itemExists(service: service))
        }
    }

    @Test func storedValuesLoadAndDeletedOnesAreGone() throws {
        try Self.withService { (service) throws in
            let store = KeychainStore(service: service)
            try store.store(key: "auth/a", value: Data([1, 2, 3]))
            try store.store(key: "app/b", value: Data())

            #expect(Self.itemExists(service: service))
            #expect(try store.load(key: "auth/a") == Data([1, 2, 3]))
            #expect(try store.load(key: "app/b") == Data())

            try store.store(key: "auth/a", value: Data([9]))
            #expect(try store.load(key: "auth/a") == Data([9]))

            try store.delete(key: "auth/a")
            #expect(try store.load(key: "auth/a") == nil)
            #expect(try store.load(key: "app/b") == Data())
        }
    }

    @Test func valuesPersistAcrossInstances() throws {
        try Self.withService { (service) throws in
            let first = KeychainStore(service: service)
            try first.store(key: "auth/device", value: Data("device".utf8))
            try first.store(key: "auth/gone", value: Data("x".utf8))
            try first.delete(key: "auth/gone")

            let second = KeychainStore(service: service)
            #expect(try second.load(key: "auth/device") == Data("device".utf8))
            #expect(try second.load(key: "auth/gone") == nil)
        }
    }
}
