import Foundation
import ImageIO
import PhoneLinkEngine
import Testing
import UniformTypeIdentifiers
@testable import PhoneLinkKit

struct AttachmentImagerTests {
    private let imager = AttachmentImager()

    @Test func sniffingNamesTheMIMETypeOfTheContent() {
        #expect(imager.sniffContentType(bytes: EdgesTestImage.png(width: 8, height: 8)) == "image/png")
        #expect(imager.sniffContentType(bytes: EdgesTestImage.jpeg(width: 8, height: 8)) == "image/jpeg")
        let gif = EdgesTestImage.encode(EdgesTestImage.draw(width: 8, height: 8), as: .gif)
        #expect(imager.sniffContentType(bytes: gif) == "image/gif")
        #expect(imager.sniffContentType(bytes: Data("BEGIN:VCARD".utf8)) == nil)
        #expect(imager.sniffContentType(bytes: Data()) == nil)
    }

    @Test func anImageWithinBothLimitsComesBackUnchanged() throws {
        let png = EdgesTestImage.png(width: 100, height: 80)
        let fitted = try #require(try imager.fitImage(bytes: png, maxWidth: 100, maxHeight: 80, maxBytes: UInt64(png.count)))
        #expect(fitted.bytes == png)
        #expect(fitted.contentType == "image/png")
    }

    @Test func aLargeImageIsScaledIntoTheBoxKeepingItsAspectRatio() throws {
        let jpeg = EdgesTestImage.jpeg(width: 1600, height: 1000)
        let fitted = try #require(try imager.fitImage(bytes: jpeg, maxWidth: 640, maxHeight: 480, maxBytes: 10_000_000))
        #expect(fitted.contentType == "image/jpeg")
        #expect(EdgesTestImage.size(fitted.bytes)! == (640, 400))
    }

    @Test func anImageOverItsByteBudgetIsReencodedAtTheSameSize() throws {
        let jpeg = EdgesTestImage.jpeg(width: 800, height: 600, quality: 1)
        let budget = UInt64(jpeg.count / 4)
        let fitted = try #require(try imager.fitImage(bytes: jpeg, maxWidth: 1000, maxHeight: 1000, maxBytes: budget))
        #expect(fitted.contentType == "image/jpeg")
        #expect(UInt64(fitted.bytes.count) <= budget)
        #expect(EdgesTestImage.size(fitted.bytes)! == (800, 600))
    }

    @Test func aLosslessImageOverItsBudgetBecomesJPEGOverWhite() throws {
        let png = EdgesTestImage.png(width: 300, height: 200, opaque: false)
        let budget = UInt64(png.count / 2)
        let fitted = try #require(try imager.fitImage(bytes: png, maxWidth: 1000, maxHeight: 1000, maxBytes: budget))
        #expect(fitted.contentType == "image/jpeg")
        #expect(EdgesTestImage.typeIdentifier(fitted.bytes) == UTType.jpeg.identifier)
        #expect(UInt64(fitted.bytes.count) <= budget)

        let decoded = try #require(EdgesTestImage.decode(fitted.bytes))
        let corner = EdgesTestImage.pixel(of: decoded, x: 2, y: 2)
        #expect(corner[0] > 240 && corner[1] > 240 && corner[2] > 240, "the transparent quarter is white, not black")
    }

    @Test(.enabled(if: EdgeImageIO.canEncode(.heic), "this Mac's ImageIO writes HEIC"))
    func aHEICImageStaysHEIC() throws {
        let heic = EdgesTestImage.encode(
            EdgesTestImage.draw(width: 600, height: 400),
            as: .heic,
            properties: [kCGImageDestinationLossyCompressionQuality: 1.0]
        )

        let fitted = try #require(try imager.fitImage(bytes: heic, maxWidth: 300, maxHeight: 300, maxBytes: 10_000_000))
        #expect(fitted.contentType == "image/heic")
        #expect(EdgesTestImage.size(fitted.bytes)! == (300, 200))
    }

    @Test func theOrientationIsAppliedBeforeFitting() throws {
        // Stored 200 × 100, shown 100 × 200 (orientation 6: rotated a quarter).
        let jpeg = EdgesTestImage.jpeg(width: 200, height: 100, orientation: 6)
        let fitted = try #require(try imager.fitImage(bytes: jpeg, maxWidth: 50, maxHeight: 50, maxBytes: 10_000_000))
        #expect(EdgesTestImage.size(fitted.bytes)! == (25, 50))
    }

