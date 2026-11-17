import Foundation
import os
import PhoneLinkEngine

private let log = AppLog(category: "phone-files")

/// The app's side of the File Provider extension's calls: each one goes to the engine's phone-files
/// API and comes back as JSON or a handle. A download's temporary file is opened, unlinked and handed
/// over as an open handle, so the sandboxed extension reads it without a path; an upload's content
/// arrives as a handle and is copied to a temporary file the engine reads.
public final class PhoneFilesHostService: NSObject, PhoneFilesHost, @unchecked Sendable {
    private let api: any PhoneFilesApiProtocol
    private let lock = NSLock()
    private var progressSink: (@Sendable (String, Int64, Int64) -> Void)?
    /// The phone was unavailable for a call, and no call has succeeded since (logged once: the Finder
    /// keeps asking while the phone is away).
    private var unavailableLogged = false

    public init(api: any PhoneFilesApiProtocol) {
        self.api = api
    }

    /// Where download progress goes (the extension's side of the connection).
    func setProgressSink(_ sink: (@Sendable (String, Int64, Int64) -> Void)?) {
        lock.lock()
        progressSink = sink
        lock.unlock()
    }

    private func reportProgress(_ requestID: String, _ done: Int64, _ total: Int64) {
        lock.lock()
        let sink = progressSink
        lock.unlock()
        sink?(requestID, done, total)
    }

    // MARK: - Failures

    /// `error` for the extension, logged once here: the engine does not log what it returns. Answers
    /// the File Provider expects (no engine running, an item gone, a cancel) are debug; the phone
    /// being unavailable is logged when it starts.
    private func fail(_ error: Error, _ subject: StaticString) -> NSError {
        let converted = Self.xpcError(error)
        switch PhoneFilesXPCError(rawValue: converted.code) {
        case .notRunning?, .unknownItem?, .ignored?, .cancelled?:
            log.debug("\(subject.description, privacy: .public): \(error: error)")
        case .unavailable?:
            let first = lock.withLock {
                defer { unavailableLogged = true }
                return !unavailableLogged
            }
            if first {
                log.notice("\(subject.description, privacy: .public): \(error: error); later calls fail the same until one succeeds")
            }
        default:
            log.error("\(subject.description, privacy: .public): \(error: error)")
        }
        return converted
    }

    /// A call reached the phone: a later unavailable phone is news again.
    private func succeeded() {
        lock.withLock { unavailableLogged = false }
    }

    // MARK: - Conversions

    static func record(_ item: PhoneFileItem) -> PhoneFileRecord {
        PhoneFileRecord(
            identifier: item.identifier,
            parent: item.parent,
            name: item.name,
            isDirectory: item.isDirectory,
            size: item.size,
            created: item.created,
            modified: item.modified,
            contentVersion: item.contentVersion,
            metadataVersion: item.metadataVersion,
            hasThumbnail: item.hasThumbnail,
            synthetic: item.synthetic
        )
    }

    static func encode<T: Encodable>(_ value: T) -> Data? {
        try? JSONEncoder().encode(value)
    }

    /// The error the extension reads.
    static func xpcError(_ error: Error) -> NSError {
        guard case let PhoneFileError.Failure(failure, message) = error else {
            return PhoneFilesXPC.error(.failed, UserText.describe(error))
        }
        let code: PhoneFilesXPCError = switch failure {
        case .notRunning: .notRunning
        case .unknownItem: .unknownItem
        case .exists: .exists
        case .ignored: .ignored
        case .notSupported: .notSupported
        case .notInSync: .notInSync
        case .permissionRequired: .permissionRequired
        case .phoneUnavailable, .ltwOff, .metered, .pcLocked, .timedOut: .unavailable
        case .fileNotFound: .fileNotFound
        case .cancelled: .cancelled
        case .inProgress: .inProgress
        case .partial: .partial
        case .failed: .failed
        }
        return PhoneFilesXPC.error(code, message)
    }

    // MARK: - PhoneFilesHost

    public func item(domain: String, identifier: String, reply: @escaping @Sendable (Data?, NSError?) -> Void) {
        do {
            reply(Self.encode(Self.record(try api.item(domain: domain, identifier: identifier))), nil)
        } catch {
            reply(nil, fail(error, "phone file not read"))
        }
    }

    public func children(domain: String, identifier: String, reply: @escaping @Sendable (Data?, NSError?) -> Void) {
        do {
            reply(Self.encode(try api.children(domain: domain, identifier: identifier).map(Self.record)), nil)
        } catch {
            reply(nil, fail(error, "phone folder not listed"))
        }
    }

    public func generation(domain: String, reply: @escaping @Sendable (Int64, NSError?) -> Void) {
        do {
            reply(try api.generation(domain: domain), nil)
        } catch {
            reply(0, fail(error, "phone files generation not read"))
        }
    }

    public func changes(domain: String, since generation: Int64, reply: @escaping @Sendable (Data?, Bool, NSError?) -> Void) {
        do {
            guard let changes = try api.changes(domain: domain, generation: generation) else {
                return reply(nil, true, nil)
            }

            let set = PhoneFileChangeSet(
                updated: changes.updated.map(Self.record),
                removed: changes.removed,
                generation: changes.generation
            )
            reply(Self.encode(set), false, nil)
        } catch {
            reply(nil, false, fail(error, "phone file changes not read"))
        }
    }

