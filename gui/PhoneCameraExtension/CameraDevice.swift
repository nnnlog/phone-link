import CoreMediaIO
import CoreVideo
import Foundation
import IOKit.audio

/// One phone's camera: a source stream for the apps that use the camera and a sink stream the app
/// feeds. The source sends the latest fed frame 30 times a second (black before any), so a still image
/// the app fed once keeps showing.
final class CameraDeviceSource: NSObject, CMIOExtensionDeviceSource {
    private(set) var device: CMIOExtensionDevice!
    private var source: CameraStreamSource!
    private var sink: CameraStreamSource!
    private let format: CMFormatDescription
    private let queue = DispatchQueue(label: "\(PhoneCameraShared.extensionBundleID).device", qos: .userInteractive)
    private let clientsProperty = CMIOExtensionProperty(
        rawValue: PhoneCameraShared.propertyName(PhoneCameraShared.clientsSelector)
    )
    // State below is used on `queue` only.
    private var clients = 0
    private var timer: DispatchSourceTimer?
    private var latest: CVPixelBuffer?
    private let black: CVPixelBuffer
    private var sinkClient: CMIOExtensionClient?
    /// A frame was not sent (logged once per camera, since every frame would repeat it).
    private var sendFailureLogged = false

    init(uid: UUID, name: String) throws {
        var description: CMFormatDescription?
        let status = CMVideoFormatDescriptionCreate(
            allocator: kCFAllocatorDefault,
            codecType: PhoneCameraShared.pixelFormat,
            width: PhoneCameraShared.width,
            height: PhoneCameraShared.height,
            extensions: nil,
            formatDescriptionOut: &description
        )
        guard status == noErr, let description else { throw CameraExtensionError.format(status) }

        format = description
        black = try Self.blackFrame()
        super.init()

        device = CMIOExtensionDevice(localizedName: name, deviceID: uid, legacyDeviceID: uid.uuidString, source: self)
        let duration = CMTime(value: 1, timescale: PhoneCameraShared.framesPerSecond)
        let streamFormat = CMIOExtensionStreamFormat(
            formatDescription: description,
            maxFrameDuration: duration,
            minFrameDuration: duration,
            validFrameDurations: nil
        )

        source = CameraStreamSource(name: "\(name) Video", direction: .source, format: streamFormat, device: self)
        sink = CameraStreamSource(name: "\(name) Feed", direction: .sink, format: streamFormat, device: self)
        try device.addStream(source.stream)
        try device.addStream(sink.stream)
    }

    var availableProperties: Set<CMIOExtensionProperty> {
        [.deviceTransportType, .deviceModel, clientsProperty]
    }

    func deviceProperties(forProperties properties: Set<CMIOExtensionProperty>) throws -> CMIOExtensionDeviceProperties {
        let result = CMIOExtensionDeviceProperties(dictionary: [:])
        if properties.contains(.deviceTransportType) {
            result.transportType = kIOAudioDeviceTransportTypeVirtual
        }
        if properties.contains(.deviceModel) {
            result.model = "Phone Link Camera"
        }
        if properties.contains(clientsProperty) {
            let streaming = queue.sync { clients > 0 }
            result.setPropertyState(clientsState(streaming), forProperty: clientsProperty)
        }
        return result
    }

    func setDeviceProperties(_ deviceProperties: CMIOExtensionDeviceProperties) throws {}

    private func clientsState(_ streaming: Bool) -> CMIOExtensionPropertyState<AnyObject> {
        CMIOExtensionPropertyState(value: (streaming ? "1" : "0") as NSString)
    }

    // MARK: - The source: apps using the camera

    func startSource() {
        queue.async { [self] in
            clients += 1
            guard clients == 1 else { return }

            let timer = DispatchSource.makeTimerSource(flags: .strict, queue: queue)
            timer.schedule(deadline: .now(), repeating: 1.0 / Double(PhoneCameraShared.framesPerSecond), leeway: .milliseconds(2))
            timer.setEventHandler { [weak self] in self?.sendFrame() }
            timer.resume()
            self.timer = timer
            device.notifyPropertiesChanged([clientsProperty: clientsState(true)])
        }
    }

    func stopSource() {
        queue.async { [self] in
            guard clients > 0 else { return }
            clients -= 1
            guard clients == 0 else { return }

            timer?.cancel()
            timer = nil
            device.notifyPropertiesChanged([clientsProperty: clientsState(false)])
        }
    }

    private func sendFrame() {
        let frame = latest ?? black
        var timing = CMSampleTimingInfo(
            duration: CMTime(value: 1, timescale: PhoneCameraShared.framesPerSecond),
            presentationTimeStamp: CMClockGetTime(CMClockGetHostTimeClock()),
            decodeTimeStamp: .invalid
        )

        var sample: CMSampleBuffer?
        let status = CMSampleBufferCreateReadyWithImageBuffer(
            allocator: kCFAllocatorDefault,
            imageBuffer: frame,
            formatDescription: format,
            sampleTiming: &timing,
            sampleBufferOut: &sample
        )

        guard status == noErr, let sample else {
            if !sendFailureLogged {
                sendFailureLogged = true
                log.error("camera frame not sent: status \(status); later failures are not logged")
            }
            return
        }

        let nanos = UInt64(timing.presentationTimeStamp.seconds * Double(NSEC_PER_SEC))
        source.stream.send(sample, discontinuity: [], hostTimeInNanoseconds: nanos)
    }

