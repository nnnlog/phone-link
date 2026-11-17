import AppKit
import CoreMedia
import CoreVideo
import PhoneLinkEngine
import VideoToolbox

/// The camera's 1280 × 720 NV12 frames: decoded pictures copied in plane by plane with each buffer's
/// own row strides (a picture of another size is cut to the frame or padded with black), still images
/// drawn and converted, and either wrapped as a sample for the feed.
final class CameraFrames {
    private let pool: CVPixelBufferPool
    private var transfer: VTPixelTransferSession?

    init() throws {
        let attributes: [CFString: Any] = [
            kCVPixelBufferWidthKey: Int(PhoneCameraShared.width),
            kCVPixelBufferHeightKey: Int(PhoneCameraShared.height),
            kCVPixelBufferPixelFormatTypeKey: PhoneCameraShared.pixelFormat,
            kCVPixelBufferIOSurfacePropertiesKey: [:] as CFDictionary,
        ]

        var pool: CVPixelBufferPool?
        let status = CVPixelBufferPoolCreate(kCFAllocatorDefault, nil, attributes as CFDictionary, &pool)
        guard status == kCVReturnSuccess, let pool else { throw CameraLinkError.status("creating the frame pool", status) }
        self.pool = pool
    }

    private func newFrame() -> CVPixelBuffer? {
        var frame: CVPixelBuffer?
        CVPixelBufferPoolCreatePixelBuffer(kCFAllocatorDefault, pool, &frame)
        return frame
    }

    /// `picture` (NV12) in a camera frame.
    func frame(from picture: CVPixelBuffer) -> CVPixelBuffer? {
        guard CVPixelBufferGetPlaneCount(picture) == 2, let frame = newFrame() else { return nil }

        CVPixelBufferLockBaseAddress(picture, .readOnly)
        CVPixelBufferLockBaseAddress(frame, [])
        defer {
            CVPixelBufferUnlockBaseAddress(frame, [])
            CVPixelBufferUnlockBaseAddress(picture, .readOnly)
        }

        for (plane, fill) in [(0, UInt8(16)), (1, UInt8(128))] {
            guard let source = CVPixelBufferGetBaseAddressOfPlane(picture, plane),
                  let target = CVPixelBufferGetBaseAddressOfPlane(frame, plane)
            else { return nil }

            let sourceStride = CVPixelBufferGetBytesPerRowOfPlane(picture, plane)
            let targetStride = CVPixelBufferGetBytesPerRowOfPlane(frame, plane)
            let targetRows = CVPixelBufferGetHeightOfPlane(frame, plane)
            let rows = min(CVPixelBufferGetHeightOfPlane(picture, plane), targetRows)
            // Two bytes per chroma sample pair, one per luma sample: the row's bytes are its width.
            let rowBytes = min(CVPixelBufferGetWidthOfPlane(picture, plane), CVPixelBufferGetWidthOfPlane(frame, plane))
                * (plane == 0 ? 1 : 2)

            if rows < targetRows || rowBytes < targetStride {
                memset(target, Int32(fill), targetStride * targetRows)
            }
            for row in 0 ..< rows {
                memcpy(target + row * targetStride, source + row * sourceStride, rowBytes)
            }
        }
        return frame
    }

    /// `image` (any size; drawn to fill) in a camera frame.
    func frame(from image: CGImage) -> CVPixelBuffer? {
        var bgra: CVPixelBuffer?
        let width = Int(PhoneCameraShared.width)
        let height = Int(PhoneCameraShared.height)
        guard CVPixelBufferCreate(kCFAllocatorDefault, width, height, kCVPixelFormatType_32BGRA, nil, &bgra) == kCVReturnSuccess,
              let bgra, let frame = newFrame()
        else { return nil }

        CVPixelBufferLockBaseAddress(bgra, [])
        if let context = CGContext(
            data: CVPixelBufferGetBaseAddress(bgra),
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: CVPixelBufferGetBytesPerRow(bgra),
            space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue
        ) {
            context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
        }
        CVPixelBufferUnlockBaseAddress(bgra, [])

        if transfer == nil {
            VTPixelTransferSessionCreate(allocator: kCFAllocatorDefault, pixelTransferSessionOut: &transfer)
        }
        guard let transfer, VTPixelTransferSessionTransferImage(transfer, from: bgra, to: frame) == noErr else { return nil }
        return frame
    }

