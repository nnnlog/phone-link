import CoreMediaIO
import Foundation
import os
import PhoneLinkEngine

private let log = AppLog(category: "camera")

/// The phone's camera on this Mac, through the app's camera extension: publishing a camera asks the
/// extension for it and feeds it once it appears; still images and decoded video go to its feed; its
/// clients starting and stopping are reported to `onClientsChanged`. The engine calls this on its own
/// threads.
public final class PhoneCameraAdapter: PhoneCamera, @unchecked Sendable {
    /// Called with a camera's device id when its clients start (`true`) or stop streaming.
    public var onClientsChanged: (@Sendable (String, Bool) -> Void)?

    private let lock = NSLock()
    private var extensionActive = false
    private var outputs: [String: CameraOutput] = [:]

    public init() {}

    /// The camera extension was activated, or is no longer.
    public func setExtensionActive(_ active: Bool) {
        lock.withLock { extensionActive = active }
    }

    public func isAvailable() -> Bool {
        lock.withLock { extensionActive } && CameraExtensionLink.provider() != nil
    }

    public func publish(deviceId: String, name: String) throws {
        guard lock.withLock({ extensionActive }), let provider = CameraExtensionLink.provider() else {
            throw EdgeError.Unexpected(message: "the camera extension is not running")
        }

        let output: CameraOutput
        do {
            output = try CameraOutput(deviceID: deviceId, name: name) { [weak self] streaming in
                self?.onClientsChanged?(deviceId, streaming)
            }
        } catch {
            throw EdgeError.Unexpected(message: error.localizedDescription)
        }

        let (replaced, cameras) = lock.withLock {
            let replaced = outputs.updateValue(output, forKey: deviceId)
            return (replaced, outputs.values.map(\.camera))
        }
        replaced?.close()

        do {
            try CameraExtensionLink.publish(cameras, provider: provider)
        } catch {
            lock.withLock { outputs[deviceId] = nil }
            output.close()
            throw EdgeError.Unexpected(message: error.localizedDescription)
        }

        output.connect()
    }

    public func remove(deviceId: String) {
        let (removed, cameras) = lock.withLock { (outputs.removeValue(forKey: deviceId), outputs.values.map(\.camera)) }
        guard let removed else { return }
        removed.close()

        guard let provider = CameraExtensionLink.provider() else { return }
        do {
            try CameraExtensionLink.publish(cameras, provider: provider)
        } catch {
            log.error("camera not removed from the extension: \(error: error)")
        }
    }

    public func showPlaceholder(deviceId: String, placeholder: CameraPlaceholder) {
        output(deviceId)?.show(placeholder)
    }

    public func videoFrame(deviceId: String, data: Data, show: Bool) -> Bool {
        output(deviceId)?.video(data, show: show) ?? false
    }

    public func resetDecoder(deviceId: String) {
        output(deviceId)?.resetDecoder()
    }

    private func output(_ deviceId: String) -> CameraOutput? {
        lock.withLock { outputs[deviceId] }
    }
}

/// One published camera: its feed, its decoder, the image it shows.
final class CameraOutput: @unchecked Sendable {
    let camera: PhoneCameraShared.Camera
    private let onClients: @Sendable (Bool) -> Void
    /// The feed, the still images and the animation run here.
    private let queue: DispatchQueue
    private let frames: CameraFrames
    private let decodeLock = NSLock()
    private let decoder = H264Decoder()
    // State below is used on `queue` only.
    private var feed: SinkFeed?
    private var current: CVPixelBuffer?
    private var animation: DispatchSourceTimer?
    private var clientsStreaming = false
    private var closed = false
    /// A frame was dropped for not converting (logged once per camera, since each frame repeats it).
    private var frameFailureLogged = false

    /// How long the camera may take to appear after it was asked for.
    private static let appearAttempts = 100
    private static let appearInterval: DispatchTimeInterval = .milliseconds(100)

    init(deviceID: String, name: String, onClients: @escaping @Sendable (Bool) -> Void) throws {
        camera = PhoneCameraShared.Camera(uid: PhoneCameraShared.deviceUID(for: deviceID), name: name)
        self.onClients = onClients
        queue = DispatchQueue(label: "\(PhoneLinkIdentity.bundleID).camera.\(deviceID)", qos: .userInteractive)
        frames = try CameraFrames()
    }

    /// Opens the feed once the extension published the camera.
    func connect() {
        queue.async { self.connect(attempt: 0) }
    }

    private func connect(attempt: Int) {
        guard !closed else { return }
        guard let device = CameraExtensionLink.device(uid: camera.uid),
              let stream = CameraExtensionLink.sinkStream(of: device)
        else {
            if attempt + 1 < Self.appearAttempts {
                queue.asyncAfter(deadline: .now() + Self.appearInterval) { self.connect(attempt: attempt + 1) }
            } else {
                log.error("published camera not found in CoreMediaIO after 10 s; it shows nothing")
            }
            return
        }

        do {
            feed = try SinkFeed(device: device, stream: stream)
        } catch {
            log.error("camera feed not opened: \(error: error)")
            return
        }

        log.info("camera feed opened")
        if let current {
            send(current)
        }

        _ = CameraExtensionLink.observeClients(device, queue: queue) { [weak self] in
            self?.clientsChanged(device)
        }
        clientsChanged(device)
    }

    private func clientsChanged(_ device: CMIODeviceID) {
        guard !closed else { return }
        let streaming = CameraExtensionLink.clientsStreaming(device)
        guard streaming != clientsStreaming else { return }
        clientsStreaming = streaming
        onClients(streaming)
    }

    func show(_ placeholder: CameraPlaceholder) {
        queue.async { [self] in
            stopAnimation()
            guard placeholder == .connecting else {
                showStill(placeholder, frame: 0)
                return
            }

            var step = 0
            let timer = DispatchSource.makeTimerSource(queue: queue)
            timer.schedule(deadline: .now(), repeating: .milliseconds(200))
            timer.setEventHandler { [weak self] in
                self?.showStill(.connecting, frame: step)
                step = (step + 1) % CameraPlaceholderArt.connectingFrames
            }
            timer.resume()
            animation = timer
        }
    }

    private func showStill(_ placeholder: CameraPlaceholder, frame: Int) {
        guard let image = CameraPlaceholderArt.image(placeholder, frame: frame),
              let still = frames.frame(from: image)
        else {
            frameDropped()
            return
        }

        current = still
        send(still)
    }

    /// Decodes `unit`; shows the picture when `show`. `true` when it decoded to a picture.
    func video(_ unit: Data, show: Bool) -> Bool {
        guard let picture = decodeLock.withLock({ decoder.decode(unit) }) else { return false }
        if show {
            queue.async { [self] in
                stopAnimation()
                guard let frame = frames.frame(from: picture) else {
                    frameDropped()
                    return
                }
                current = frame
                send(frame)
            }
        }
        return true
    }

    func resetDecoder() {
        decodeLock.withLock { decoder.reset() }
    }

    private func send(_ frame: CVPixelBuffer) {
        guard let feed else { return }
        guard let sample = CameraFrames.sample(frame) else {
            frameDropped()
            return
        }
        feed.enqueue(sample)
    }

    private func frameDropped() {
        guard !frameFailureLogged else { return }
        frameFailureLogged = true
        log.error("camera frame not converted; it and later failing frames are dropped")
    }

    private func stopAnimation() {
        animation?.cancel()
        animation = nil
    }

    func close() {
        queue.sync {
            closed = true
            stopAnimation()
            feed?.stop()
            feed = nil
        }
        resetDecoder()
    }
}
