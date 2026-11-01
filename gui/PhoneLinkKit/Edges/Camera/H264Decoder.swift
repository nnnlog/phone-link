import CoreMedia
import Foundation
import VideoToolbox

/// Decodes the phone's H.264 stream, one Annex-B access unit at a time: a unit carrying SPS and PPS
/// (re)configures the decoder; a unit with slices decodes to an NV12 picture.
final class H264Decoder {
    private var session: VTDecompressionSession?
    private var format: CMVideoFormatDescription?
    private var sps: Data?
    private var pps: Data?

    /// The picture `unit` decodes to; `nil` for a unit without slices or one that does not decode.
    func decode(_ unit: Data) -> CVPixelBuffer? {
        var slices: [Data] = []
        var parameters = false
        for nal in Self.nalUnits(unit) {
            switch nal.first.map({ $0 & 0x1F }) {
            case 7:
                if nal != sps { sps = nal; parameters = true }
            case 8:
                if nal != pps { pps = nal; parameters = true }
            case 1, 5:
                slices.append(nal)
            default:
                break
            }
        }

        if parameters || session == nil {
            configure()
        }

        guard !slices.isEmpty, let session, let format else { return nil }
        guard let sample = Self.sample(slices, format: format) else { return nil }
        var picture: CVPixelBuffer?
        let status = VTDecompressionSessionDecodeFrame(session, sampleBuffer: sample, flags: [], infoFlagsOut: nil) { status, _, image, _, _ in
            if status == noErr {
                picture = image
            }
        }

        VTDecompressionSessionWaitForAsynchronousFrames(session)
        if status == kVTInvalidSessionErr {
            reset()
        }
        return picture
    }

    /// Forgets the stream: the next unit must carry its parameters.
    func reset() {
        if let session {
            VTDecompressionSessionInvalidate(session)
        }
        session = nil
        format = nil
        sps = nil
        pps = nil
    }

    private func configure() {
        guard let sps, let pps else { return }
        if let session {
            VTDecompressionSessionInvalidate(session)
            self.session = nil
        }

        var description: CMFormatDescription?
        let status = sps.withUnsafeBytes { spsBytes in
            pps.withUnsafeBytes { ppsBytes in
                let pointers = [
                    spsBytes.bindMemory(to: UInt8.self).baseAddress!,
                    ppsBytes.bindMemory(to: UInt8.self).baseAddress!,
                ]
                return CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    allocator: kCFAllocatorDefault,
                    parameterSetCount: 2,
                    parameterSetPointers: pointers,
                    parameterSetSizes: [sps.count, pps.count],
                    nalUnitHeaderLength: 4,
                    formatDescriptionOut: &description
                )
            }
        }
        guard status == noErr, let description else { return }

        let attributes: [CFString: Any] = [
            kCVPixelBufferPixelFormatTypeKey: PhoneCameraShared.pixelFormat,
            kCVPixelBufferIOSurfacePropertiesKey: [:] as CFDictionary,
        ]
        var session: VTDecompressionSession?
        let created = VTDecompressionSessionCreate(
            allocator: kCFAllocatorDefault,
            formatDescription: description,
            decoderSpecification: nil,
            imageBufferAttributes: attributes as CFDictionary,
            outputCallback: nil,
            decompressionSessionOut: &session
        )
        guard created == noErr, let session else { return }

        VTSessionSetProperty(session, key: kVTDecompressionPropertyKey_RealTime, value: kCFBooleanTrue)
        format = description
        self.session = session
    }

    /// The NAL units of an Annex-B byte stream, without their start codes.
    static func nalUnits(_ data: Data) -> [Data] {
        let bytes = [UInt8](data)
        var starts: [(code: Int, payload: Int)] = []
        var i = 0
        while i + 2 < bytes.count {
            if bytes[i] == 0, bytes[i + 1] == 0, bytes[i + 2] == 1 {
                let code = i > 0 && bytes[i - 1] == 0 ? i - 1 : i
                starts.append((code, i + 3))
                i += 3
            } else {
                i += 1
            }
        }

        return starts.enumerated().compactMap { index, start in
            let end = index + 1 < starts.count ? starts[index + 1].code : bytes.count
            return end > start.payload ? Data(bytes[start.payload ..< end]) : nil
        }
    }

    /// `slices` as one length-prefixed sample.
    private static func sample(_ slices: [Data], format: CMVideoFormatDescription) -> CMSampleBuffer? {
        var avcc = Data()
        for nal in slices {
            var length = UInt32(nal.count).bigEndian
            avcc.append(Data(bytes: &length, count: 4))
            avcc.append(nal)
        }

        var block: CMBlockBuffer?
        guard CMBlockBufferCreateWithMemoryBlock(
            allocator: kCFAllocatorDefault,
            memoryBlock: nil,
            blockLength: avcc.count,
            blockAllocator: kCFAllocatorDefault,
            customBlockSource: nil,
            offsetToData: 0,
            dataLength: avcc.count,
            flags: 0,
            blockBufferOut: &block
        ) == noErr, let block else { return nil }

        let copied = avcc.withUnsafeBytes {
            CMBlockBufferReplaceDataBytes(with: $0.baseAddress!, blockBuffer: block, offsetIntoDestination: 0, dataLength: avcc.count)
        }
        guard copied == noErr else { return nil }

        var sample: CMSampleBuffer?
        var size = avcc.count
        guard CMSampleBufferCreateReady(
            allocator: kCFAllocatorDefault,
            dataBuffer: block,
            formatDescription: format,
            sampleCount: 1,
            sampleTimingEntryCount: 0,
            sampleTimingArray: nil,
            sampleSizeEntryCount: 1,
            sampleSizeArray: &size,
            sampleBufferOut: &sample
        ) == noErr else { return nil }
        return sample
    }
}
