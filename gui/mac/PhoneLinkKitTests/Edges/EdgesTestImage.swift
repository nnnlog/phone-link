import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

/// Images the edge tests draw and encode themselves.
enum EdgesTestImage {
    static let pngSignature = Data([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])

    /// A `width` × `height` sRGB image with fine detail (so lossy encodes have work to do); unless
    /// `opaque`, its left quarter is fully transparent.
    static func draw(width: Int, height: Int, opaque: Bool = true) -> CGImage {
        let space = CGColorSpace(name: CGColorSpace.sRGB)!
        let context = CGContext(
            data: nil,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: width * 4,
            space: space,
            bitmapInfo: (opaque ? CGImageAlphaInfo.noneSkipLast : .premultipliedLast).rawValue
        )!

        let pixels = context.data!.bindMemory(to: UInt8.self, capacity: width * height * 4)
        var seed: UInt32 = 12345
        for y in 0..<height {
            for x in 0..<width {
                seed = seed &* 1_103_515_245 &+ 12345
                let noise = UInt8(truncatingIfNeeded: seed >> 16) / 4
                let offset = (y * width + x) * 4
                let transparent = !opaque && x < width / 4

                pixels[offset] = transparent ? 0 : UInt8(x * 255 / max(width - 1, 1)) / 2 + noise
                pixels[offset + 1] = transparent ? 0 : UInt8(y * 255 / max(height - 1, 1)) / 2 + noise
                pixels[offset + 2] = transparent ? 0 : 96 + noise
                pixels[offset + 3] = transparent ? 0 : 255
            }
        }

        return context.makeImage()!
    }

    /// `image` encoded as `type` with these destination properties.
    static func encode(_ image: CGImage, as type: UTType, properties: [CFString: Any] = [:]) -> Data {
        let output = NSMutableData()
        let destination = CGImageDestinationCreateWithData(output as CFMutableData, type.identifier as CFString, 1, nil)!
        CGImageDestinationAddImage(destination, image, properties as CFDictionary)
        precondition(CGImageDestinationFinalize(destination), "encoding the test image as \(type.identifier) failed")
        return output as Data
    }

    static func png(width: Int, height: Int, opaque: Bool = true) -> Data {
        encode(draw(width: width, height: height, opaque: opaque), as: .png)
    }

    static func jpeg(width: Int, height: Int, quality: Double = 1, orientation: Int = 1) -> Data {
        encode(
            draw(width: width, height: height),
            as: .jpeg,
            properties: [kCGImageDestinationLossyCompressionQuality: quality, kCGImagePropertyOrientation: orientation]
        )
    }

    static func decode(_ data: Data) -> CGImage? {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil) else { return nil }
        return CGImageSourceCreateImageAtIndex(source, 0, nil)
    }

    /// The pixel size of the first image of `data`.
    static func size(_ data: Data) -> (width: Int, height: Int)? {
        decode(data).map { ($0.width, $0.height) }
    }

    /// The type ImageIO detects in `data`.
    static func typeIdentifier(_ data: Data) -> String? {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil) else { return nil }
        return CGImageSourceGetType(source) as String?
    }

    /// The RGBA bytes of one pixel of `image`, drawn into sRGB with the origin at the top left.
    static func pixel(of image: CGImage, x: Int, y: Int) -> [UInt8] {
        let space = CGColorSpace(name: CGColorSpace.sRGB)!
        var bytes = [UInt8](repeating: 0, count: image.width * image.height * 4)

        bytes.withUnsafeMutableBytes { buffer in
            let context = CGContext(
                data: buffer.baseAddress,
                width: image.width,
                height: image.height,
                bitsPerComponent: 8,
                bytesPerRow: image.width * 4,
                space: space,
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
            )!
            context.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
        }

        let offset = (y * image.width + x) * 4
        return Array(bytes[offset..<offset + 4])
    }
}
