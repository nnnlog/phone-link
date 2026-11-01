import CoreMediaIO
import Foundation
import os

let log = Logger(subsystem: PhoneCameraShared.extensionBundleID, category: "camera")

/// The extension's provider: one camera per phone the app asks for, through the provider's cameras
/// property. The cameras go away when the app disconnects, so none outlives the app.
final class CameraProviderSource: NSObject, CMIOExtensionProviderSource {
    private(set) var provider: CMIOExtensionProvider!
    private let camerasProperty = CMIOExtensionProperty(
        rawValue: PhoneCameraShared.propertyName(PhoneCameraShared.camerasSelector)
    )
    private let lock = NSLock()
    private var cameras: [PhoneCameraShared.Camera] = []
    private var devices: [String: CameraDeviceSource] = [:]
    /// The app's signing identifier: its bundle identifier.
    private let appSigningID = PhoneLinkIdentity.bundleID

    init(clientQueue: DispatchQueue?) {
        super.init()
        provider = CMIOExtensionProvider(source: self, clientQueue: clientQueue)
    }

    func connect(to client: CMIOExtensionClient) throws {}

    func disconnect(from client: CMIOExtensionClient) {
        if client.signingID == appSigningID {
            log.info("app disconnected; its cameras are removed")
            apply([])
        }
    }

    var availableProperties: Set<CMIOExtensionProperty> {
        [.providerManufacturer, camerasProperty]
    }

    func providerProperties(forProperties properties: Set<CMIOExtensionProperty>) throws -> CMIOExtensionProviderProperties {
        let result = CMIOExtensionProviderProperties(dictionary: [:])
        if properties.contains(.providerManufacturer) {
            result.manufacturer = PhoneCameraShared.manufacturer
        }
        if properties.contains(camerasProperty) {
            let json = try JSONEncoder().encode(lock.withLock { cameras })
            result.setPropertyState(
                CMIOExtensionPropertyState(value: String(decoding: json, as: UTF8.self) as NSString),
                forProperty: camerasProperty
            )
        }
        return result
    }

    func setProviderProperties(_ providerProperties: CMIOExtensionProviderProperties) throws {
        guard let state = providerProperties.propertiesDictionary[camerasProperty] else { return }
        guard let text = state.value as? String,
              let wanted = try? JSONDecoder().decode([PhoneCameraShared.Camera].self, from: Data(text.utf8))
        else {
            log.error("cameras property not decoded; the cameras stay as they were")
            throw CocoaError(.coderReadCorrupt)
        }

        apply(wanted)
    }

    /// Publishes exactly `wanted`: removes the others, adds the new ones (a renamed camera is added again).
    private func apply(_ wanted: [PhoneCameraShared.Camera]) {
        let (removed, added): ([CameraDeviceSource], [PhoneCameraShared.Camera]) = lock.withLock {
            let removed = devices.filter { uid, _ in !wanted.contains { $0.uid == uid && cameras.contains($0) } }
            for uid in removed.keys {
                devices[uid] = nil
            }

            let added = wanted.filter { devices[$0.uid] == nil }
            cameras = wanted
            return (Array(removed.values), added)
        }

        for device in removed {
            device.stop()
            do {
                try provider.removeDevice(device.device)
                log.info("camera removed")
            } catch {
                log.error("camera not removed: \(String(describing: error), privacy: .public)")
            }
        }

        for camera in added {
            guard let uid = UUID(uuidString: camera.uid) else {
                log.error("camera not added; its uid \(camera.uid, privacy: .private) is no UUID")
                continue
            }
            do {
                let source = try CameraDeviceSource(uid: uid, name: camera.name)
                try provider.addDevice(source.device)
                lock.withLock { devices[camera.uid] = source }
                log.info("camera added")
            } catch {
                log.error("camera not added: \(String(describing: error), privacy: .public)")
            }
        }
    }
}
