import AVFoundation
import AudioToolbox
import Foundation
import os
import PhoneLinkEngine

private let log = AppLog(category: "mirroring")

/// Plays one phone app's audio stream on the Mac's output: each frame is decoded (Opus with
/// AudioToolbox's decoder, PCM converted) to 32-bit float and queued on a player node. AudioToolbox's
/// Opus decoder decodes only packets of the length its format names, so there is one per packet
/// length the stream uses, read from each packet's table of contents. Frames are handled in order on
/// the player's own queue, so the engine's thread never waits for them. Each frame is queued as it
/// arrives; what waits to play is measured in samples, and past the maximum of `MirrorAudioQueue`
/// frames are dropped until the queue is back at its desired depth. Audio is not timed against the
/// app's video.
final class MirrorAudioPlayer: @unchecked Sendable {
    private enum Decoder {
        case opus
        case pcm(bytesPerSample: Int, isFloat: Bool)
    }

    private let queue: DispatchQueue
    private let engine = AVAudioEngine()
    private let player = AVAudioPlayerNode()
    private let output: AVAudioFormat
    private let decoder: Decoder
    private let sampleRate: Int
    private let channels: UInt32
    /// The Opus decoders by packet length in samples, with their input formats.
    private var opusDecoders: [Int: (AVAudioConverter, AVAudioFormat)] = [:]
    private var policy: MirrorAudioQueue
    /// Samples scheduled on the player and not yet consumed by its output.
    private var queued = 0
    /// A frame has been queued since the player started; an empty queue after that ran dry.
    private var everPlayed = false
    private var running = false
    private var stopped = false
    /// The output failed to start since it last ran (logged once: each frame tries again).
    private var startFailureLogged = false
    private var configurationObserver: NSObjectProtocol?

    /// A player for `format`; `nil` for a format this Mac does not decode (logged).
    init?(packageName: String, format: AudioStreamFormat) {
        guard format.channels > 0, format.sampleRate > 0,
              let output = AVAudioFormat(
                  commonFormat: .pcmFormatFloat32,
                  sampleRate: Double(format.sampleRate),
                  channels: AVAudioChannelCount(format.channels),
                  interleaved: false
              )
        else {
            log.error("mirrored audio of \(packageName, privacy: .private) not played; \(format.channels) channels at \(format.sampleRate) Hz is no output format")
            return nil
        }

        switch format.codec {
        case 0:
            guard Self.opusDecoder(samples: 960, sampleRate: Int(format.sampleRate), channels: format.channels, output: output) != nil else {
                log.error("mirrored audio of \(packageName, privacy: .private) not played; this Mac has no Opus decoder for it")
                return nil
            }
            decoder = .opus
        case 1:
            guard [2, 3, 4].contains(format.bytesPerSample), !format.isFloat || format.bytesPerSample == 4 else {
                log.error("mirrored audio of \(packageName, privacy: .private) not played; PCM of \(format.bytesPerSample) bytes per sample is not decoded")
                return nil
            }
            decoder = .pcm(bytesPerSample: Int(format.bytesPerSample), isFloat: format.isFloat)
        default:
            // AAC carries no decoder configuration in the stream's format; the phone advertises Opus
            // first, which the engine chooses.
            log.error("mirrored audio of \(packageName, privacy: .private) not played; codec \(format.codec) is not decoded")
            return nil
        }

        self.output = output
        channels = format.channels
        sampleRate = Int(format.sampleRate)
        policy = MirrorAudioQueue(frameLengthMs: Int(format.frameLengthMs), sampleRate: sampleRate)
        queue = DispatchQueue(label: "\(PhoneLinkIdentity.bundleID).mirror-audio")

        engine.attach(player)
        engine.connect(player, to: engine.mainMixerNode, format: output)
        configurationObserver = NotificationCenter.default.addObserver(
            forName: .AVAudioEngineConfigurationChange, object: engine, queue: nil
        ) { [weak self] _ in
            guard let self else { return }
            queue.async { self.restart() }
        }
    }

