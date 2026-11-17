import Foundation
import PhoneLinkEngine
import Testing
@testable import PhoneLinkKit

/// How a log line is split into public and private parts, and the saved log's suggested name.
struct AppLogTests {
    private func parts(_ message: LogMessage) -> [String] {
        message.segments.map { $0.isPublic ? $0.text : "[\($0.text)]" }
    }

    @Test func stringsArePrivateUnlessMarkedPublic() {
        let name = "Pixel"
        let route = "/legacy/phonecontent"
        #expect(parts("phone \(name) via \(route, privacy: .public)") == ["phone ", "[Pixel]", " via /legacy/phonecontent"])
    }

    @Test func numbersAndFlagsArePublicUnlessMarkedPrivate() {
        let id: Int64 = 42
        let rate = 48000.0
        let secret = 7

        #expect(
            parts("photo \(id) at \(rate) Hz, ok \(true), pin \(secret, privacy: .private)")
                == ["photo 42 at 48000.0 Hz, ok true, pin ", "[7]"]
        )
    }

    @Test func anErrorIsPublicWithoutTheHotspotCredentials() {
        let error = HotspotError.JoinFailed(ssid: "Home", passphrase: "hunter22", reason: "timed out")
        #expect(parts("join failed: \(error: error)") == ["join failed: JoinFailed(reason: \"timed out\")"])
        #expect(parts("\(error: HotspotError.NotVisible(ssid: "Home", passphrase: "hunter22"))") == ["NotVisible"])
    }

    @Test func aLiteralIsOnePublicPart() {
        #expect(parts("the app connected") == ["the app connected"])
    }

    @Test func theSavedLogIsNamedAfterItsTime() {
        let date = Date(timeIntervalSince1970: 1_793_455_500)  // 2026-10-31 14:05 UTC
        #expect(LogArchive.suggestedFileName(at: date, timeZone: TimeZone(identifier: "UTC")!) == "Phone Link Log 2026-10-31 14.05.log")
    }
}
