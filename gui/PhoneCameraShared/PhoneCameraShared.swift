import CoreMediaIO
import CryptoKit
import Foundation

/// What the app and its camera extension agree on. The app tells the extension which cameras to
/// publish through a custom property of the extension's provider, and the extension tells the app
/// whether a camera's clients are streaming through a custom property of the camera. The app feeds
/// each camera through its sink stream.
enum PhoneCameraShared {
    /// The camera extension's bundle identifier.
    static let extensionBundleID = "\(PhoneLinkIdentity.bundleID).CameraExtension"
    /// The provider's manufacturer, by which the app finds the extension among the system's plug-ins.
    static let manufacturer = "Phone Link"

    /// The provider property the app writes: the cameras to publish, a JSON array of `Camera`.
    static let camerasSelector = fourCharCode("plcm")
    /// The camera property the extension publishes: `"1"` while a client streams, else `"0"`.
    static let clientsSelector = fourCharCode("plcl")

    /// The custom-property name of `selector` on the extension side.
    static func propertyName(_ selector: CMIOObjectPropertySelector) -> String {
        "4cc_\(fourCharString(selector))_glob_0000"
    }

    /// Every camera's format: NV12, 1280 × 720 at 30 fps.
    static let width: Int32 = 1280
    static let height: Int32 = 720
    static let framesPerSecond: Int32 = 30
    static let pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange

    /// One camera to publish.
    struct Camera: Codable, Equatable, Sendable {
        /// The camera's device UID.
        var uid: String
        var name: String
    }

    /// The device UID of the camera for phone `deviceID`: the same on every launch, and changed only by
    /// a different bundle identifier.
    static func deviceUID(for deviceID: String) -> String {
        var bytes = Array(SHA256.hash(data: Data("\(PhoneLinkIdentity.bundleID).camera/\(deviceID)".utf8)).prefix(16))
        bytes[6] = (bytes[6] & 0x0F) | 0x50
        bytes[8] = (bytes[8] & 0x3F) | 0x80

        let uuid = UUID(uuid: (
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
        ))
        return uuid.uuidString
    }

    static func fourCharCode(_ text: String) -> CMIOObjectPropertySelector {
        precondition(text.utf8.count == 4, "a four-character code has four ASCII characters")
        return text.utf8.reduce(0) { ($0 << 8) | UInt32($1) }
    }

    static func fourCharString(_ code: CMIOObjectPropertySelector) -> String {
        let bytes = [24, 16, 8, 0].map { UInt8((code >> $0) & 0xFF) }
        return String(decoding: bytes, as: UTF8.self)
    }
}