    // MARK: - The sink: the app's feed

    func startSink(client: CMIOExtensionClient) {
        queue.async { [self] in
            sinkClient = client
            consume(client)
        }
    }

    func stopSink() {
        queue.async { [self] in sinkClient = nil }
    }

    /// Takes the app's next frame, then asks for the one after while the sink runs.
    private func consume(_ client: CMIOExtensionClient) {
        sink.stream.consumeSampleBuffer(from: client) { [weak self] sample, sequence, _, _, error in
            guard let self else { return }
            self.queue.async {
                if let sample, let image = CMSampleBufferGetImageBuffer(sample) {
                    self.latest = image
                    let now = CMClockGetTime(CMClockGetHostTimeClock())
                    let output = CMIOExtensionScheduledOutput(
                        sequenceNumber: sequence,
                        hostTimeInNanoseconds: UInt64(now.seconds * Double(NSEC_PER_SEC))
                    )
                    self.sink.stream.notifyScheduledOutputChanged(output)
                } else if let error {
                    log.debug("feed frame not taken: \(String(describing: error), privacy: .public)")
                }

                if self.sinkClient === client {
                    self.consume(client)
                }
            }
        }
    }

    /// The camera is being removed.
    func stop() {
        queue.sync {
            timer?.cancel()
            timer = nil
            sinkClient = nil
            latest = nil
        }
    }

    private static func blackFrame() throws -> CVPixelBuffer {
        var buffer: CVPixelBuffer?
        let attributes: [CFString: Any] = [kCVPixelBufferIOSurfacePropertiesKey: [:] as CFDictionary]
        let status = CVPixelBufferCreate(
            kCFAllocatorDefault,
            Int(PhoneCameraShared.width),
            Int(PhoneCameraShared.height),
            PhoneCameraShared.pixelFormat,
            attributes as CFDictionary,
            &buffer
        )
        guard status == kCVReturnSuccess, let buffer else { throw CameraExtensionError.buffer(status) }

        CVPixelBufferLockBaseAddress(buffer, [])
        defer { CVPixelBufferUnlockBaseAddress(buffer, []) }

        // Video-range black: luma 16, chroma 128.
        for (plane, value) in [(0, UInt8(16)), (1, UInt8(128))] {
            if let base = CVPixelBufferGetBaseAddressOfPlane(buffer, plane) {
                memset(base, Int32(value), CVPixelBufferGetBytesPerRowOfPlane(buffer, plane) * CVPixelBufferGetHeightOfPlane(buffer, plane))
            }
        }
        return buffer
    }
}

/// A camera's stream: the source apps read or the sink the app feeds.
final class CameraStreamSource: NSObject, CMIOExtensionStreamSource {
    private(set) var stream: CMIOExtensionStream!
    private let format: CMIOExtensionStreamFormat
    private unowned let device: CameraDeviceSource
    private let direction: CMIOExtensionStream.Direction

    init(name: String, direction: CMIOExtensionStream.Direction, format: CMIOExtensionStreamFormat, device: CameraDeviceSource) {
        self.format = format
        self.device = device
        self.direction = direction
        super.init()
        stream = CMIOExtensionStream(localizedName: name, streamID: UUID(), direction: direction, clockType: .hostTime, source: self)
    }

    var formats: [CMIOExtensionStreamFormat] { [format] }

    var availableProperties: Set<CMIOExtensionProperty> {
        var properties: Set<CMIOExtensionProperty> = [.streamActiveFormatIndex, .streamFrameDuration]
        if direction == .sink {
            properties.formUnion([.streamSinkBufferQueueSize, .streamSinkBuffersRequiredForStartup, .streamSinkBufferUnderrunCount, .streamSinkEndOfData])
        }
        return properties
    }

    func streamProperties(forProperties properties: Set<CMIOExtensionProperty>) throws -> CMIOExtensionStreamProperties {
        let result = CMIOExtensionStreamProperties(dictionary: [:])
        if properties.contains(.streamActiveFormatIndex) {
            result.activeFormatIndex = 0
        }
        if properties.contains(.streamFrameDuration) {
            result.frameDuration = CMTime(value: 1, timescale: PhoneCameraShared.framesPerSecond)
        }
        if properties.contains(.streamSinkBufferQueueSize) {
            result.sinkBufferQueueSize = 1
        }
        if properties.contains(.streamSinkBuffersRequiredForStartup) {
            result.sinkBuffersRequiredForStartup = 1
        }
        return result
    }

    func setStreamProperties(_ streamProperties: CMIOExtensionStreamProperties) throws {}

    private var sinkClient: CMIOExtensionClient?

    func authorizedToStartStream(for client: CMIOExtensionClient) -> Bool {
        if direction == .sink {
            sinkClient = client
        }
        return true
    }

    func startStream() throws {
        switch direction {
        case .source:
            device.startSource()
        case .sink:
            guard let sinkClient else { throw CameraExtensionError.noSinkClient }
            device.startSink(client: sinkClient)
        @unknown default:
            throw CameraExtensionError.noSinkClient
        }
    }

    func stopStream() throws {
        switch direction {
        case .source:
            device.stopSource()
        case .sink:
            device.stopSink()
        @unknown default:
            break
        }
    }
}

enum CameraExtensionError: Error {
    case format(OSStatus)
    case buffer(CVReturn)
    case noSinkClient
}
