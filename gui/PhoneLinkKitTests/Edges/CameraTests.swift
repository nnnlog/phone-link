import CoreMedia
import CoreVideo
import Foundation
import Testing
import PhoneLinkEngine
import VideoToolbox
@testable import PhoneLinkKit

/// A 1280 × `height` NV12 picture whose luma rows carry their row number and whose chroma rows carry
/// 100 + their row number.
private func picture(height: Int) -> CVPixelBuffer {
    var created: CVPixelBuffer?
    let attributes: [CFString: Any] = [kCVPixelBufferIOSurfacePropertiesKey: [:] as CFDictionary]
    CVPixelBufferCreate(kCFAllocatorDefault, 1280, height, PhoneCameraShared.pixelFormat, attributes as CFDictionary, &created)
    let buffer = created!

    CVPixelBufferLockBaseAddress(buffer, [])
    for plane in 0 ..< 2 {
        let base = CVPixelBufferGetBaseAddressOfPlane(buffer, plane)!
        let stride = CVPixelBufferGetBytesPerRowOfPlane(buffer, plane)
        for row in 0 ..< CVPixelBufferGetHeightOfPlane(buffer, plane) {
            memset(base + row * stride, Int32((plane == 0 ? row : 100 + row) & 0xFF), stride)
        }
    }

    CVPixelBufferUnlockBaseAddress(buffer, [])
    return buffer
}

private func byte(_ buffer: CVPixelBuffer, plane: Int, row: Int) -> UInt8 {
    CVPixelBufferLockBaseAddress(buffer, .readOnly)
    defer { CVPixelBufferUnlockBaseAddress(buffer, .readOnly) }
    let base = CVPixelBufferGetBaseAddressOfPlane(buffer, plane)!
    return base.load(fromByteOffset: row * CVPixelBufferGetBytesPerRowOfPlane(buffer, plane) + 10, as: UInt8.self)
}

@Test func nalUnitsSplitOnThreeAndFourByteStartCodes() {
    let stream = Data([0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 4, 5, 6])
    let units = H264Decoder.nalUnits(stream)
    #expect(units == [Data([0x67, 1, 2]), Data([0x68, 3]), Data([0x65, 4, 5, 6])])
    #expect(H264Decoder.nalUnits(Data([1, 2, 3])).isEmpty)
}

@Test func deviceUIDsAreStablePerPhone() {
    let a = PhoneCameraShared.deviceUID(for: "phone-a")
    #expect(a == PhoneCameraShared.deviceUID(for: "phone-a"))
    #expect(a != PhoneCameraShared.deviceUID(for: "phone-b"))
    #expect(UUID(uuidString: a) != nil)
    #expect(Array(a)[14] == "5", "a name-based UUID")
}

@Test func propertyNamesUseFourCharacterCodes() {
    #expect(PhoneCameraShared.propertyName(PhoneCameraShared.camerasSelector) == "4cc_plcm_glob_0000")
    #expect(PhoneCameraShared.fourCharString(PhoneCameraShared.fourCharCode("plcl")) == "plcl")
}

@Test func aTallerPictureKeepsItsTopRowsAndColours() throws {
    let frames = try CameraFrames()
    let frame = try #require(frames.frame(from: picture(height: 736)))
    #expect(CVPixelBufferGetHeight(frame) == 720)
    #expect(byte(frame, plane: 0, row: 0) == 0)
    #expect(byte(frame, plane: 0, row: 719) == UInt8(719 & 0xFF))
    #expect(byte(frame, plane: 1, row: 0) == 100)
    #expect(byte(frame, plane: 1, row: 359) == UInt8((100 + 359) & 0xFF))
}

@Test func aShorterPictureIsPaddedWithBlack() throws {
    let frames = try CameraFrames()
    let frame = try #require(frames.frame(from: picture(height: 480)))
    #expect(byte(frame, plane: 0, row: 479) == UInt8(479 & 0xFF))
    #expect(byte(frame, plane: 0, row: 600) == 16)
    #expect(byte(frame, plane: 1, row: 300) == 128)
}

