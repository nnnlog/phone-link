import PhoneLinkEngine
import Testing
@testable import PhoneLinkKit

struct UTTypeFileTypesTests {
    private let types = UTTypeFileTypes()

    @Test(arguments: [
        ("text/vcard", FileFamily.vCard),
        ("text/x-vcard", .vCard),
        ("image/jpeg", .image),
        ("IMAGE/PNG", .image),
        ("image/gif", .image),
        ("image/heic", .image),
        ("audio/mpeg", .audio),
        ("audio/amr", .audio),
        ("video/mp4", .video),
        ("video/3gpp", .video),
        ("text/plain", .text),
        ("text/html", .text),
        ("text/rtf", .text),
        ("application/pdf", .document),
        ("application/msword", .document),
        ("application/vnd.openxmlformats-officedocument.wordprocessingml.document", .document),
        ("application/vnd.oasis.opendocument.text", .document),
        ("application/vnd.ms-excel", .document),
        ("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", .document),
        ("application/vnd.ms-powerpoint", .document),
        ("application/vnd.openxmlformats-officedocument.presentationml.presentation", .document),
        ("application/zip", .other),
        ("application/x-phonelink-unknown", .other),
        ("text/plain; charset=utf-8", .other),
        ("", .other),
    ])
    func family(contentType: String, family: FileFamily) {
        #expect(types.family(contentType: contentType) == family)
    }

    @Test(arguments: [
        ("image/jpeg", ".jpeg"),
        ("image/png", ".png"),
        ("text/x-vcard", ".vcf"),
        ("text/plain", ".txt"),
        ("video/mp4", ".mp4"),
        ("application/pdf", ".pdf"),
    ])
    func fileExtensionHasItsDot(contentType: String, fileExtension: String) {
        #expect(types.fileExtension(contentType: contentType) == fileExtension)
    }

    @Test func aTypeWithoutAnExtensionHasNone() {
        #expect(types.fileExtension(contentType: "application/x-phonelink-unknown") == nil)
        #expect(types.fileExtension(contentType: "") == nil)
    }
}
