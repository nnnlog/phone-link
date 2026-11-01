import Testing
@testable import PhoneLinkKit

struct HostInfoAdapterTests {
    @Test func theRegionIsATwoLetterCountryCode() {
        #expect(HostInfoAdapter.regionCode(identifier: "KR") == "KR")
        #expect(HostInfoAdapter.regionCode(identifier: "US") == "US")
        #expect(HostInfoAdapter.regionCode(identifier: "001") == nil, "UN M49 world")
        #expect(HostInfoAdapter.regionCode(identifier: "419") == nil, "UN M49 Latin America")
        #expect(HostInfoAdapter.regionCode(identifier: "kr") == nil)
        #expect(HostInfoAdapter.regionCode(identifier: "USA") == nil)
        #expect(HostInfoAdapter.regionCode(identifier: "") == nil)
        #expect(HostInfoAdapter.regionCode(identifier: nil) == nil)
    }

    @Test func theHostNameIsThisMacs() {
        let name = HostInfoAdapter().hostName()
        #expect(!name.isEmpty)
        #expect(!name.contains("\0"))
    }
}