@Test func placeholdersDrawIntoCameraFrames() throws {
    let frames = try CameraFrames()
    for placeholder in [CameraPlaceholder.black, .connecting, .paused, .phoneLocked, .disconnected, .startStreamNotification, .error] {
        let image = try #require(CameraPlaceholderArt.image(placeholder, frame: 3))
        #expect(frames.frame(from: image) != nil)
    }
}

/// Encodes one picture with VideoToolbox's H.264 encoder into an Annex-B unit carrying SPS and PPS,
/// as the phone sends a key frame.
func encodedKeyFrame() throws -> Data {
    var created: VTCompressionSession?
    VTCompressionSessionCreate(
        allocator: nil, width: 1280, height: 720, codecType: kCMVideoCodecType_H264,
        encoderSpecification: nil, imageBufferAttributes: nil, compressedDataAllocator: nil,
        outputCallback: nil, refcon: nil, compressionSessionOut: &created
    )

    let session = try #require(created)
    var unit = Data()
    let startCode = Data([0, 0, 0, 1])

    let status = VTCompressionSessionEncodeFrame(
        session, imageBuffer: picture(height: 720), presentationTimeStamp: .zero, duration: .invalid,
        frameProperties: [kVTEncodeFrameOptionKey_ForceKeyFrame: true] as CFDictionary, infoFlagsOut: nil
    ) { status, _, sample in
        guard status == noErr, let sample, let format = CMSampleBufferGetFormatDescription(sample) else { return }

        var count = 0
        CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format, parameterSetIndex: 0, parameterSetPointerOut: nil, parameterSetSizeOut: nil, parameterSetCountOut: &count, nalUnitHeaderLengthOut: nil)
        for index in 0 ..< count {
            var pointer: UnsafePointer<UInt8>?
            var size = 0
            CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format, parameterSetIndex: index, parameterSetPointerOut: &pointer, parameterSetSizeOut: &size, parameterSetCountOut: nil, nalUnitHeaderLengthOut: nil)
            unit += startCode + Data(bytes: pointer!, count: size)
        }

        let block = CMSampleBufferGetDataBuffer(sample)!
        var length = 0
        var data: UnsafeMutablePointer<CChar>?
        CMBlockBufferGetDataPointer(block, atOffset: 0, lengthAtOffsetOut: nil, totalLengthOut: &length, dataPointerOut: &data)
        let bytes = Data(bytes: data!, count: length)

        var offset = 0
        while offset + 4 <= bytes.count {
            let size = bytes[offset ..< offset + 4].reduce(0) { $0 << 8 | Int($1) }
            unit += startCode + bytes[offset + 4 ..< offset + 4 + size]
            offset += 4 + size
        }
    }

    #expect(status == noErr)
    VTCompressionSessionCompleteFrames(session, untilPresentationTimeStamp: .invalid)
    VTCompressionSessionInvalidate(session)
    return unit
}

@Test func anAnnexBKeyFrameDecodesToAPicture() throws {
    let decoder = H264Decoder()
    let unit = try encodedKeyFrame()
    let picture = try #require(decoder.decode(unit))
    #expect(CVPixelBufferGetWidth(picture) == 1280)
    #expect(CVPixelBufferGetHeight(picture) == 720)
    #expect(CVPixelBufferGetPixelFormatType(picture) == PhoneCameraShared.pixelFormat)

    // Parameters alone decode to nothing; after a reset a slice without them does not decode.
    let parameters = H264Decoder.nalUnits(unit).filter { [7, 8].contains($0[0] & 0x1F) }
    #expect(decoder.decode(parameters.reduce(Data()) { $0 + Data([0, 0, 0, 1]) + $1 }) == nil)

    decoder.reset()
    let slices = H264Decoder.nalUnits(unit).filter { $0[0] & 0x1F == 5 }
    #expect(decoder.decode(slices.reduce(Data()) { $0 + Data([0, 0, 0, 1]) + $1 }) == nil)
}
