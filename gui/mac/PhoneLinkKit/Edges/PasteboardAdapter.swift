import AppKit
import os
import PhoneLinkEngine

/// The engine's `ClipboardPasteboard`: the change count, types, data and one write of a pasteboard,
/// the general one in the app. Every rule about what the content means is engine code.
///
/// The engine polls from its own threads. These `NSPasteboard` calls need no main thread, and the
/// pasteboard is reachable only through the lock, so the adapter's own calls never overlap.
public final class PasteboardAdapter: ClipboardPasteboard {
    /// Writes one representation: `NSPasteboard.setData(_:forType:)`, whose `false` is the
    /// pasteboard refusing it (a test stands in a pasteboard that refuses one).
    typealias SetData = @Sendable (NSPasteboard, Data, NSPasteboard.PasteboardType) -> Bool

    private let pasteboard: OSAllocatedUnfairLock<NSPasteboard>
    private let setData: SetData

    public convenience init(pasteboard: NSPasteboard = .general) {
        self.init(pasteboard: pasteboard) { $0.setData($1, forType: $2) }
    }

    init(pasteboard: NSPasteboard, setData: @escaping SetData) {
        self.pasteboard = OSAllocatedUnfairLock(uncheckedState: pasteboard)
        self.setData = setData
    }

    public func changeCount() throws -> Int64 {
        pasteboard.withLockUnchecked { Int64($0.changeCount) }
    }

    public func types() throws -> [String] {
        pasteboard.withLockUnchecked { ($0.types ?? []).map(\.rawValue) }
    }

    public func data(uti: String) throws -> Data? {
        pasteboard.withLockUnchecked { $0.data(forType: NSPasteboard.PasteboardType(uti)) }
    }

    /// One item holding every representation, stopping at the first one the pasteboard refuses. The
    /// change count `clearContents()` returned is the count once the write ended, since `setData`
    /// does not change it — also when a representation was refused after it, so the engine can
    /// still tell the write was its own.
    public func write(representations: [ClipboardPasteboardData]) throws -> ClipboardPasteboardWrite {
        pasteboard.withLockUnchecked { pasteboard in
            let changeCount = Int64(pasteboard.clearContents())
            for representation in representations {
                guard setData(pasteboard, representation.data, NSPasteboard.PasteboardType(representation.uti)) else {
                    return ClipboardPasteboardWrite(changeCount: changeCount, failure: "the pasteboard did not take \(representation.uti)")
                }
            }
            return ClipboardPasteboardWrite(changeCount: changeCount, failure: nil)
        }
    }
}
