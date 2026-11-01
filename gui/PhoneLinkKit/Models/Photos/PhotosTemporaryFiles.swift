import Foundation
import ImageIO
import os
import PhoneLinkEngine
import UniformTypeIdentifiers

private let log = AppLog(category: "photos")

/// The photo files Phone Link hands to Quick Look and to other apps (open, copy, share, drag-out).
///
/// They live in a folder of this run under the temporary directory: a file action's file in the
/// photo's folder, one per save-as name — a later action replaces it atomically, so an app still using
/// it keeps a valid path — and Quick Look's files apart under `viewer/`, named after the image type of
/// their bytes rather than the phone's extension, so removing them when Quick Look moves on leaves an
/// app's copy alone. The model removes a photo's files when it leaves the gallery or changes, and
/// Quick Look's when it shows another photo or closes; the rest go when the app quits. A run that did
/// not quit cleanly leaves its folder behind, so a launch removes the folders of earlier runs.
///
/// Disk work runs on one serial queue off the main thread, in the order it was asked for, so a
/// removal and a later write of the same file never swap.
final class PhotosTemporaryFiles: Sendable {
    /// This run's folder, created by the first write.
    let folder: URL
    private let queue = DispatchQueue(label: "\(PhoneLinkIdentity.bundleID).photos.files", qos: .userInitiated)

    init(base: URL) {
        let root = base.appendingPathComponent("\(PhoneLinkIdentity.bundleID).photos", isDirectory: true)
        let fileManager = FileManager.default
        do {
            for earlier in try fileManager.contentsOfDirectory(at: root, includingPropertiesForKeys: nil) {
                try fileManager.removeItem(at: earlier)
            }
        } catch let error as CocoaError where error.code == .fileReadNoSuchFile {
            // No run has written photo files yet.
        } catch {
            log.notice("earlier runs' photo files not removed: \(error: error)")
        }

        folder = root.appendingPathComponent(UUID().uuidString, isDirectory: true)
    }

    /// Runs `work` on the queue, after the work asked for before it. The writes below run in it.
    func perform<T: Sendable>(_ work: @escaping @Sendable () throws -> T) async throws -> T {
        try await withCheckedThrowingContinuation { continuation in
            queue.async {
                continuation.resume(with: Result(catching: work))
            }
        }
    }

    /// Writes a photo's full image for a file action under its save-as name, the name every other app
    /// shows for it.
    func writeFullImage(_ data: Data, id: Int64, name: PhotoFileName) throws -> URL {
        try write(data, to: itemFolder(id).appendingPathComponent(Self.fileName(name)))
    }

    /// Writes the full image Quick Look shows, named after its bytes (`viewerFileName`); `nil`, writing
    /// nothing, when they are no photo.
    func writeViewerImage(_ data: Data, id: Int64, name: PhotoFileName) throws -> URL? {
        guard let fileName = try Self.viewerFileName(of: data, name: name) else { return nil }
        return try write(data, to: viewerFolder(id).appendingPathComponent(fileName))
    }

    /// Writes the thumbnail Quick Look shows until the full image arrives, named after its bytes
    /// (`viewerFileName`) — its format need not be the photo's; `nil`, writing nothing, when they are
    /// no photo.
    func writeThumbnail(_ data: Data, id: Int64, name: PhotoFileName) throws -> URL? {
        guard let fileName = try Self.viewerFileName(of: data, name: name) else { return nil }
        let folder = viewerFolder(id).appendingPathComponent("thumbnail", isDirectory: true)
        return try write(data, to: folder.appendingPathComponent(fileName))
    }

    /// The name of a Quick Look file of `data`: the photo's stem with the extension of the image type
    /// ImageIO detects in the bytes — the save-as extension when it names that same type, the type's
    /// own otherwise — never the phone's extension alone. Quick Look's own Share and Open With hand the
    /// file on, so it opens as what it holds whatever the phone named the photo. `nil` when the bytes
    /// are no photo: no type ImageIO detects, or one that is no image or is SVG (`PhotosModel.isPhoto`),
    /// such as a PDF.
    static func viewerFileName(of data: Data, name: PhotoFileName) throws -> String? {
        guard let type = PhotosImageDecoder.type(of: data), PhotosModel.isPhoto(type) else { return nil }
        let named = String(name.extension.dropFirst())

        let ext: String
        if UTType(filenameExtension: named) == type {
            ext = named
        } else if let preferred = type.preferredFilenameExtension {
            ext = preferred
        } else {
            return nil
        }

        return try fileName(PhotoFileName(stem: name.stem, extension: "." + ext))
    }