    public func thumbnail(domain: String, identifier: String, reply: @escaping @Sendable (Data?, NSError?) -> Void) {
        let api = self.api
        Task { [self] in
            do {
                reply(try await api.thumbnail(domain: domain, identifier: identifier), nil)
                succeeded()
            } catch {
                reply(nil, fail(error, "phone file thumbnail not fetched"))
            }
        }
    }

    public func download(
        domain: String,
        identifier: String,
        requestID: String,
        reply: @escaping @Sendable (FileHandle?, Data?, NSError?) -> Void
    ) {
        let api = self.api
        let progress = DownloadProgress { [weak self] done, total in
            self?.reportProgress(requestID, Int64(clamping: done), Int64(clamping: total))
        }

        Task { [self] in
            let content: PhoneFileContent
            do {
                content = try await api.download(domain: domain, identifier: identifier, requestId: requestID, progress: progress)
            } catch {
                return reply(nil, nil, self.fail(error, "phone file not downloaded"))
            }

            self.succeeded()
            let url = URL(fileURLWithPath: content.path)
            do {
                let handle = try FileHandle(forReadingFrom: url)
                // The open handle keeps the content readable; the name is not needed any more.
                try? FileManager.default.removeItem(at: url)
                reply(handle, Self.encode(Self.record(content.item)), nil)
            } catch {
                log.error("downloaded phone file not opened: \(error: error)")
                reply(nil, nil, Self.xpcError(error))
            }
        }
    }

    public func upload(
        domain: String,
        parent: String,
        name: String,
        content: FileHandle,
        createdMs: NSNumber?,
        modifiedMs: NSNumber?,
        requestID: String,
        reply: @escaping @Sendable (Data?, NSError?) -> Void
    ) {
        let api = self.api
        let created = createdMs?.int64Value
        let modified = modifiedMs?.int64Value
        let source = FileManager.default.temporaryDirectory.appendingPathComponent("phone-files-upload-\(UUID().uuidString)")

        do {
            try Self.copy(content, to: source)
        } catch {
            try? content.close()
            log.error("file to upload not copied: \(error: error)")
            return reply(nil, PhoneFilesXPC.error(.failed, "the file could not be read: \(error.localizedDescription)"))
        }
        try? content.close()

        Task { [self] in
            defer { try? FileManager.default.removeItem(at: source) }
            do {
                let item = try await api.upload(
                    domain: domain,
                    parent: parent,
                    name: name,
                    source: source.path,
                    createdMs: created,
                    modifiedMs: modified,
                    requestId: requestID
                )
                reply(Self.encode(Self.record(item)), nil)
                self.succeeded()
            } catch {
                reply(nil, self.fail(error, "phone file not uploaded"))
            }
        }
    }

    private static func copy(_ handle: FileHandle, to url: URL) throws {
        guard FileManager.default.createFile(atPath: url.path, contents: nil, attributes: [.posixPermissions: 0o600]) else {
            throw CocoaError(.fileWriteUnknown)
        }

        let out = try FileHandle(forWritingTo: url)
        defer { try? out.close() }
        while let chunk = try handle.read(upToCount: 1 << 20), !chunk.isEmpty {
            try out.write(contentsOf: chunk)
        }
    }

    public func createFolder(domain: String, parent: String, name: String, reply: @escaping @Sendable (Data?, NSError?) -> Void) {
        let api = self.api
        Task { [self] in
            do {
                reply(Self.encode(Self.record(try await api.createFolder(domain: domain, parent: parent, name: name))), nil)
                self.succeeded()
            } catch {
                reply(nil, self.fail(error, "phone folder not created"))
            }
        }
    }

    public func rename(domain: String, identifier: String, parent: String, name: String, reply: @escaping @Sendable (Data?, NSError?) -> Void) {
        let api = self.api
        Task { [self] in
            do {
                let item = try await api.rename(domain: domain, identifier: identifier, parent: parent, name: name)
                reply(Self.encode(Self.record(item)), nil)
                self.succeeded()
            } catch {
                reply(nil, self.fail(error, "phone file not renamed"))
            }
        }
    }

    public func delete(domain: String, identifier: String, reply: @escaping @Sendable (NSError?) -> Void) {
        let api = self.api
        Task { [self] in
            do {
                try await api.delete(domain: domain, identifier: identifier)
                reply(nil)
                self.succeeded()
            } catch {
                reply(self.fail(error, "phone file not deleted"))
            }
        }
    }

    public func cancel(requestID: String) {
        api.cancel(requestId: requestID)
    }
}

/// A download's progress, handed to the engine.
final class DownloadProgress: PhoneFileProgress, @unchecked Sendable {
    private let report: @Sendable (UInt64, UInt64) -> Void

    init(_ report: @escaping @Sendable (UInt64, UInt64) -> Void) {
        self.report = report
    }

    func progress(done: UInt64, total: UInt64) {
        report(done, total)
    }
}
