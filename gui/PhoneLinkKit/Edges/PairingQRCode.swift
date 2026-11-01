import CoreGraphics
import CoreImage

/// The QR code a pairing session shows: the session's link encoded byte for byte as given (UTF-8, no
/// rewriting), error-correction level L, with a quiet zone of four modules around the symbol.
public enum PairingQRCode {
    /// Modules of light margin on each side.
    public static let quietZone = 4

    /// The code as a grayscale image of one pixel per module (black on white), quiet zone included;
    /// draw it scaled without interpolation. `nil` when the text does not fit a QR code.
    public static func image(for text: String) -> CGImage? {
        guard let filter = CIFilter(name: "CIQRCodeGenerator") else { return nil }
        filter.setValue(Data(text.utf8), forKey: "inputMessage")
        filter.setValue("L", forKey: "inputCorrectionLevel")

        guard let output = filter.outputImage,
              let generated = CIContext().createCGImage(output, from: output.extent),
              let modules = darkModules(of: generated)
        else { return nil }
        return render(modules)
    }

    /// The symbol's modules (`true` dark), rows from the top, without the generator's own margin: the
    /// bounding box of the dark pixels, which the three finder patterns span.
    static func darkModules(of image: CGImage) -> [[Bool]]? {
        let (width, height) = (image.width, image.height)
        var pixels = [UInt8](repeating: 255, count: width * height)
        let drawn = pixels.withUnsafeMutableBytes { buffer -> Bool in
            guard let context = CGContext(
                data: buffer.baseAddress,
                width: width,
                height: height,
                bitsPerComponent: 8,
                bytesPerRow: width,
                space: CGColorSpaceCreateDeviceGray(),
                bitmapInfo: CGImageAlphaInfo.none.rawValue
            ) else { return false }

            context.interpolationQuality = .none
            context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
            return true
        }
        guard drawn else { return nil }

        let dark = { (x: Int, y: Int) in pixels[y * width + x] < 128 }
        let rows = (0..<height).filter { y in (0..<width).contains { dark($0, y) } }
        let columns = (0..<width).filter { x in (0..<height).contains { dark(x, $0) } }
        guard let top = rows.first, let bottom = rows.last, let left = columns.first, let right = columns.last,
              bottom - top == right - left
        else { return nil }
        return (top...bottom).map { y in (left...right).map { x in dark(x, y) } }
    }

    private static func render(_ modules: [[Bool]]) -> CGImage? {
        let side = modules.count + 2 * quietZone
        var pixels = [UInt8](repeating: 255, count: side * side)
        for (y, row) in modules.enumerated() {
            for (x, isDark) in row.enumerated() where isDark {
                pixels[(y + quietZone) * side + x + quietZone] = 0
            }
        }

        guard let provider = CGDataProvider(data: Data(pixels) as CFData) else { return nil }
        return CGImage(
            width: side,
            height: side,
            bitsPerComponent: 8,
            bitsPerPixel: 8,
            bytesPerRow: side,
            space: CGColorSpaceCreateDeviceGray(),
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.none.rawValue),
            provider: provider,
            decode: nil,
            shouldInterpolate: false,
            intent: .defaultIntent
        )
    }
}
