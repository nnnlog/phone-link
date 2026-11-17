import CoreGraphics
import Foundation
import ImageIO
import PhoneLinkEngine
import UniformTypeIdentifiers

/// ImageIO decoding and encoding shared by the image adapters.
enum EdgeImageIO {
    /// The image formats this Mac's ImageIO writes.
    private static let encodable = Set(CGImageDestinationCopyTypeIdentifiers() as? [String] ?? [])

    /// The type ImageIO detects in `data`, when it recognizes one.
    static func type(of data: Data) -> UTType? {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil),
              let identifier = CGImageSourceGetType(source)
        else { return nil }
        return UTType(identifier as String)
    }

    /// The first image of `data`.
    static func decode(_ data: Data) throws -> CGImage {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil),
              let image = CGImageSourceCreateImageAtIndex(source, 0, nil)
        else { throw EdgeError.Unexpected(message: "ImageIO decodes no image from \(data.count) bytes") }
        return image
    }

    static func canEncode(_ type: UTType) -> Bool {
        encodable.contains(type.identifier)
    }

    /// `image` encoded as `type`; `quality` (0…1) only for lossy formats.
    static func encode(_ image: CGImage, as type: UTType, quality: Double? = nil) throws -> Data {
        let output = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(output as CFMutableData, type.identifier as CFString, 1, nil) else {
            throw EdgeError.Unexpected(message: "ImageIO cannot write \(type.identifier)")
        }

        let properties = quality.map { [kCGImageDestinationLossyCompressionQuality: $0] as CFDictionary }
        CGImageDestinationAddImage(destination, image, properties)
        guard CGImageDestinationFinalize(destination) else {
            throw EdgeError.Unexpected(message: "encoding a \(image.width)×\(image.height) image as \(type.identifier) failed")
        }
        return output as Data
    }

    /// A new bitmap context of `width` × `height` in `space`, 8 bits per component.
    static func context(width: Int, height: Int, space: CGColorSpace, alpha: CGImageAlphaInfo) throws -> CGContext {
        guard let context = CGContext(
            data: nil,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: 0,
            space: space,
            bitmapInfo: alpha.rawValue
        ) else { throw EdgeError.Unexpected(message: "no \(width)×\(height) bitmap context") }
        return context
    }

    static func sRGB() throws -> CGColorSpace {
        guard let space = CGColorSpace(name: CGColorSpace.sRGB) else {
            throw EdgeError.Unexpected(message: "no sRGB color space")
        }
        return space
    }

    static func makeImage(_ context: CGContext) throws -> CGImage {
        guard let image = context.makeImage() else {
            throw EdgeError.Unexpected(message: "drawing a \(context.width)×\(context.height) image failed")
        }
        return image
    }
}
