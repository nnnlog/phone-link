import FileProvider

/// A folder's entries, or the working set's changes. Every change of the tree is reported through
/// the working set, whose sync anchor is the engine's generation; a folder's own enumerator lists its
/// entries and reports no changes of its own.
final class FileProviderEnumerator: NSObject, NSFileProviderEnumerator {
    private let container: NSFileProviderItemIdentifier
    private let domain: String

    init(container: NSFileProviderItemIdentifier, domain: String) {
        self.container = container
        self.domain = domain
    }

    func invalidate() {}

    func enumerateItems(for observer: NSFileProviderEnumerationObserver, startingAt page: NSFileProviderPage) {
        if container == .workingSet || container == .trashContainer {
            observer.finishEnumerating(upTo: nil)
            return
        }

        let once = Once()
        let fail: (Error) -> Void = { error in
            if once.claim() { observer.finishEnumeratingWithError(error) }
        }
        HostConnection.shared.withHost(fail) { [domain, container] host in
            host.children(domain: domain, identifier: engineIdentifier(container)) { data, error in
                guard once.claim() else { return }
                if let error = providerError(error) {
                    return observer.finishEnumeratingWithError(error)
                }
                guard let data, let records = FileProviderItem.decodeList([PhoneFileRecord].self, data) else {
                    return observer.finishEnumeratingWithError(NSFileProviderError(.cannotSynchronize))
                }

                observer.didEnumerate(records.map { FileProviderItem($0) })
                observer.finishEnumerating(upTo: nil)
            }
        }
    }

    func enumerateChanges(for observer: NSFileProviderChangeObserver, from anchor: NSFileProviderSyncAnchor) {
        let once = Once()
        let fail: (Error) -> Void = { error in
            if once.claim() { observer.finishEnumeratingWithError(error) }
        }

        guard container == .workingSet else {
            currentSyncAnchor { current in
                guard once.claim() else { return }
                observer.finishEnumeratingChanges(upTo: current ?? anchor, moreComing: false)
            }
            return
        }
        guard let since = Self.generation(anchor) else {
            return fail(NSFileProviderError(.syncAnchorExpired))
        }

        HostConnection.shared.withHost(fail) { [domain] host in
            host.changes(domain: domain, since: since) { data, expired, error in
                guard once.claim() else { return }
                if let error = providerError(error) {
                    return observer.finishEnumeratingWithError(error)
                }
                if expired {
                    return observer.finishEnumeratingWithError(NSFileProviderError(.syncAnchorExpired))
                }
                guard let data, let changes = FileProviderItem.decodeList(PhoneFileChangeSet.self, data) else {
                    return observer.finishEnumeratingWithError(NSFileProviderError(.cannotSynchronize))
                }

                if !changes.updated.isEmpty {
                    observer.didUpdate(changes.updated.map { FileProviderItem($0) })
                }
                if !changes.removed.isEmpty {
                    observer.didDeleteItems(withIdentifiers: changes.removed.map { NSFileProviderItemIdentifier($0) })
                }
                observer.finishEnumeratingChanges(upTo: Self.anchor(changes.generation), moreComing: false)
            }
        }
    }

    func currentSyncAnchor(completionHandler: @escaping (NSFileProviderSyncAnchor?) -> Void) {
        let once = Once()
        HostConnection.shared.withHost({ _ in if once.claim() { completionHandler(nil) } }) { [domain] host in
            host.generation(domain: domain) { generation, error in
                guard once.claim() else { return }
                completionHandler(error == nil ? Self.anchor(generation) : nil)
            }
        }
    }

    static func anchor(_ generation: Int64) -> NSFileProviderSyncAnchor {
        NSFileProviderSyncAnchor(Data(String(generation).utf8))
    }

    static func generation(_ anchor: NSFileProviderSyncAnchor) -> Int64? {
        Int64(String(decoding: anchor.rawValue, as: UTF8.self))
    }
}
