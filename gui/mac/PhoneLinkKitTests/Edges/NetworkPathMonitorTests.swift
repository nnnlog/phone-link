import Network
import PhoneLinkEngine
import Testing
@testable import PhoneLinkKit

struct NetworkPathMonitorTests {
    private static func kind(satisfied: Bool, _ interfaces: [NWInterface.InterfaceType]) -> PcInternetPath {
        NetworkPathMonitor.kind(satisfied: satisfied, uses: { interfaces.contains($0) })
    }

    @Test func aSatisfiedPathIsNamedByItsInterface() {
        #expect(Self.kind(satisfied: true, [.wifi]) == .wiFi)
        #expect(Self.kind(satisfied: true, [.cellular]) == .cellular)
        #expect(Self.kind(satisfied: true, [.wiredEthernet]) == .wiredEthernet)
        #expect(Self.kind(satisfied: true, [.other]) == .other)
        #expect(Self.kind(satisfied: true, [.loopback]) == .other)
        #expect(Self.kind(satisfied: true, []) == .other)
    }

    @Test func wiFiComesFirst() {
        #expect(Self.kind(satisfied: true, [.wiredEthernet, .wifi]) == .wiFi)
        #expect(Self.kind(satisfied: true, [.wiredEthernet, .cellular]) == .cellular)
    }

    @Test func anUnsatisfiedPathIsUnavailable() {
        #expect(Self.kind(satisfied: false, [.wifi]) == .unavailable)
        #expect(Self.kind(satisfied: false, []) == .unavailable)
    }
}
