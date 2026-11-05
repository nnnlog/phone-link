import AVFoundation
import CoreGraphics
import CoreVideo
import Foundation
import PhoneLinkEngine
import Testing
@testable import PhoneLinkKit

struct MirrorDisplayTests {
    @Test func aPortraitScreenFitsTheWindowCentred() throws {
        let display = MirrorDisplay(deviceSize: CGSize(width: 1080, height: 2400), videoSize: CGSize(width: 720, height: 1600))
        #expect(display.screenSize == CGSize(width: 720, height: 1600), "the video's size")

        let rect = try #require(display.screenRect(in: CGSize(width: 900, height: 1200)))
        #expect(rect.height == 1200)
        #expect(rect.width == 540)
        #expect(rect.minX == 180 && rect.minY == 0)

        let centre = try #require(display.normalized(CGPoint(x: 450, y: 600), in: CGSize(width: 900, height: 1200)))
        #expect(centre == CGPoint(x: 0.5, y: 0.5))
        let corner = try #require(display.normalized(CGPoint(x: 180, y: 0), in: CGSize(width: 900, height: 1200)))
        #expect(corner == .zero)
        let outside = try #require(display.normalized(CGPoint(x: 10, y: 600), in: CGSize(width: 900, height: 1200)))
        #expect(outside.x < 0, "outside the video is outside 0–1")
    }

    /// A video area of another aspect letterboxes the video: scaled the same both ways, centred.
    @Test func theVideoIsLetterboxedNotStretched() throws {
        let display = MirrorDisplay(videoSize: CGSize(width: 1000, height: 500))
        let rect = try #require(display.screenRect(in: CGSize(width: 400, height: 400)))
        #expect(rect == CGRect(x: 0, y: 100, width: 400, height: 200))
        let point = try #require(display.normalized(CGPoint(x: 200, y: 150), in: CGSize(width: 400, height: 400)))
        #expect(point == CGPoint(x: 0.5, y: 0.25))
    }

    @Test func aTurnedPhoneSwapsTheVideosSides() {
        let display = MirrorDisplay(videoSize: CGSize(width: 720, height: 1600), rotation: -90)
        #expect(display.swapsSides)
        #expect(display.screenSize == CGSize(width: 1600, height: 720), "the rotated video")
        #expect(abs(display.layerRotation - .pi / 2) < 1e-9, "−90° turns counter-clockwise in a y-up layer")

        #expect(!MirrorDisplay(rotation: -180).swapsSides)
        #expect(MirrorDisplay(rotation: -270).swapsSides)
    }

    /// The video sets the drawn screen; before it is known nothing is drawn, a new window opens with the
    /// phone record's resolution, and the navigation bar's room is measured against the current one.
    @Test func theVideoSetsTheScreen() {
        let display = MirrorDisplay(deviceSize: CGSize(width: 1080, height: 2400), videoSize: CGSize(width: 1080, height: 2404))
        #expect(display.screenSize == CGSize(width: 1080, height: 2404))

        let started = MirrorDisplay(deviceSize: CGSize(width: 2400, height: 1080), initialDeviceSize: CGSize(width: 1080, height: 2400))
        #expect(started.screenSize == nil)
        #expect(started.openingSize == CGSize(width: 1080, height: 2400))
        #expect(started.shownSize == CGSize(width: 2400, height: 1080))
        #expect(MirrorDisplay().openingSize == nil)

        let turned = MirrorDisplay(deviceSize: CGSize(width: 2400, height: 1080), videoSize: CGSize(width: 540, height: 1200), rotation: -90)
        #expect(turned.openingSize == CGSize(width: 1200, height: 540))
        #expect(turned.shownSize == CGSize(width: 1200, height: 540))
    }

    @Test func nothingIsLaidOutWithoutASize() {
        #expect(MirrorDisplay().screenRect(in: CGSize(width: 100, height: 100)) == nil)
        #expect(MirrorDisplay(initialDeviceSize: CGSize(width: 10, height: 10)).screenRect(in: CGSize(width: 100, height: 100)) == nil)
        #expect(MirrorDisplay(videoSize: CGSize(width: 10, height: 10)).screenRect(in: .zero) == nil)
    }