    /// Removes the files Quick Look showed of a photo, once the work asked for before is done.
    func removeViewerFiles(of id: Int64) {
        queue.async { [self] in
            remove(viewerFolder(id), "quick look's photo files not removed")
        }
    }

    /// Removes every file of a photo (it left the gallery, or changed), once the work asked for
    /// before is done.
    func removeFiles(of id: Int64) {
        queue.async { [self] in
            remove(itemFolder(id), "photo's files not removed")
            remove(viewerFolder(id), "quick look's photo files not removed")
        }
    }

    /// Removes this run's files (the app quits), waiting for the work under way.
    func removeAll() {
        queue.sync {
            remove(folder, "this run's photo files not removed")
        }
    }

    /// The save-as name as one file name. The engine takes the stem and extension from the phone's
    /// file name, so they hold no path separator; a name that is not a single file name is refused.
    static func fileName(_ name: PhotoFileName) throws -> String {
        let fileName = name.stem + name.extension
        guard !fileName.isEmpty, fileName != ".", fileName != "..", !fileName.contains("/"), !fileName.contains("\0")
        else {
            throw PhotosUnusableFileName(name: fileName)
        }
        return fileName
    }

    /// A file action's files of the photo. Ids are numbers, so no photo's folder is `viewer`.
    func itemFolder(_ id: Int64) -> URL {
        folder.appendingPathComponent(String(id), isDirectory: true)
    }

    /// Quick Look's files of the photo.
    func viewerFolder(_ id: Int64) -> URL {
        folder.appendingPathComponent("viewer", isDirectory: true).appendingPathComponent(String(id), isDirectory: true)
    }

    private func write(_ data: Data, to url: URL) throws -> URL {
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try data.write(to: url, options: .atomic)
        return url
    }

    private func remove(_ url: URL, _ subject: StaticString) {
        do {
            try FileManager.default.removeItem(at: url)
        } catch let error as CocoaError where error.code == .fileNoSuchFile {
            // Nothing was written there.
        } catch {
            log.notice("\(subject.description, privacy: .public): \(error: error)")
        }
    }
}

/// The phone named a photo in a way no file can be named.
struct PhotosUnusableFileName: LocalizedError {
    let name: String

    var errorDescription: String? {
        String(localized: "The photo's name “\(name)” can't be used as a file name.")
    }
}

/// The phone's file name names a type other than a photo or video; Phone Link hands no such file to
/// an app, whose handler it would run.
struct PhotosNotPhotoOrVideo: LocalizedError {
    let name: String

    var errorDescription: String? {
        String(localized: "“\(name)” isn't a photo or video file.")
    }
}

/// Image decoding for the grid's cells and Quick Look's stand-in, safe to run off the main thread.
enum PhotosImageDecoder {
    /// The bytes as an upright image at most `maxPixelSize` pixels on its longer side, decoded now
    /// rather than when a cell first draws it; `nil` when ImageIO cannot read them.
    static func thumbnail(of data: Data, maxPixelSize: Int) -> CGImage? {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil) else { return nil }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: maxPixelSize,
            kCGImageSourceShouldCacheImmediately: true,
        ]
        return CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary)
    }

    /// The image format ImageIO recognises in the bytes.
    static func type(of data: Data) -> UTType? {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil),
              let identifier = CGImageSourceGetType(source)
        else {
            return nil
        }
        return UTType(identifier as String)
    }
}

/// Counts Quick Look's file requests, shared with the file queue: a request still queued when a newer
/// one came skips its work, so flipping through photos writes only the one that stays.
final class PhotosRequestCounter: Sendable {
    private let newest = OSAllocatedUnfairLock(initialState: 0)

    /// A new request; every earlier one is outdated from now on.
    func start() -> Int {
        newest.withLock { value in
            value += 1
            return value
        }
    }

    /// Outdates every request (Quick Look closed).
    func outdateAll() {
        newest.withLock { $0 += 1 }
    }

    func isNewest(_ request: Int) -> Bool {
        newest.withLock { $0 == request }
    }
}
