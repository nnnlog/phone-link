import AVFoundation
import CoreMedia
import CoreVideo

/// Where a mirroring session's decoded pictures go: its window.
public protocol MirrorVideoSink: AnyObject, Sendable {
    /// Shows `picture`; `true` when it was handed to the screen.
    func present(_ picture: CVPixelBuffer) -> Bool
}

/// A window's video surface: a sample-buffer display layer that shows each decoded picture at once.
/// `present` is called on the decoding thread; the layer's renderer takes buffers from any thread.
public final class MirrorVideoLayer: MirrorVideoSink, @unchecked Sendable {
    public let layer: AVSampleBufferDisplayLayer
    private let renderer: AVSampleBufferVideoRenderer

    @MainActor
    public init() {
        layer = AVSampleBufferDisplayLayer()
        layer.videoGravity = .resize
        layer.backgroundColor = CGColor(gray: 0, alpha: 1)
        renderer = layer.sampleBufferRenderer
    }

    public func present(_ picture: CVPixelBuffer) -> Bool {
        guard let sample = Self.immediateSample(picture) else { return false }
        if renderer.status == .failed || renderer.requiresFlushToResumeDecoding {
            renderer.flush()
        }
        renderer.enqueue(sample)
        return renderer.status != .failed
    }

    /// `picture` as a sample the layer shows as soon as it gets it.
    static func immediateSample(_ picture: CVPixelBuffer) -> CMSampleBuffer? {
        var format: CMVideoFormatDescription?
        guard CMVideoFormatDescriptionCreateForImageBuffer(allocator: kCFAllocatorDefault, imageBuffer: picture, formatDescriptionOut: &format) == noErr,
              let format
        else { return nil }

        var timing = CMSampleTimingInfo(
            duration: .invalid,
            presentationTimeStamp: CMClockGetTime(CMClockGetHostTimeClock()),
            decodeTimeStamp: .invalid
        )
        var sample: CMSampleBuffer?
        guard CMSampleBufferCreateReadyWithImageBuffer(
            allocator: kCFAllocatorDefault,
            imageBuffer: picture,
            formatDescription: format,
            sampleTiming: &timing,
            sampleBufferOut: &sample
        ) == noErr, let sample else { return nil }

        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: true),
           CFArrayGetCount(attachments) > 0 {
            let first = unsafeBitCast(CFArrayGetValueAtIndex(attachments, 0), to: CFMutableDictionary.self)
            CFDictionarySetValue(
                first,
                Unmanaged.passUnretained(kCMSampleAttachmentKey_DisplayImmediately).toOpaque(),
                Unmanaged.passUnretained(kCFBooleanTrue).toOpaque()
            )
        }
        return sample
    }
}