    /// The phone's content area (a folded phone's half) is not used: the whole video shows.
    @Test func aSessionsValuesBecomeTheDisplay() throws {
        var view = mirrorSession("s")
        view.rotation = -90
        view.contentArea = MirrorRect(left: 0, top: 10, right: 1080, bottom: 1210)
        let display = MirrorDisplay(view)

        #expect(display.deviceSize == CGSize(width: 1080, height: 2400))
        #expect(display.initialDeviceSize == CGSize(width: 1440, height: 3120))
        #expect(display.videoSize == CGSize(width: 720, height: 1600))
        #expect(display.rotation == -90)
        #expect(display.transform == MirrorDisplay.Transform(videoSize: CGSize(width: 720, height: 1600), rotation: -90))
        let rect = try #require(display.screenRect(in: CGSize(width: 1600, height: 720)))
        #expect(rect == CGRect(x: 0, y: 0, width: 1600, height: 720), "the whole turned video")

        view.deviceResolution = MirrorSize(width: 0, height: 0)
        #expect(MirrorDisplay(view).deviceSize == nil)
    }

    /// The first transform shows at once; a different one after a fade; the same one again at once.
    /// A transform is compared with the last one received.
    @Test func aNewTransformFades() {
        var transitions = MirrorVideoTransitions()
        let portrait = MirrorDisplay.Transform(videoSize: CGSize(width: 720, height: 1600), rotation: 0)
        let turned = MirrorDisplay.Transform(videoSize: CGSize(width: 720, height: 1600), rotation: -90)
        let resized = MirrorDisplay.Transform(videoSize: CGSize(width: 1600, height: 720), rotation: -90)

        #expect(transitions.receive(nil) == .apply, "nothing to draw yet")
        #expect(transitions.receive(portrait) == .apply, "the first")
        #expect(transitions.receive(portrait) == .apply)
        #expect(transitions.receive(turned) == .fade)
        #expect(transitions.receive(resized) == .fade)
        #expect(transitions.receive(nil) == .apply)
        #expect(transitions.receive(resized) == .apply, "unchanged")
        #expect(transitions.receive(portrait) == .fade)
    }
}

struct MirrorScrollTests {
    @Test func aWheelLineIsANotchOf120() {
        var scroll = MirrorScrollAccumulator()
        #expect(scroll.add(deltaX: 0, deltaY: 1, precise: false, pointsPerLine: 10).map(\.y) == 120)
        #expect(scroll.add(deltaX: 0, deltaY: -2, precise: false, pointsPerLine: 10).map(\.y) == -240, "one scroll per event")
        // Content moving right (AppKit's positive x) scrolls left on the phone.
        #expect(scroll.add(deltaX: 1, deltaY: 0, precise: false, pointsPerLine: 10).map(\.x) == -120)
        let both = scroll.add(deltaX: -0.5, deltaY: 0.25, precise: false, pointsPerLine: 10)
        #expect(both?.x == 60 && both?.y == 30, "both axes in one scroll")
    }

    @Test func preciseScrollingIsPointsPerLine() {
        var scroll = MirrorScrollAccumulator()
        #expect(scroll.add(deltaX: 0, deltaY: 10, precise: true, pointsPerLine: 10).map(\.y) == 120, "one line")
        #expect(scroll.add(deltaX: 0, deltaY: 1, precise: true, pointsPerLine: 10).map(\.y) == 12, "a point is a tenth")
        #expect(scroll.add(deltaX: 0, deltaY: 0.05, precise: true, pointsPerLine: 10) == nil, "less than a unit")
        #expect(scroll.add(deltaX: 0, deltaY: 0.05, precise: true, pointsPerLine: 10).map(\.y) == 1, "carried over")

        _ = scroll.add(deltaX: 0, deltaY: 0.05, precise: true, pointsPerLine: 10)
        scroll.reset()
        #expect(scroll.add(deltaX: 0, deltaY: 0.05, precise: true, pointsPerLine: 10) == nil, "a reset forgets the rest")
    }
}

