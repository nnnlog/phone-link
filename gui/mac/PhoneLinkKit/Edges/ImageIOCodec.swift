import CoreGraphics
import Foundation
import PhoneLinkEngine
import UniformTypeIdentifiers

/// The clipboard's image codec for image re-encoding: ImageIO decoding and PNG/TIFF encoding, Core
/// Graphics resampling. Every method decodes its input and throws when it cannot; the size loop is
/// engine code.
public final class ImageIOCodec: ClipboardImageCodec {
    public init() {}

    public func size(image: Data) throws -> ClipboardImageSize {
        let decoded = try EdgeImageIO.decode(image)
        return ClipboardImageSize(width: UInt32(decoded.width), height: UInt32(decoded.height))
    }

    /// Full size; the alpha channel is kept.
    public func encodePng(image: Data) throws -> Data {
        try EdgeImageIO.encode(EdgeImageIO.decode(image), as: .png)
    }

    /// Redrawn into a new 32-bit RGBA bitmap with high-quality interpolation, in the image's own
    /// RGB color space when a bitmap can use it, else sRGB.
    public func resizePng(image: Data, width: UInt32, height: UInt32) throws -> Data {
        let source = try EdgeImageIO.decode(image)
        guard width > 0, height > 0 else {
            throw EdgeError.Unexpected(message: "cannot resize to \(width)×\(height)")
        }

        let space = try source.colorSpace.flatMap { $0.model == .rgb && $0.supportsOutput ? $0 : nil } ?? EdgeImageIO.sRGB()
        let context = try EdgeImageIO.context(width: Int(width), height: Int(height), space: space, alpha: .premultipliedLast)
        context.interpolationQuality = .high
        context.draw(source, in: CGRect(x: 0, y: 0, width: Int(width), height: Int(height)))
        return try EdgeImageIO.encode(EdgeImageIO.makeImage(context), as: .png)
    }

    /// For the pasteboard, next to the PNG.
    public func encodeTiff(image: Data) throws -> Data {
        try EdgeImageIO.encode(EdgeImageIO.decode(image), as: .tiff)
    }
}
