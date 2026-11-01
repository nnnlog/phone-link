import FileProvider
import Foundation

/// The contract between the app, which runs the engine, and its File Provider extension, which shows
/// the phone's files in the Finder. The app connects to the extension through the extension's File
/// Provider service and exports `PhoneFilesHost` on that connection; the extension calls it for every
/// item, enumeration, download, upload and change, and reports download progress back through
/// `PhoneFilesExtension`. Items and change sets travel as JSON.
public enum PhoneFilesXPC {
    /// The extension's service the app connects to.
    public static let serviceName = NSFileProviderServiceName("\(PhoneLinkIdentity.bundleID).phone-files.host")
    /// Posted (as a distributed notification, with no user info) by an extension process that has no
    /// connection to the app: when it starts, and when a call finds none. The system starts and stops
    /// extension processes on its own, and a connection the app opened ends with the process it went to,
    /// so the app connects again when it sees this.
    public static let needsHostNotification = Notification.Name("\(PhoneLinkIdentity.bundleID).phone-files.needs-host")
    /// The domain of the errors the host returns; the code is a `PhoneFilesXPCError`.
    public static let errorDomain = "\(PhoneLinkIdentity.bundleID).phone-files"

    public static func error(_ code: PhoneFilesXPCError, _ message: String) -> NSError {
        NSError(domain: errorDomain, code: code.rawValue, userInfo: [NSLocalizedDescriptionKey: message])
    }
}

/// Why a host call failed.
public enum PhoneFilesXPCError: Int, Sendable {
    /// The domain is not the selected phone's, or phone files is not running for it.
    case notRunning = 1
    case unknownItem
    case exists
    /// The name stays on the Mac: it is not sent to the phone.
    case ignored
    case notSupported
    /// The Mac's copy of the tree is behind the phone.
    case notInSync
    case permissionRequired
    /// The phone or the app cannot be reached.
    case unavailable
    case fileNotFound
    case cancelled
    /// The same file is already being downloaded.
    case inProgress
    /// Only some of a folder's items were deleted.
    case partial
    case failed
}

/// One entry of the phone's tree. The identifier is `""` for the root.
public struct PhoneFileRecord: Codable, Sendable, Equatable {
    public var identifier: String
    public var parent: String
    public var name: String
    public var isDirectory: Bool
    public var size: Int64
    /// Unix seconds.
    public var created: Int64?
    public var modified: Int64?
    public var contentVersion: String
    public var metadataVersion: String
    public var hasThumbnail: Bool
    /// A folder the tree adds between the root and a phone item.
    public var synthetic: Bool

    public init(
        identifier: String, parent: String, name: String, isDirectory: Bool, size: Int64, created: Int64?,
        modified: Int64?, contentVersion: String, metadataVersion: String, hasThumbnail: Bool, synthetic: Bool
    ) {
        self.identifier = identifier
        self.parent = parent
        self.name = name
        self.isDirectory = isDirectory
        self.size = size
        self.created = created
        self.modified = modified
        self.contentVersion = contentVersion
        self.metadataVersion = metadataVersion
        self.hasThumbnail = hasThumbnail
        self.synthetic = synthetic
    }
}

/// What changed after a generation.
public struct PhoneFileChangeSet: Codable, Sendable, Equatable {
    public var updated: [PhoneFileRecord]
    public var removed: [String]
    public var generation: Int64

    public init(updated: [PhoneFileRecord], removed: [String], generation: Int64) {
        self.updated = updated
        self.removed = removed
        self.generation = generation
    }
}

/// The app's side. Every reply is called once; `Data` is JSON (`PhoneFileRecord`, `[PhoneFileRecord]`,
/// `PhoneFileChangeSet`).
@objc public protocol PhoneFilesHost {
    func item(domain: String, identifier: String, reply: @escaping @Sendable (Data?, NSError?) -> Void)
    func children(domain: String, identifier: String, reply: @escaping @Sendable (Data?, NSError?) -> Void)
    func generation(domain: String, reply: @escaping @Sendable (Int64, NSError?) -> Void)
    /// `expired` when `generation` is too old: read everything again.
    func changes(domain: String, since generation: Int64, reply: @escaping @Sendable (Data?, Bool, NSError?) -> Void)
    func thumbnail(domain: String, identifier: String, reply: @escaping @Sendable (Data?, NSError?) -> Void)
    /// The file's content (a handle open for reading) and its item.
    func download(domain: String, identifier: String, requestID: String, reply: @escaping @Sendable (FileHandle?, Data?, NSError?) -> Void)
    /// Times are Unix milliseconds.
    func upload(
        domain: String, parent: String, name: String, content: FileHandle, createdMs: NSNumber?, modifiedMs: NSNumber?,
        requestID: String, reply: @escaping @Sendable (Data?, NSError?) -> Void
    )
    func createFolder(domain: String, parent: String, name: String, reply: @escaping @Sendable (Data?, NSError?) -> Void)
    func rename(domain: String, identifier: String, parent: String, name: String, reply: @escaping @Sendable (Data?, NSError?) -> Void)
    func delete(domain: String, identifier: String, reply: @escaping @Sendable (NSError?) -> Void)
    func cancel(requestID: String)
}

/// The extension's side.
@objc public protocol PhoneFilesExtension {
    /// The app is there; the extension keeps this connection for its calls.
    func attach(reply: @escaping @Sendable () -> Void)
    func progress(requestID: String, done: Int64, total: Int64)
}