struct MirrorModifierKeysTests {
    @Test func modifierKeysGoDownAndUpByTheirOwnFlag() {
        // Left Shift down (device bit 0x2 and Shift).
        #expect(MirrorModifierKeys.change(keyCode: 0x38, flags: 1 << 17 | 0x2) == .down)
        // Right Shift released while the left one stays down.
        #expect(MirrorModifierKeys.change(keyCode: 0x3C, flags: 1 << 17 | 0x2) == .up)
        #expect(MirrorModifierKeys.change(keyCode: 0x3A, flags: 1 << 19 | 0x20) == .down)
        #expect(MirrorModifierKeys.change(keyCode: 0x3A, flags: 0) == .up)

        // Without device bits, the device-independent flag decides.
        #expect(MirrorModifierKeys.change(keyCode: 0x3B, flags: 1 << 18) == .down)
        #expect(MirrorModifierKeys.change(keyCode: 0x3F, flags: 1 << 23) == .down)
    }

    @Test func capsLockIsAPressAndReleaseOnEachToggle() {
        #expect(MirrorModifierKeys.change(keyCode: 0x39, flags: 1 << 16) == .toggle)
        #expect(MirrorModifierKeys.change(keyCode: 0x39, flags: 0) == .toggle)
        #expect(MirrorModifierKeys.change(keyCode: 0x00, flags: 0) == nil, "A is no modifier key")
    }
}

struct OpusPacketTests {
    @Test func theTocGivesThePacketsLength() {
        // CELT 20 ms (config 31), one frame.
        #expect(OpusPacket.sampleCount([31 << 3], sampleRate: 48000) == 960)
        // SILK 40 ms (config 2), two frames (code 1): 80 ms.
        #expect(OpusPacket.sampleCount([2 << 3 | 1], sampleRate: 48000) == 3840)
        // Hybrid 10 ms (config 12), code 3 with 3 frames.
        #expect(OpusPacket.sampleCount([12 << 3 | 3, 3], sampleRate: 48000) == 1440)
        // CELT 2.5 ms (config 16).
        #expect(OpusPacket.sampleCount([16 << 3], sampleRate: 48000) == 120)
    }

    @Test func malformedPacketsHaveNoLength() {
        #expect(OpusPacket.sampleCount([], sampleRate: 48000) == nil)
        #expect(OpusPacket.sampleCount([3], sampleRate: 48000) == nil, "code 3 without its count byte")
        #expect(OpusPacket.sampleCount([3, 0], sampleRate: 48000) == nil, "no frames")
        #expect(OpusPacket.sampleCount([31 << 3 | 3, 7], sampleRate: 48000) == nil, "longer than 120 ms")
    }

    @Test func opusPacketsOfEveryLengthDecode() throws {
        let format = AudioStreamFormat(channels: 2, sampleRate: 48000, codec: 0, bytesPerSample: 0, isFloat: false, frameLengthMs: 40)
        let player = try #require(MirrorAudioPlayer(packageName: "com.app", format: format))
        let pcm = try #require(AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 48000, channels: 2, interleaved: false))

        // 20 ms and 40 ms packets, as a phone may mix them; the first of each length primes its decoder.
        var decoded: [AVAudioFrameCount] = []
        for samples in [960, 960, 1920, 1920, 960] as [UInt32] {
            let packet = try opusPacket(samples: samples, format: pcm)
            #expect(OpusPacket.sampleCount([UInt8](packet), sampleRate: 48000) == Int(samples))
            decoded.append(player.decode(packet)?.frameLength ?? 0)
        }

        #expect(decoded[1] == 960 && decoded[3] == 1920 && decoded[4] == 960)
        #expect(player.decode(Data()) == nil)
    }

