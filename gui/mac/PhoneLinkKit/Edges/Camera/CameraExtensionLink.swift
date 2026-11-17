import CoreMediaIO
import Foundation
import os

private let log = AppLog(category: "camera")

/// The app's side of the camera extension, through the CoreMediaIO hardware API: the extension's
/// provider (its cameras property), a camera by its UID, the camera's sink stream the app feeds, and
/// the camera's clients property.
enum CameraExtensionLink {
    static func address(_ selector: CMIOObjectPropertySelector) -> CMIOObjectPropertyAddress {
        CMIOObjectPropertyAddress(
            mSelector: selector,
            mScope: CMIOObjectPropertyScope(kCMIOObjectPropertyScopeGlobal),
            mElement: CMIOObjectPropertyElement(kCMIOObjectPropertyElementMain)
        )
    }

    /// The object ids `selector` lists on `object`.
    static func objects(of object: CMIOObjectID, _ selector: Int) -> [CMIOObjectID] {
        var address = address(CMIOObjectPropertySelector(selector))
        var size: UInt32 = 0
        guard CMIOObjectGetPropertyDataSize(object, &address, 0, nil, &size) == noErr, size > 0 else { return [] }

        var ids = [CMIOObjectID](repeating: 0, count: Int(size) / MemoryLayout<CMIOObjectID>.size)
        var used: UInt32 = 0
        guard CMIOObjectGetPropertyData(object, &address, 0, nil, size, &used, &ids) == noErr else { return [] }
        return Array(ids.prefix(Int(used) / MemoryLayout<CMIOObjectID>.size))
    }

    static func uint32(of object: CMIOObjectID, _ selector: CMIOObjectPropertySelector) -> UInt32? {
        var address = address(selector)
        var value: UInt32 = 0
        var used: UInt32 = 0
        let status = CMIOObjectGetPropertyData(object, &address, 0, nil, UInt32(MemoryLayout<UInt32>.size), &used, &value)
        return status == noErr ? value : nil
    }

    static func string(of object: CMIOObjectID, _ selector: CMIOObjectPropertySelector) -> String? {
        var address = address(selector)
        var size: UInt32 = 0
        guard CMIOObjectGetPropertyDataSize(object, &address, 0, nil, &size) == noErr,
              size == UInt32(MemoryLayout<CFString>.size)
        else {
            return nil
        }

        var value: Unmanaged<CFString>?
        var used: UInt32 = 0
        guard CMIOObjectGetPropertyData(object, &address, 0, nil, size, &used, &value) == noErr else { return nil }
        return value?.takeRetainedValue() as String?
    }

    /// The extension's provider: the plug-in whose manufacturer is the extension's.
    static func provider() -> CMIOObjectID? {
        objects(of: CMIOObjectID(kCMIOObjectSystemObject), kCMIOObjectPropertyOwnedObjects).first { object in
            uint32(of: object, CMIOObjectPropertySelector(kCMIOObjectPropertyClass)) == CMIOClassID(kCMIOPlugInClassID)
                && string(of: object, CMIOObjectPropertySelector(kCMIOObjectPropertyManufacturer)) == PhoneCameraShared.manufacturer
        }
    }

    /// Asks the extension to publish exactly `cameras`.
    static func publish(_ cameras: [PhoneCameraShared.Camera], provider: CMIOObjectID) throws {
        let json = String(decoding: try JSONEncoder().encode(cameras), as: UTF8.self) as CFString
        var address = address(PhoneCameraShared.camerasSelector)
        var value: Unmanaged<CFString>? = Unmanaged.passUnretained(json)
        let status = CMIOObjectSetPropertyData(provider, &address, 0, nil, UInt32(MemoryLayout<CFString>.size), &value)
        guard status == noErr else { throw CameraLinkError.status("setting the cameras", status) }
    }

    /// The camera whose UID is `uid`, once the extension published it.
    static func device(uid: String) -> CMIODeviceID? {
        objects(of: CMIOObjectID(kCMIOObjectSystemObject), kCMIOHardwarePropertyDevices).first { device in
            string(of: device, CMIOObjectPropertySelector(kCMIODevicePropertyDeviceUID)) == uid
        }
    }

    /// The camera's sink stream: its output stream (direction 0), which the app feeds.
    static func sinkStream(of device: CMIODeviceID) -> CMIOStreamID? {
        objects(of: device, kCMIODevicePropertyStreams).first { stream in
            uint32(of: stream, CMIOObjectPropertySelector(kCMIOStreamPropertyDirection)) == 0
        }
    }

    /// The camera's clients are streaming.
    static func clientsStreaming(_ device: CMIODeviceID) -> Bool {
        string(of: device, PhoneCameraShared.clientsSelector) == "1"
    }

    /// Calls `handler` on `queue` whenever the camera's clients property changes.
    static func observeClients(_ device: CMIODeviceID, queue: DispatchQueue, handler: @escaping () -> Void) -> Bool {
        var address = address(PhoneCameraShared.clientsSelector)
        let status = CMIOObjectAddPropertyListenerBlock(device, &address, queue) { _, _ in handler() }
        if status != noErr {
            log.error("camera clients not observed: status \(status); the engine is not told when apps use the camera")
        }
        return status == noErr
    }
}

/// The app's feed into a camera's sink stream. Frames the extension has not taken yet are replaced by
/// newer ones.
final class SinkFeed: @unchecked Sendable {
    private let device: CMIODeviceID
    private let stream: CMIOStreamID
    private let queue: CMSimpleQueue

    init(device: CMIODeviceID, stream: CMIOStreamID) throws {
        var queue: Unmanaged<CMSimpleQueue>?
        let status = CMIOStreamCopyBufferQueue(stream, { _, _, _ in }, nil, &queue)
        guard status == noErr, let queue else { throw CameraLinkError.status("opening the feed", status) }

        self.device = device
        self.stream = stream
        self.queue = queue.takeRetainedValue()

        let started = CMIODeviceStartStream(device, stream)
        guard started == noErr else { throw CameraLinkError.status("starting the feed", started) }
    }

    func enqueue(_ sample: CMSampleBuffer) {
        while CMSimpleQueueGetCount(queue) >= CMSimpleQueueGetCapacity(queue),
              let old = CMSimpleQueueDequeue(queue)
        {
            Unmanaged<CMSampleBuffer>.fromOpaque(old).release()
        }

        let element = Unmanaged.passRetained(sample).toOpaque()
        if CMSimpleQueueEnqueue(queue, element: element) != noErr {
            Unmanaged<CMSampleBuffer>.fromOpaque(element).release()
        }
    }

    func stop() {
        CMIODeviceStopStream(device, stream)
    }
}

enum CameraLinkError: LocalizedError {
    case status(String, OSStatus)

    var errorDescription: String? {
        switch self {
        case let .status(what, status): "\(what) failed (\(status))"
        }
    }
}