    deinit {
        if let configurationObserver {
            NotificationCenter.default.removeObserver(configurationObserver)
        }
    }

    /// Queues one encoded frame.
    func play(_ frame: Data) {
        queue.async { [self] in
            // Decoded even when dropped, so the decoder's state follows the stream.
            guard !stopped, let buffer = decode(frame) else { return }
            if !running {
                start()
            }

            let wasDraining = policy.draining
            let admitted = policy.admit(queued: queued)
            if policy.draining != wasDraining {
                if policy.draining {
                    log.notice("mirrored audio queue at \(self.queued * 1000 / self.sampleRate) ms; frames are dropped until it drains")
                } else {
                    log.notice("mirrored audio queue at \(self.queued * 1000 / self.sampleRate) ms; frames play again")
                }
            }

            guard admitted else { return }
            if queued == 0, everPlayed {
                log.debug("mirrored audio queue ran dry")
            }
            everPlayed = true
            schedule(buffer)
            if running, !player.isPlaying {
                player.play()
            }
        }
    }

    private func schedule(_ buffer: AVAudioPCMBuffer) {
        let frames = Int(buffer.frameLength)
        queued += frames
        player.scheduleBuffer(buffer, completionCallbackType: .dataConsumed) { [weak self] _ in
            guard let self else { return }
            queue.async { self.queued -= frames }
        }
    }

    /// Ends playback; the player plays nothing afterwards.
    func stop() {
        queue.async { [self] in
            stopped = true
            player.stop()
            engine.stop()
            running = false
        }
    }

    private func start() {
        do {
            engine.prepare()
            try engine.start()
            running = true
            startFailureLogged = false
        } catch {
            if !startFailureLogged {
                startFailureLogged = true
                log.error("mirrored audio output not started: \(error: error); retried with each frame, later failures are not logged")
            }
        }
    }

    /// The output device changed: the engine stopped, and starts again on the new device.
    private func restart() {
        guard !stopped else { return }
        running = false
        start()
        if running, queued > 0 {
            player.play()
        }
    }

    /// One frame as samples to play; `nil` for a frame that does not decode. Called on the queue (or
    /// by a test that owns the player).
    func decode(_ frame: Data) -> AVAudioPCMBuffer? {
        switch decoder {
        case .opus:
            return decodeOpus(frame)
        case let .pcm(bytes, isFloat):
            return Self.pcm(frame, bytesPerSample: bytes, isFloat: isFloat, format: output)
        }
    }

    /// A decoder of Opus packets of `samples` samples, with its input format.
    private static func opusDecoder(samples: Int, sampleRate: Int, channels: UInt32, output: AVAudioFormat) -> (AVAudioConverter, AVAudioFormat)? {
        var description = AudioStreamBasicDescription(
            mSampleRate: Double(sampleRate),
            mFormatID: kAudioFormatOpus,
            mFormatFlags: 0,
            mBytesPerPacket: 0,
            mFramesPerPacket: UInt32(samples),
            mBytesPerFrame: 0,
            mChannelsPerFrame: channels,
            mBitsPerChannel: 0,
            mReserved: 0
        )
        guard let input = AVAudioFormat(streamDescription: &description),
              let converter = AVAudioConverter(from: input, to: output)
        else { return nil }
        return (converter, input)
    }