    /// `frame` as a sample stamped now.
    static func sample(_ frame: CVPixelBuffer) -> CMSampleBuffer? {
        var format: CMVideoFormatDescription?
        guard CMVideoFormatDescriptionCreateForImageBuffer(allocator: kCFAllocatorDefault, imageBuffer: frame, formatDescriptionOut: &format) == noErr,
              let format
        else { return nil }

        var timing = CMSampleTimingInfo(
            duration: CMTime(value: 1, timescale: PhoneCameraShared.framesPerSecond),
            presentationTimeStamp: CMClockGetTime(CMClockGetHostTimeClock()),
            decodeTimeStamp: .invalid
        )
        var sample: CMSampleBuffer?
        guard CMSampleBufferCreateReadyWithImageBuffer(
            allocator: kCFAllocatorDefault,
            imageBuffer: frame,
            formatDescription: format,
            sampleTiming: &timing,
            sampleBufferOut: &sample
        ) == noErr else { return nil }
        return sample
    }
}

/// The still images a camera shows while no video does.
enum CameraPlaceholderArt {
    /// Images of the connecting animation.
    static let connectingFrames = 16

    nonisolated(unsafe) private static let cache = NSCache<NSString, CGImage>()  // NSCache is thread-safe

    /// `placeholder`'s image; `frame` picks the connecting animation's image.
    static func image(_ placeholder: CameraPlaceholder, frame: Int = 0) -> CGImage? {
        let key = "\(placeholder)-\(frame)" as NSString
        if let cached = cache.object(forKey: key) {
            return cached
        }

        let image = draw(placeholder, frame: frame)
        if let image {
            cache.setObject(image, forKey: key)
        }
        return image
    }

    private static func draw(_ placeholder: CameraPlaceholder, frame: Int = 0) -> CGImage? {
        let size = NSSize(width: CGFloat(PhoneCameraShared.width), height: CGFloat(PhoneCameraShared.height))
        let image = NSImage(size: size, flipped: false) { rect in
            NSColor(white: placeholder == .black ? 0 : 0.11, alpha: 1).setFill()
            rect.fill()

            guard let (symbol, text) = content(placeholder) else { return true }
            let center = NSPoint(x: rect.midX, y: rect.midY + 50)
            if placeholder == .connecting {
                drawSpinner(at: center, step: frame)
            } else if let glyph = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)?
                .withSymbolConfiguration(.init(pointSize: 120, weight: .regular).applying(.init(paletteColors: [.white])))
            {
                let glyphSize = glyph.size
                glyph.draw(in: NSRect(
                    x: center.x - glyphSize.width / 2,
                    y: center.y - glyphSize.height / 2,
                    width: glyphSize.width,
                    height: glyphSize.height
                ))
            }

            let style = NSMutableParagraphStyle()
            style.alignment = .center
            let attributes: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: 40, weight: .medium),
                .foregroundColor: NSColor.white,
                .paragraphStyle: style,
            ]
            (text as NSString).draw(in: NSRect(x: 80, y: rect.midY - 160, width: rect.width - 160, height: 110), withAttributes: attributes)
            return true
        }

        var proposed = NSRect(origin: .zero, size: size)
        return image.cgImage(forProposedRect: &proposed, context: nil, hints: nil)
    }

    private static func content(_ placeholder: CameraPlaceholder) -> (String, String)? {
        switch placeholder {
        case .black: nil
        case .connecting: ("", String(localized: "Connecting to your phone…"))
        case .paused: ("pause.circle", String(localized: "Paused", comment: "The phone camera's feed"))
        case .phoneLocked: ("lock.iphone", String(localized: "Check your phone"))
        case .disconnected: ("iphone.slash", String(localized: "Your phone is disconnected"))
        case .startStreamNotification: ("hand.tap", String(localized: "Tap the notification on your phone to start"))
        case .error: ("exclamationmark.triangle", String(localized: "The camera stopped"))
        }
    }

    /// A tick per animation image around `center`, the one at `step` brightest.
    private static func drawSpinner(at center: NSPoint, step: Int) {
        let ticks = connectingFrames
        let lead = step % ticks
        for tick in 0 ..< ticks {
            let angle = CGFloat.pi / 2 - CGFloat(tick) * 2 * .pi / CGFloat(ticks)
            let age = (lead - tick + ticks) % ticks
            NSColor(white: 1, alpha: max(0.2, 1 - CGFloat(age) * 0.06)).setStroke()

            let path = NSBezierPath()
            path.lineWidth = 12
            path.lineCapStyle = .round
            path.move(to: NSPoint(x: center.x + cos(angle) * 34, y: center.y + sin(angle) * 34))
            path.line(to: NSPoint(x: center.x + cos(angle) * 64, y: center.y + sin(angle) * 64))
            path.stroke()
        }
    }
}
