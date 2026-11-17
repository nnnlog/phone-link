import FileProvider
import Foundation

/// The phone's storage in the Finder: one domain per phone, served by the app over its connection
/// (`HostConnection`). Opening a file downloads it; a new file or folder goes to the phone; a rename
/// or a move inside the domain renames or moves it there; a delete deletes it there (the phone keeps
/// deleted files in its own recycle bin). Moving an item to the Trash sends nothing, so the file stays
/// on the phone. An item the phone did not delete is put back.
final class FileProviderExtension: NSObject, NSFileProviderReplicatedExtension, NSFileProviderServicing, NSFileProviderThumbnailing {
    private let domain: NSFileProviderDomain
    private var domainID: String { domain.identifier.rawValue }

    required init(domain: NSFileProviderDomain) {
        self.domain = domain
        super.init()
        _ = HostConnection.shared
    }

    func invalidate() {}

    // MARK: - Items

    func item(
        for identifier: NSFileProviderItemIdentifier,
        request: NSFileProviderRequest,
        completionHandler: @escaping (NSFileProviderItem?, Error?) -> Void
    ) -> Progress {
        let progress = Progress(totalUnitCount: 1)
        record(identifier) { item, error in
            progress.completedUnitCount = 1
            completionHandler(item, error)
        }
        return progress
    }

    /// The item `identifier` names, from the app.
    private func record(_ identifier: NSFileProviderItemIdentifier, _ completion: @escaping (FileProviderItem?, Error?) -> Void) {
        let once = Once()
        HostConnection.shared.withHost({ error in if once.claim() { completion(nil, error) } }) { [domainID] host in
            host.item(domain: domainID, identifier: engineIdentifier(identifier)) { data, error in
                guard once.claim() else { return }
                if let error = providerError(error) { return completion(nil, error) }
                guard let item = FileProviderItem.decode(data) else {
                    return completion(nil, NSFileProviderError(.noSuchItem))
                }
                completion(item, nil)
            }
        }
    }

    func fetchContents(
        for itemIdentifier: NSFileProviderItemIdentifier,
        version requestedVersion: NSFileProviderItemVersion?,
        request: NSFileProviderRequest,
        completionHandler: @escaping (URL?, NSFileProviderItem?, Error?) -> Void
    ) -> Progress {
        let progress = Progress(totalUnitCount: 100)
        let requestID = UUID().uuidString
        let once = Once()
        let finish: (URL?, NSFileProviderItem?, Error?) -> Void = { url, item, error in
            guard once.claim() else { return }
            HostConnection.shared.onProgress(requestID, nil)
            completionHandler(url, item, error)
        }

        HostConnection.shared.onProgress(requestID) { done, total in
            if total > 0 {
                progress.totalUnitCount = total
                progress.completedUnitCount = done
            }
        }

        HostConnection.shared.withHost({ finish(nil, nil, $0) }) { [domain, domainID] host in
            progress.cancellationHandler = { host.cancel(requestID: requestID) }
            host.download(domain: domainID, identifier: engineIdentifier(itemIdentifier), requestID: requestID) { handle, data, error in
                if let error = providerError(error) { return finish(nil, nil, error) }
                guard let handle, let item = FileProviderItem.decode(data) else {
                    return finish(nil, nil, NSFileProviderError(.cannotSynchronize))
                }
                do {
                    let url = try Self.copy(handle, into: domain)
                    finish(url, item, nil)
                } catch {
                    log.error("downloaded content not written: \(String(describing: error), privacy: .public)")
                    finish(nil, nil, error)
                }
            }
        }

        return progress
    }

    /// Copies what `handle` reads into a new file of the domain's temporary folder.
    private static func copy(_ handle: FileHandle, into domain: NSFileProviderDomain) throws -> URL {
        defer { try? handle.close() }
        guard let manager = NSFileProviderManager(for: domain) else { throw NSFileProviderError(.providerNotFound) }
        let url = try manager.temporaryDirectoryURL().appendingPathComponent(UUID().uuidString)
        guard FileManager.default.createFile(atPath: url.path, contents: nil) else {
            throw CocoaError(.fileWriteUnknown)
        }

        let out = try FileHandle(forWritingTo: url)
        defer { try? out.close() }
        while let chunk = try handle.read(upToCount: 1 << 20), !chunk.isEmpty {
            try out.write(contentsOf: chunk)
        }
        return url
    }

