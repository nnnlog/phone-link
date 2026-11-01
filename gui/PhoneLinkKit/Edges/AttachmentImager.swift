import CoreGraphics
import Foundation
import ImageIO
import PhoneLinkEngine
import UniformTypeIdentifiers

/// Outgoing message attachments fitted to the carrier limits: ImageIO type detection, thumbnailing
/// and re-encoding.
public final class AttachmentImager: AttachmentImaging {
    /// The qualities a lossy format is encoded at, tried in order until the image fits its budget.
    private static let qualities: [Double] = [0.9, 0.8, 0.7, 0.6, 0.5, 0.4, 0.3, 0.2, 0.1]
    /// The lossy formats; any other is encoded once per size.
    private static let lossyFormats: Set<UTType> = [.jpeg, .heic]
    /// When no encoding of a size fits the budget, the next size tried is this share of it…
    private static let shrinkFactor = 0.75
    /// …down to a longest side of this many pixels.
    static let smallestSide = 16

    public init() {}

    public func sniffContentType(bytes: Data) -> String? {
        EdgeImageIO.type(of: bytes)?.preferredMIMEType
    }

    /// Unchanged when the image already fits both limits. Otherwise the largest size within the
    /// limits (aspect ratio kept, never enlarged, its EXIF orientation applied) that an encoding
    /// fits into `maxBytes`: at each size the source's own format when ImageIO writes it — every
    /// frame, for a GIF — then JPEG, a lossy format at decreasing quality; a size that none fits is
    /// shrunk by `shrinkFactor`. Throws when nothing fits at `smallestSide` (the engine then sends
    /// the original); `nil` when the bytes are no image ImageIO can transform.
    public func fitImage(bytes: Data, maxWidth: UInt32, maxHeight: UInt32, maxBytes: UInt64) throws -> FittedImage? {
        guard let source = CGImageSourceCreateWithData(bytes as CFData, nil),
              let identifier = CGImageSourceGetType(source),
              let type = UTType(identifier as String), type.conforms(to: .image),
              let contentType = type.preferredMIMEType,
              let size = Self.shownSize(source)
        else { return nil }
        guard maxWidth > 0, maxHeight > 0 else {
            throw EdgeError.Unexpected(message: "no image fits within \(maxWidth)×\(maxHeight)")
        }

        let scale = min(Double(maxWidth) / Double(size.width), Double(maxHeight) / Double(size.height), 1)
        if scale == 1, UInt64(bytes.count) <= maxBytes {
            return FittedImage(bytes: bytes, contentType: contentType)
        }

        let formats = Self.outputFormats(for: type)
        var side = max(1, Int((Double(max(size.width, size.height)) * scale).rounded(.down)))
        while true {
            for format in formats {
                if let fitted = try Self.encode(source, as: format, longestSide: side, within: maxBytes) {
                    return fitted
                }
            }

            let next = Int((Double(side) * Self.shrinkFactor).rounded(.down))
            guard next >= Self.smallestSide else {
                throw EdgeError.Unexpected(message: "no encoding of the image fits \(maxBytes) bytes, down to \(side) pixels")
            }
            side = next
        }
    }

    /// The formats tried at each size: the source's own when ImageIO writes it, then JPEG.
    static func outputFormats(for source: UTType) -> [UTType] {
        (EdgeImageIO.canEncode(source) ? [source] : []) + (source == .jpeg ? [] : [.jpeg])
    }

    /// The image at `longestSide` as `format` when that fits `maxBytes`: a lossy format at the first
    /// quality that fits, a GIF with all its frames, any other format once.
    private static func encode(
        _ source: CGImageSource,
        as format: UTType,
        longestSide: Int,
        within maxBytes: UInt64
    ) throws -> FittedImage? {
        guard let mimeType = format.preferredMIMEType else {
            preconditionFailure("\(format.identifier) has a MIME type")
        }

        if format == .gif {
            let encoded = try animatedGIF(source, longestSide: longestSide)
            return UInt64(encoded.count) <= maxBytes ? FittedImage(bytes: encoded, contentType: mimeType) : nil
        }

        let thumbnail = try thumbnail(source, index: 0, longestSide: longestSide)
        let image = format == .jpeg && hasAlpha(thumbnail) ? try overWhite(thumbnail) : thumbnail
        let qualities: [Double?] = lossyFormats.contains(format) ? Self.qualities : [nil]

        for quality in qualities {
            let encoded = try EdgeImageIO.encode(image, as: format, quality: quality)
            if UInt64(encoded.count) <= maxBytes {
                return FittedImage(bytes: encoded, contentType: mimeType)
            }
        }
        return nil
    }

