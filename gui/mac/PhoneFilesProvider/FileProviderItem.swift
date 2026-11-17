import FileProvider
import UniformTypeIdentifiers

/// An entry of the phone's tree as the Finder sees it. Files can be read, renamed, moved, trashed and
/// deleted, but not written: an edit has no way back to the phone. A folder the tree adds between the
/// root and a phone item can only be read and added to; the phone has no entry for it to rename.
final class FileProviderItem: NSObject, NSFileProviderItem {
    let record: PhoneFileRecord
    /// The user moved it to the Trash; it is still on the phone.
    let inTrash: Bool

    init(_ record: PhoneFileRecord, inTrash: Bool = false) {
        self.record = record
        self.inTrash = inTrash
    }

    static func decode(_ data: Data?) -> FileProviderItem? {
        guard let data else { return nil }
        do {
            return FileProviderItem(try JSONDecoder().decode(PhoneFileRecord.self, from: data))
        } catch {
            log.error("item from the app not decoded: \(String(describing: error), privacy: .public)")
            return nil
        }
    }

    /// The records the app sent; `nil` when they do not decode.
    static func decodeList<T: Decodable>(_ type: T.Type, _ data: Data) -> T? {
        do {
            return try JSONDecoder().decode(type, from: data)
        } catch {
            log.error("records from the app not decoded: \(String(describing: error), privacy: .public)")
            return nil
        }
    }

    var itemIdentifier: NSFileProviderItemIdentifier {
        record.identifier.isEmpty ? .rootContainer : NSFileProviderItemIdentifier(record.identifier)
    }

    var parentItemIdentifier: NSFileProviderItemIdentifier {
        if inTrash { return .trashContainer }
        return record.parent.isEmpty ? .rootContainer : NSFileProviderItemIdentifier(record.parent)
    }

    /// The root has no name of its own, but the system requires one for every item (it shows the
    /// domain's display name instead).
    var filename: String { record.name.isEmpty ? "Phone" : record.name }

    var contentType: UTType {
        if record.isDirectory { return .folder }
        let ext = (record.name as NSString).pathExtension
        return UTType(filenameExtension: ext) ?? .data
    }

    var capabilities: NSFileProviderItemCapabilities {
        if record.isDirectory {
            if record.synthetic || record.identifier.isEmpty {
                return [.allowsReading, .allowsContentEnumerating, .allowsAddingSubItems]
            }
            return [.allowsReading, .allowsContentEnumerating, .allowsAddingSubItems, .allowsRenaming, .allowsReparenting, .allowsTrashing, .allowsDeleting]
        }
        return [.allowsReading, .allowsRenaming, .allowsReparenting, .allowsTrashing, .allowsDeleting]
    }

    var documentSize: NSNumber? { record.isDirectory ? nil : NSNumber(value: record.size) }

    var creationDate: Date? { record.created.map { Date(timeIntervalSince1970: TimeInterval($0)) } }

    var contentModificationDate: Date? { record.modified.map { Date(timeIntervalSince1970: TimeInterval($0)) } }

    var itemVersion: NSFileProviderItemVersion {
        NSFileProviderItemVersion(
            contentVersion: Data(record.contentVersion.utf8),
            metadataVersion: Data((record.metadataVersion + (inTrash ? "#trash" : "")).utf8)
        )
    }
}