    @Test func aPNGThatFitsOnceScaledStaysPNG() throws {
        let png = EdgesTestImage.png(width: 1200, height: 600)
        let fitted = try #require(try imager.fitImage(bytes: png, maxWidth: 600, maxHeight: 600, maxBytes: 10_000_000))
        #expect(fitted.contentType == "image/png")
        #expect(EdgesTestImage.size(fitted.bytes)! == (600, 300))
    }

    @Test func anAnimatedGIFKeepsItsFramesAndDelays() throws {
        let gif = Self.animatedGIF(frames: 3, width: 400, height: 200, delay: 0.25)
        let fitted = try #require(try imager.fitImage(bytes: gif, maxWidth: 200, maxHeight: 200, maxBytes: 10_000_000))
        #expect(fitted.contentType == "image/gif")
        let source = try #require(CGImageSourceCreateWithData(fitted.bytes as CFData, nil))
        #expect(CGImageSourceGetCount(source) == 3)
        #expect(EdgesTestImage.size(fitted.bytes)! == (200, 100))

        let frame = CGImageSourceCopyPropertiesAtIndex(source, 2, nil) as? [CFString: Any]
        let delay = (frame?[kCGImagePropertyGIFDictionary] as? [CFString: Any])?[kCGImagePropertyGIFDelayTime] as? Double
        #expect(delay == 0.25)
    }

    @Test func whenNoQualityFitsTheImageIsMadeSmallerUntilItDoes() throws {
        let jpeg = EdgesTestImage.jpeg(width: 640, height: 480)
        let lowest = try EdgeImageIO.encode(EdgesTestImage.draw(width: 640, height: 480), as: .jpeg, quality: 0.1)
        let budget = UInt64(lowest.count / 3)
        let fitted = try #require(try imager.fitImage(bytes: jpeg, maxWidth: 1000, maxHeight: 1000, maxBytes: budget))
        #expect(fitted.contentType == "image/jpeg")
        #expect(UInt64(fitted.bytes.count) <= budget)

        let size = try #require(EdgesTestImage.size(fitted.bytes))
        #expect(size.width < 640 && size.height < 480)
        #expect(abs(Double(size.width) / Double(size.height) - 640.0 / 480.0) < 0.05)
    }

    @Test func anImageThatFitsNoBudgetIsAnError() {
        let jpeg = EdgesTestImage.jpeg(width: 320, height: 240)
        #expect(throws: EdgeError.self) {
            try imager.fitImage(bytes: jpeg, maxWidth: 1000, maxHeight: 1000, maxBytes: 1)
        }
    }

    @Test func theSourceFormatIsTriedBeforeJPEG() {
        #expect(AttachmentImager.outputFormats(for: .png) == [.png, .jpeg])
        #expect(AttachmentImager.outputFormats(for: .jpeg) == [.jpeg])
        #expect(AttachmentImager.outputFormats(for: .gif) == [.gif, .jpeg])
    }

    /// A GIF of `frames` frames, each shown `delay` seconds.
    private static func animatedGIF(frames: Int, width: Int, height: Int, delay: Double) -> Data {
        let output = NSMutableData()
        let destination = CGImageDestinationCreateWithData(output as CFMutableData, UTType.gif.identifier as CFString, frames, nil)!
        CGImageDestinationSetProperties(destination, [kCGImagePropertyGIFDictionary: [kCGImagePropertyGIFLoopCount: 0]] as CFDictionary)

        for _ in 0..<frames {
            let properties = [kCGImagePropertyGIFDictionary: [kCGImagePropertyGIFDelayTime: delay]] as CFDictionary
            CGImageDestinationAddImage(destination, EdgesTestImage.draw(width: width, height: height), properties)
        }

        precondition(CGImageDestinationFinalize(destination), "encoding the test GIF failed")
        return output as Data
    }

    @Test func bytesThatAreNoImageAreNotTransformed() throws {
        #expect(try imager.fitImage(bytes: Data("hello".utf8), maxWidth: 10, maxHeight: 10, maxBytes: 1) == nil)
        #expect(try imager.fitImage(bytes: Data(), maxWidth: 10, maxHeight: 10, maxBytes: 1) == nil)
    }
}