    /// One Opus packet of `samples` samples of a tone, encoded by AudioToolbox.
    private func opusPacket(samples: UInt32, format: AVAudioFormat) throws -> Data {
        var description = AudioStreamBasicDescription(
            mSampleRate: 48000, mFormatID: kAudioFormatOpus, mFormatFlags: 0, mBytesPerPacket: 0,
            mFramesPerPacket: samples, mBytesPerFrame: 0, mChannelsPerFrame: 2, mBitsPerChannel: 0, mReserved: 0
        )
        let opus = try #require(AVAudioFormat(streamDescription: &description))
        let encoder = try #require(AVAudioConverter(from: format, to: opus))

        let input = try #require(AVAudioPCMBuffer(pcmFormat: format, frameCapacity: samples))
        input.frameLength = samples
        for channel in 0 ..< 2 {
            for i in 0 ..< Int(samples) {
                input.floatChannelData![channel][i] = sin(Float(i) * 0.05) * 0.5
            }
        }

        let output = AVAudioCompressedBuffer(format: opus, packetCapacity: 1, maximumPacketSize: encoder.maximumOutputPacketSize)
        final class Once: @unchecked Sendable { var given = false }
        let once = Once()
        var error: NSError?

        _ = encoder.convert(to: output, error: &error) { _, status in
            if once.given {
                status.pointee = .noDataNow
                return nil
            }

            once.given = true
            status.pointee = .haveData
            return input
        }

        #expect(output.packetCount == 1)
        return Data(bytes: output.data, count: Int(output.byteLength))
    }

    @Test func thePhonesTenMillisecondPacketsDecode() throws {
        // The phone's encoder output: CELT-only (RESTRICTED_LOWDELAY) 10 ms stereo packets of a 1 kHz tone,
        // made with libopus 48 kHz, 480 samples per opus_encode_float.
        let packets = [
            "f49fdf547efa9e039ece29e8a185f76e101f1c1561af9d1ebf16fcbf96490ed230553774a30e88c56dd4b0fd2e098b51ce075cf3c9653f7202960a68d36cd7326fa75a16e236b9df127f17d18140115c7f8aa560000000003c00f0cff0f8a70a4ca34442d2aa88b7b2f5dffed10f4eeb2470acee93fabe1dad0ad9bab6276a5bf8062e02549c4c192fa2763b149c2552ddefea182f97c3d760611b43f4800164aaf05f0fed5faab1",
            "f49e7119ff640bfae508306d97a068ed9b31713e839a8e4a893dd697774ab3e97950d786c19a2aa702f596c9ecc0be7db6d67eaaac8a032e9792c8b6b7dde940767d88ee2d6b7aebbd0f847f0446013f6fce035158cd95fe38a8933746e452c965342e76817b37970d75a3b8591da6e6a5b86d32705e48d3a1c043993c42402aa96bf0f26226511bfff1",
        ].map { hex in
            Data(stride(from: 0, to: hex.count, by: 2).map { i in
                UInt8(hex[hex.index(hex.startIndex, offsetBy: i) ..< hex.index(hex.startIndex, offsetBy: i + 2)], radix: 16)!
            })
        }

        let format = AudioStreamFormat(channels: 2, sampleRate: 48000, codec: 0, bytesPerSample: 0, isFloat: false, frameLengthMs: 40)
        let player = try #require(MirrorAudioPlayer(packageName: "com.app", format: format))

        #expect(OpusPacket.sampleCount([UInt8](packets[0]), sampleRate: 48000) == 480)
        _ = player.decode(packets[0])
        let second = try #require(player.decode(packets[1]))
        #expect(second.frameLength == 480)

        let peak = (0 ..< 480).map { abs(second.floatChannelData![0][$0]) }.max() ?? 0
        #expect(peak > 0.4 && peak < 0.6, "the 0.5 tone")
    }

    @Test func pcmBecomesFloatChannels() throws {
        let format = try #require(AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 48000, channels: 2, interleaved: false))
        // Two 16-bit stereo frames: (16384, -16384), (32767, 0).
        let bytes: [UInt8] = [0x00, 0x40, 0x00, 0xC0, 0xFF, 0x7F, 0x00, 0x00]
        let buffer = try #require(MirrorAudioPlayer.pcm(Data(bytes), bytesPerSample: 2, isFloat: false, format: format))

        #expect(buffer.frameLength == 2)
        let channels = try #require(buffer.floatChannelData)
        #expect(channels[0][0] == 0.5 && channels[1][0] == -0.5)
        #expect(abs(channels[0][1] - 1) < 0.001 && channels[1][1] == 0)
    }
}