    /// Every frame of the GIF at `longestSide`, each with its delay, and the file's loop count.
    private static func animatedGIF(_ source: CGImageSource, longestSide: Int) throws -> Data {
        let count = CGImageSourceGetCount(source)
        let output = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(output as CFMutableData, UTType.gif.identifier as CFString, count, nil) else {
            throw EdgeError.Unexpected(message: "ImageIO cannot write GIF")
        }

        let file = (CGImageSourceCopyProperties(source, nil) as? [CFString: Any])?[kCGImagePropertyGIFDictionary] as? [CFString: Any]
        if let loops = file?[kCGImagePropertyGIFLoopCount] {
            CGImageDestinationSetProperties(destination, [kCGImagePropertyGIFDictionary: [kCGImagePropertyGIFLoopCount: loops]] as CFDictionary)
        }

        for index in 0..<count {
            let frame = try thumbnail(source, index: index, longestSide: longestSide)
            let properties = (CGImageSourceCopyPropertiesAtIndex(source, index, nil) as? [CFString: Any])?[kCGImagePropertyGIFDictionary] as? [CFString: Any]
            let delays = properties?.filter { $0.key == kCGImagePropertyGIFDelayTime || $0.key == kCGImagePropertyGIFUnclampedDelayTime } ?? [:]
            CGImageDestinationAddImage(destination, frame, delays.isEmpty ? nil : [kCGImagePropertyGIFDictionary: delays] as CFDictionary)
        }

        guard CGImageDestinationFinalize(destination) else {
            throw EdgeError.Unexpected(message: "encoding a \(count)-frame GIF failed")
        }
        return output as Data
    }

    /// Frame `index` drawn with its orientation, its longest side `longestSide` pixels at most.
    private static func thumbnail(_ source: CGImageSource, index: Int, longestSide: Int) throws -> CGImage {
        let options = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: longestSide,
        ] as CFDictionary

        guard let image = CGImageSourceCreateThumbnailAtIndex(source, index, options) else {
            throw EdgeError.Unexpected(message: "ImageIO made no \(longestSide)-pixel image of frame \(index)")
        }
        return image
    }

    /// The size the first image shows at: its pixel size, width and height swapped when its EXIF
    /// orientation turns it a quarter (5–8), as the thumbnail is drawn.
    private static func shownSize(_ source: CGImageSource) -> (width: Int, height: Int)? {
        guard let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
              let width = properties[kCGImagePropertyPixelWidth] as? Int, width > 0,
              let height = properties[kCGImagePropertyPixelHeight] as? Int, height > 0
        else { return nil }

        let orientation = properties[kCGImagePropertyOrientation] as? Int ?? 1
        return (5...8).contains(orientation) ? (height, width) : (width, height)
    }

    private static func hasAlpha(_ image: CGImage) -> Bool {
        switch image.alphaInfo {
        case .none, .noneSkipFirst, .noneSkipLast:
            false
        default:
            true
        }
    }

    /// The image drawn over white for JPEG, which has no alpha: its transparent pixels would come
    /// out black.
    private static func overWhite(_ image: CGImage) throws -> CGImage {
        let context = try EdgeImageIO.context(width: image.width, height: image.height, space: EdgeImageIO.sRGB(), alpha: .noneSkipLast)
        let bounds = CGRect(x: 0, y: 0, width: image.width, height: image.height)
        context.setFillColor(CGColor(gray: 1, alpha: 1))
        context.fill(bounds)
        context.draw(image, in: bounds)
        return try EdgeImageIO.makeImage(context)
    }
}
