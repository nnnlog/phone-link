import Foundation
import Testing
@testable import PhoneLinkKit

struct TextLinksTests {
    /// The linked pieces of `text` with where they lead.
    private func found(_ text: String) -> [String: String] {
        var found: [String: String] = [:]
        for link in TextLinks.links(in: text) {
            found[(text as NSString).substring(with: link.range)] = link.url.absoluteString
        }
        return found
    }

    @Test func webAndMailAddressesAreLinks() {
        #expect(found("Track it at https://example.com/t?id=1.") == ["https://example.com/t?id=1": "https://example.com/t?id=1"])
        // A bare domain's scheme is the system's choice (http or https by OS version).
        let bare = TextLinks.links(in: "see example.com now")
        #expect(bare.map(\.range) == [NSRange(location: 4, length: 11)])
        #expect(bare.first?.url.host() == "example.com")
        #expect(found("write to someone@example.com today") == ["someone@example.com": "mailto:someone@example.com"])
        #expect(found("한글 뒤에 https://example.com/가 있어요").keys.contains { $0.hasPrefix("https://example.com") })
    }

    @Test func nothingElseIsALink() {
        #expect(found("call 010-1234-5678 or +1 (425) 555-0100").isEmpty, "phone numbers are not links")
        #expect(found("get ftp://example.com/file and file:///etc/hosts").isEmpty, "only web and mail addresses open")
        #expect(found("no links here").isEmpty)
        #expect(found("").isEmpty)
    }

    @Test func aLinkIsCopiedAsItsAddress() throws {
        #expect(TextLinks.copyValue(of: try #require(URL(string: "https://example.com/a"))) == "https://example.com/a")
        #expect(TextLinks.copyValue(of: try #require(URL(string: "mailto:someone@example.com"))) == "someone@example.com")
    }

    @Test func theMenuOffersEachDistinctLinkOfTheSelection() {
        let text = "https://a.example.com https://b.example.com https://a.example.com"
        let links = TextLinks.links(in: text)
        #expect(links.count == 3)

        #expect(TextLinks.copyValues(of: links, in: nil) == ["https://a.example.com", "https://b.example.com"])

        let first = NSRange(location: 0, length: 21)
        #expect(TextLinks.copyValues(of: links, in: first) == ["https://a.example.com"])

        let partOfSecond = NSRange(location: 22, length: 9)
        #expect(TextLinks.copyValues(of: links, in: partOfSecond).isEmpty, "a link the selection only touches is not offered")
    }

    @Test func aLongAddressIsCutInAMenuTitle() {
        #expect(TextLinks.menuValue("https://example.com") == "https://example.com")
        #expect(TextLinks.menuValue("https://example.com/a/very/long/path/indeed") == "https://example.com/a/very/lon…")
    }
}
