import Foundation
import PhoneLinkEngine
import Testing
@testable import PhoneLinkKit

struct ImageIOCodecTests {
    private let codec = ImageIOCodec()

    @Test func sizeIsThePixelSizeOfAnyDecodableFormat() throws {
        #expect(try codec.size(image: EdgesTestImage.png(width: 64, height: 48)) == ClipboardImageSize(width: 64, height: 48))
        #expect(try codec.size(image: EdgesTestImage.jpeg(width: 30, height: 70)) == ClipboardImageSize(width: 30, height: 70))
    }

    @Test func encodePngKeepsTheSizeAndTheAlpha() throws {
        let png = try codec.encodePng(image: EdgesTestImage.png(width: 40, height: 20, opaque: false))
        #expect(png.starts(with: EdgesTestImage.pngSignature))

        let decoded = try #require(EdgesTestImage.decode(png))
        #expect((decoded.width, decoded.height) == (40, 20))
        #expect(EdgesTestImage.pixel(of: decoded, x: 0, y: 0)[3] == 0, "the transparent quarter stays transparent")
        #expect(EdgesTestImage.pixel(of: decoded, x: 39, y: 0)[3] == 255)
    }

    @Test func encodePngConvertsOtherFormats() throws {
        let png = try codec.encodePng(image: EdgesTestImage.jpeg(width: 16, height: 9))
        #expect(png.starts(with: EdgesTestImage.pngSignature))
        #expect(EdgesTestImage.size(png)! == (16, 9))
    }

    @Test func resizePngDrawsAtTheGivenSize() throws {
        let png = try codec.resizePng(image: EdgesTestImage.png(width: 400, height: 300, opaque: false), width: 133, height: 77)
        #expect(png.starts(with: EdgesTestImage.pngSignature))

        let decoded = try #require(EdgesTestImage.decode(png))
        #expect((decoded.width, decoded.height) == (133, 77))
        #expect(EdgesTestImage.pixel(of: decoded, x: 0, y: 0)[3] == 0, "alpha survives the redraw")

        #expect(throws: EdgeError.self) { try codec.resizePng(image: EdgesTestImage.png(width: 4, height: 4), width: 0, height: 4) }
    }

    @Test func encodeTiffIsATiffOfTheSameSize() throws {
        let tiff = try codec.encodeTiff(image: EdgesTestImage.png(width: 12, height: 34))
        #expect(EdgesTestImage.typeIdentifier(tiff) == "public.tiff")
        #expect(EdgesTestImage.size(tiff)! == (12, 34))
    }

    @Test func everyMethodThrowsWhenTheBytesDoNotDecode() {
        let garbage = Data("not an image".utf8)

        #expect(throws: EdgeError.self) { try codec.size(image: garbage) }
        #expect(throws: EdgeError.self) { try codec.encodePng(image: garbage) }
        #expect(throws: EdgeError.self) { try codec.resizePng(image: garbage, width: 2, height: 2) }
        #expect(throws: EdgeError.self) { try codec.encodeTiff(image: Data()) }
    }
}