    func createItem(
        basedOn itemTemplate: NSFileProviderItem,
        fields: NSFileProviderItemFields,
        contents url: URL?,
        options: NSFileProviderCreateItemOptions = [],
        request: NSFileProviderRequest,
        completionHandler: @escaping (NSFileProviderItem?, NSFileProviderItemFields, Bool, Error?) -> Void
    ) -> Progress {
        let progress = Progress(totalUnitCount: 1)
        let once = Once()
        let finish: (NSFileProviderItem?, Error?) -> Void = { item, error in
            guard once.claim() else { return }
            progress.completedUnitCount = 1
            completionHandler(item, [], false, error)
        }

        let parent = itemTemplate.parentItemIdentifier
        // The system asks to create the root or the trash when it lost track of them: they exist.
        if itemTemplate.itemIdentifier == .rootContainer || itemTemplate.itemIdentifier == .trashContainer {
            record(itemTemplate.itemIdentifier) { item, error in finish(item, error) }
            return progress
        }

        if options.contains(.mayAlreadyExist) {
            // A reimport of an item found on disk: the phone's item of that name in that folder, if any.
            existingChild(named: itemTemplate.filename, in: parent) { [weak self] existing in
                guard let self else { return finish(nil, NSFileProviderError(.serverUnreachable)) }
                if let existing {
                    return finish(existing, nil)
                }
                self.create(itemTemplate, contents: url, progress: progress, finish: finish)
            }
            return progress
        }

        create(itemTemplate, contents: url, progress: progress, finish: finish)
        return progress
    }

    /// The entry named `name` directly inside `parent`, from the app; `nil` when there is none.
    private func existingChild(named name: String, in parent: NSFileProviderItemIdentifier, _ completion: @escaping (FileProviderItem?) -> Void) {
        let once = Once()
        HostConnection.shared.withHost({ _ in if once.claim() { completion(nil) } }) { [domainID] host in
            host.children(domain: domainID, identifier: engineIdentifier(parent)) { data, _ in
                guard once.claim() else { return }
                let records = data.flatMap { FileProviderItem.decodeList([PhoneFileRecord].self, $0) } ?? []
                completion(records.first { $0.name == name }.map { FileProviderItem($0) })
            }
        }
    }

    /// A new folder, or a new file uploaded from `url`.
    private func create(
        _ itemTemplate: NSFileProviderItem,
        contents url: URL?,
        progress: Progress,
        finish: @escaping (NSFileProviderItem?, Error?) -> Void
    ) {
        let parent = itemTemplate.parentItemIdentifier
        if parent == .trashContainer {
            finish(nil, NSFileProviderError(.excludedFromSync))
            return
        }

        let type = itemTemplate.contentType ?? .data
        let name = itemTemplate.filename
        if type.conforms(to: .folder) || type.conforms(to: .directory) {
            HostConnection.shared.withHost({ finish(nil, $0) }) { [domainID] host in
                host.createFolder(domain: domainID, parent: engineIdentifier(parent), name: name) { data, error in
                    if let error = providerError(error) { return finish(nil, error) }
                    finish(FileProviderItem.decode(data), nil)
                }
            }
            return
        }

        guard !type.conforms(to: .symbolicLink), !type.conforms(to: .aliasFile), let url else {
            finish(nil, NSFileProviderError(.excludedFromSync))
            return
        }

        let content: FileHandle
        do {
            content = try FileHandle(forReadingFrom: url)
        } catch {
            log.error("file to upload not opened: \(String(describing: error), privacy: .public)")
            finish(nil, error)
            return
        }

        let created = itemTemplate.creationDate??.timeIntervalSince1970
        let modified = itemTemplate.contentModificationDate??.timeIntervalSince1970
        let requestID = UUID().uuidString
        HostConnection.shared.withHost({ finish(nil, $0) }) { [domainID] host in
            progress.cancellationHandler = { host.cancel(requestID: requestID) }
            host.upload(
                domain: domainID,
                parent: engineIdentifier(parent),
                name: name,
                content: content,
                createdMs: created.map { NSNumber(value: Int64($0 * 1000)) },
                modifiedMs: modified.map { NSNumber(value: Int64($0 * 1000)) },
                requestID: requestID
            ) { data, error in
                try? content.close()
                if let error = providerError(error) { return finish(nil, error) }
                finish(FileProviderItem.decode(data), nil)
            }
        }
    }