struct MirrorAudioQueueTests {
    /// 1 ms at 48 kHz.
    private let ms = 48

    @Test func boundsFollowTheFrameLength() {
        let phone = MirrorAudioQueue(frameLengthMs: 40, sampleRate: 48000)
        #expect([phone.desired, phone.maximum] as [Int] == [100 * ms, 170 * ms])
        let long = MirrorAudioQueue(frameLengthMs: 60, sampleRate: 48000)
        #expect([long.desired, long.maximum] as [Int] == [120 * ms, 240 * ms])
    }

    @Test func framesPlayAsTheyArrive() {
        var queue = MirrorAudioQueue(frameLengthMs: 40, sampleRate: 48000)
        let a1 = queue.admit(queued: 0)
        #expect(a1, "nothing waits in front of the first frame")
        let a2 = queue.admit(queued: 150 * ms)
        #expect(a2, "between desired and maximum frames play")
        let a3 = queue.admit(queued: 170 * ms)
        #expect(a3)
    }

    @Test func anOverfullQueueDrainsToTheDesiredDepth() {
        var queue = MirrorAudioQueue(frameLengthMs: 40, sampleRate: 48000)
        let a4 = queue.admit(queued: 171 * ms)
        #expect(!a4, "past the maximum frames are dropped")
        let a5 = queue.admit(queued: 150 * ms)
        #expect(!a5, "and keep being dropped above the desired depth")
        let a6 = queue.admit(queued: 100 * ms)
        #expect(a6, "until the queue is back at it")
        let a7 = queue.admit(queued: 150 * ms)
        #expect(a7)
    }
}

/// A sink that shows what it is given.
private final class RecordingSink: MirrorVideoSink, @unchecked Sendable {
    var pictures = 0
    var shows = true

    func present(_ picture: CVPixelBuffer) -> Bool {
        pictures += 1
        return shows
    }
}

struct MirrorPresenterAdapterTests {
    @Test func aFrameIsPresentedOnlyThroughItsWindowsSink() throws {
        let adapter = MirrorPresenterAdapter()
        let unit = try encodedKeyFrame()
        #expect(!adapter.videoFrame(sessionId: "s", data: unit, keyframe: true), "no window yet")

        let sink = RecordingSink()
        adapter.attach(sink, to: "s")
        #expect(adapter.videoFrame(sessionId: "s", data: unit, keyframe: true))
        #expect(sink.pictures == 1)
        #expect(!adapter.videoFrame(sessionId: "other", data: unit, keyframe: true), "another session has no window")

        sink.shows = false
        #expect(!adapter.videoFrame(sessionId: "s", data: unit, keyframe: true), "a picture the window did not show")
        sink.shows = true

        // After a reset the stream needs its parameters again: slices alone decode to nothing.
        adapter.resetVideo(sessionId: "s")
        let slices = H264Decoder.nalUnits(unit).filter { $0[0] & 0x1F == 5 }
        #expect(!adapter.videoFrame(sessionId: "s", data: slices.reduce(Data()) { $0 + Data([0, 0, 0, 1]) + $1 }, keyframe: true))
        #expect(adapter.videoFrame(sessionId: "s", data: unit, keyframe: true))

        adapter.detach("s")
        #expect(!adapter.videoFrame(sessionId: "s", data: unit, keyframe: true), "the window closed")
        #expect(sink.pictures == 3)
    }

    @Test func garbageDecodesToNothing() {
        let adapter = MirrorPresenterAdapter()
        adapter.attach(RecordingSink(), to: "s")
        #expect(!adapter.videoFrame(sessionId: "s", data: Data([0, 0, 1, 0x65, 1, 2, 3]), keyframe: true))
        #expect(!adapter.videoFrame(sessionId: "s", data: Data(), keyframe: false))
    }
}