    private func decodeOpus(_ frame: Data) -> AVAudioPCMBuffer? {
        let bytes = [UInt8](frame)
        guard let samples = OpusPacket.sampleCount(bytes, sampleRate: sampleRate),
              let pcm = AVAudioPCMBuffer(pcmFormat: output, frameCapacity: AVAudioFrameCount(samples))
        else {
            log.debug("opus frame dropped; it is no packet")
            return nil
        }

        let decoder = opusDecoders[samples] ?? Self.opusDecoder(samples: samples, sampleRate: sampleRate, channels: channels, output: output)
        guard let (converter, input) = decoder else {
            log.debug("opus frame dropped; no decoder for packets of \(samples) samples")
            return nil
        }
        opusDecoders[samples] = decoder

        let packet = AVAudioCompressedBuffer(format: input, packetCapacity: 1, maximumPacketSize: bytes.count)
        bytes.withUnsafeBytes { packet.data.copyMemory(from: $0.baseAddress!, byteCount: bytes.count) }
        packet.packetDescriptions?.pointee = AudioStreamPacketDescription(
            mStartOffset: 0, mVariableFramesInPacket: 0, mDataByteSize: UInt32(bytes.count)
        )
        packet.packetCount = 1
        packet.byteLength = UInt32(bytes.count)

        final class Once: @unchecked Sendable { var given = false }
        let once = Once()
        var error: NSError?
        let status = converter.convert(to: pcm, error: &error) { _, status in
            if once.given {
                status.pointee = .noDataNow
                return nil
            }
            once.given = true
            status.pointee = .haveData
            return packet
        }

        guard status != .error, pcm.frameLength > 0 else {
            if let error {
                log.debug("opus frame not decoded: \(error: error)")
            } else {
                log.debug("opus frame not decoded; it gave no samples")
            }
            return nil
        }
        return pcm
    }

    /// Interleaved PCM samples of `bytesPerSample` bytes each (2, 3 or 4) as a buffer of `format`
    /// (deinterleaved 32-bit float).
    static func pcm(_ frame: Data, bytesPerSample: Int, isFloat: Bool, format: AVAudioFormat) -> AVAudioPCMBuffer? {
        let channels = Int(format.channelCount)
        let frames = frame.count / (bytesPerSample * channels)
        guard frames > 0,
              let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(frames)),
              let out = buffer.floatChannelData
        else { return nil }

        let bytes = [UInt8](frame)
        for index in 0 ..< frames * channels {
            let offset = index * bytesPerSample
            let value: Float
            switch (bytesPerSample, isFloat) {
            case (4, true):
                let bits = UInt32(bytes[offset]) | UInt32(bytes[offset + 1]) << 8 | UInt32(bytes[offset + 2]) << 16 | UInt32(bytes[offset + 3]) << 24
                value = Float(bitPattern: bits)
            case (4, false):
                let bits = UInt32(bytes[offset]) | UInt32(bytes[offset + 1]) << 8 | UInt32(bytes[offset + 2]) << 16 | UInt32(bytes[offset + 3]) << 24
                value = Float(Int32(bitPattern: bits)) / 2_147_483_648
            case (3, _):
                let bits = UInt32(bytes[offset]) << 8 | UInt32(bytes[offset + 1]) << 16 | UInt32(bytes[offset + 2]) << 24
                value = Float(Int32(bitPattern: bits)) / 2_147_483_648
            default:
                let bits = UInt16(bytes[offset]) | UInt16(bytes[offset + 1]) << 8
                value = Float(Int16(bitPattern: bits)) / 32768
            }
            out[index % channels][index / channels] = value
        }

        buffer.frameLength = AVAudioFrameCount(frames)
        return buffer
    }
}

/// How much decoded audio waits to play: bounds from the stream's frame length (100 and 170 ms for
/// the phone's 40). Frames are queued as they arrive; past the maximum they are dropped until the
/// queue is back at the desired depth. No silence is added before a frame.
struct MirrorAudioQueue {
    let desired: Int
    let maximum: Int
    private(set) var draining = false

    /// Bounds in samples at `sampleRate`.
    init(frameLengthMs: Int, sampleRate: Int) {
        let samples = { (ms: Int) in ms * sampleRate / 1000 }
        desired = samples(max(2 * frameLengthMs, 100))
        maximum = samples(max(4 * frameLengthMs, 170))
    }

    /// Whether to queue one decoded frame that finds `queued` samples waiting.
    mutating func admit(queued: Int) -> Bool {
        if queued > maximum {
            draining = true
        } else if queued <= desired {
            draining = false
        }
        return !draining
    }
}