    func modifyItem(
        _ item: NSFileProviderItem,
        baseVersion version: NSFileProviderItemVersion,
        changedFields: NSFileProviderItemFields,
        contents newContents: URL?,
        options: NSFileProviderModifyItemOptions = [],
        request: NSFileProviderRequest,
        completionHandler: @escaping (NSFileProviderItem?, NSFileProviderItemFields, Bool, Error?) -> Void
    ) -> Progress {
        let progress = Progress(totalUnitCount: 1)
        let once = Once()
        let finish: (NSFileProviderItem?, Error?) -> Void = { item, error in
            guard once.claim() else { return }
            progress.completedUnitCount = 1
            completionHandler(item, [], false, error)
        }

        if changedFields.contains(.contents) {
            // An edit has no way back to the phone.
            finish(nil, CocoaError(.featureUnsupported))
            return progress
        }

        let identifier = item.itemIdentifier
        guard changedFields.contains(.filename) || changedFields.contains(.parentItemIdentifier) else {
            record(identifier) { current, error in finish(current, error) }
            return progress
        }

        if item.parentItemIdentifier == .trashContainer {
            // Moved to the Trash: nothing is sent; the file stays on the phone.
            record(identifier) { current, error in
                finish(current.map { FileProviderItem($0.record, inTrash: true) }, error)
            }
            return progress
        }

        HostConnection.shared.withHost({ finish(nil, $0) }) { [domainID] host in
            host.rename(
                domain: domainID,
                identifier: engineIdentifier(identifier),
                parent: engineIdentifier(item.parentItemIdentifier),
                name: item.filename
            ) { data, error in
                if let error = providerError(error) { return finish(nil, error) }
                finish(FileProviderItem.decode(data), nil)
            }
        }

        return progress
    }

    func deleteItem(
        identifier: NSFileProviderItemIdentifier,
        baseVersion version: NSFileProviderItemVersion,
        options: NSFileProviderDeleteItemOptions = [],
        request: NSFileProviderRequest,
        completionHandler: @escaping (Error?) -> Void
    ) -> Progress {
        let progress = Progress(totalUnitCount: 1)
        let once = Once()
        let finish: (Error?) -> Void = { error in
            guard once.claim() else { return }
            progress.completedUnitCount = 1
            completionHandler(error)
        }

        // Every delete goes to the phone: the Finder deletes on disk and the system reports it afterwards,
        // so a delete from the Finder cannot be told from any other process's. The phone keeps deleted
        // files in its recycle bin.
        HostConnection.shared.withHost({ finish($0) }) { [domainID, weak self] host in
            host.delete(domain: domainID, identifier: engineIdentifier(identifier)) { error in
                guard let error = providerError(error) else { return finish(nil) }

                // Unreachable: the system tries again later. Anything else stays on the phone.
                if (error as NSError).domain == NSFileProviderErrorDomain,
                   (error as NSError).code == NSFileProviderError.Code.serverUnreachable.rawValue {
                    return finish(error)
                }
                log.debug("item deletion refused; the item is put back")
                guard let self else { return finish(NSFileProviderError(.deletionRejected)) }
                self.rejectDeletion(of: identifier, finish)
            }
        }

        return progress
    }

    /// Rejects the deletion of `identifier` with its current item, which the system puts back.
    private func rejectDeletion(of identifier: NSFileProviderItemIdentifier, _ finish: @escaping (Error?) -> Void) {
        record(identifier) { item, error in
            guard let item else { return finish(error ?? NSFileProviderError(.deletionRejected)) }
            finish(NSError.fileProviderErrorForRejectedDeletion(of: item))
        }
    }

    func enumerator(for containerItemIdentifier: NSFileProviderItemIdentifier, request: NSFileProviderRequest) throws -> NSFileProviderEnumerator {
        FileProviderEnumerator(container: containerItemIdentifier, domain: domainID)
    }

    // MARK: - Service

    func supportedServiceSources(
        for itemIdentifier: NSFileProviderItemIdentifier,
        completionHandler: @escaping ([NSFileProviderServiceSource]?, Error?) -> Void
    ) -> Progress {
        completionHandler([ServiceSource()], nil)
        let progress = Progress(totalUnitCount: 1)
        progress.completedUnitCount = 1
        return progress
    }

    // MARK: - Thumbnails

    func fetchThumbnails(
        for itemIdentifiers: [NSFileProviderItemIdentifier],
        requestedSize size: CGSize,
        perThumbnailCompletionHandler: @escaping (NSFileProviderItemIdentifier, Data?, Error?) -> Void,
        completionHandler: @escaping (Error?) -> Void
    ) -> Progress {
        let progress = Progress(totalUnitCount: Int64(itemIdentifiers.count))
        let group = DispatchGroup()

        for identifier in itemIdentifiers {
            group.enter()
            let once = Once()
            let finish: (Data?, Error?) -> Void = { data, error in
                guard once.claim() else { return }
                perThumbnailCompletionHandler(identifier, data, error)
                progress.completedUnitCount += 1
                group.leave()
            }

            HostConnection.shared.withHost({ finish(nil, $0) }) { [domainID] host in
                host.thumbnail(domain: domainID, identifier: engineIdentifier(identifier)) { data, error in
                    finish(data, providerError(error))
                }
            }
        }

        group.notify(queue: .global()) { completionHandler(nil) }
        return progress
    }
}
