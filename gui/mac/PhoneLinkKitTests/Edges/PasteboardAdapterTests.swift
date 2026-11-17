import AppKit
import PhoneLinkEngine
import Testing
@testable import PhoneLinkKit

/// Each test works on its own named pasteboard, never the general one, and releases it.
struct PasteboardAdapterTests {
    private static func withPasteboard(_ body: (NSPasteboard, PasteboardAdapter) throws -> Void) rethrows {
        let pasteboard = NSPasteboard(name: NSPasteboard.Name("phonelink-test-\(UUID().uuidString)"))
        defer { pasteboard.releaseGlobally() }
        try body(pasteboard, PasteboardAdapter(pasteboard: pasteboard))
    }

    @Test func aWriteIsOneItemWithEveryRepresentation() throws {
        try Self.withPasteboard { (pasteboard, adapter) throws in
            let html = Data("<b>hi</b>".utf8)
            let text = Data("hi".utf8)
            let written = try adapter.write(representations: [
                ClipboardPasteboardData(uti: "public.html", data: html),
                ClipboardPasteboardData(uti: "public.utf8-plain-text", data: text),
            ])

            #expect(written == ClipboardPasteboardWrite(changeCount: Int64(pasteboard.changeCount), failure: nil))
            #expect(try adapter.changeCount() == written.changeCount)
            #expect(pasteboard.pasteboardItems?.count == 1)
            #expect(Set(try adapter.types()).isSuperset(of: ["public.html", "public.utf8-plain-text"]))
            #expect(try adapter.data(uti: "public.html") == html)
            #expect(try adapter.data(uti: "public.utf8-plain-text") == text)
            #expect(try adapter.data(uti: "public.png") == nil)
        }
    }

    @Test func everyWriteAndOutsideChangeMovesTheChangeCount() throws {
        try Self.withPasteboard { (pasteboard, adapter) throws in
            let first = try adapter.write(representations: [ClipboardPasteboardData(uti: "public.utf8-plain-text", data: Data("a".utf8))]).changeCount
            let second = try adapter.write(representations: [ClipboardPasteboardData(uti: "public.utf8-plain-text", data: Data("b".utf8))]).changeCount

            #expect(second > first)
            #expect(try adapter.data(uti: "public.utf8-plain-text") == Data("b".utf8), "the write replaced the contents")

            pasteboard.clearContents()
            pasteboard.setString("c", forType: .string)

            #expect(try adapter.changeCount() > second)
            #expect(try adapter.data(uti: "public.utf8-plain-text") == Data("c".utf8))
        }
    }

    /// Regression: a representation the pasteboard refuses stops the
    /// write after `clearContents()` changed the pasteboard; the write reports that change count and
    /// the refusal, and the representations before it stay written.
    @Test func aWriteStoppedPartWayReportsItsChangeCount() throws {
        let pasteboard = NSPasteboard(name: NSPasteboard.Name("phonelink-test-\(UUID().uuidString)"))
        defer { pasteboard.releaseGlobally() }
        let adapter = PasteboardAdapter(pasteboard: pasteboard) { pasteboard, data, type in
            type.rawValue == "public.tiff" ? false : pasteboard.setData(data, forType: type)
        }

        let png = Data("png".utf8)
        let written = try adapter.write(representations: [
            ClipboardPasteboardData(uti: "public.png", data: png),
            ClipboardPasteboardData(uti: "public.tiff", data: Data("tiff".utf8)),
            ClipboardPasteboardData(uti: "public.utf8-plain-text", data: Data("never".utf8)),
        ])

        #expect(written.changeCount == Int64(pasteboard.changeCount))
        #expect(written.failure?.contains("public.tiff") == true)
        #expect(try adapter.data(uti: "public.png") == png)
        // What the item holds (`types` of the pasteboard also lists the TIFF macOS would convert to).
        #expect(pasteboard.pasteboardItems?.map { $0.types.map(\.rawValue) } == [["public.png"]], "nothing from the refused one on")

        let complete = try adapter.write(representations: [ClipboardPasteboardData(uti: "public.png", data: png)])
        #expect(complete.failure == nil)
        #expect(complete.changeCount > written.changeCount)
    }

    @Test func anEmptyPasteboardHasNoTypes() throws {
        try Self.withPasteboard { (pasteboard, adapter) throws in
            pasteboard.clearContents()
            #expect(try adapter.types().isEmpty)
            #expect(try adapter.data(uti: "public.utf8-plain-text") == nil)
        }
    }
}
