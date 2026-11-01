import AppKit
import CoreGraphics
import Foundation
import ImageIO
import PhoneLinkEngine
import UniformTypeIdentifiers

/// The Mac's desktop picture for the phone's request: the primary screen's picture
/// (`NSWorkspace.desktopImageURL(for:)` of the first screen, the one with the menu bar), its first
/// image decoded, scaled to the engine's size rule and encoded as JPEG. Read on every request: the
/// phone asks when it wants the picture, so nothing watches for changes. While macOS lists no screen
/// (every display asleep or disconnected) the read fails rather than reporting no picture.
public final class PcWallpaperAdapter: PcWallpaperSource {
    /// The JPEG quality the picture is encoded at.
    static let jpegQuality = 0.9

    private let desktopPictureURL: @Sendable () throws -> URL?

    /// `desktopPictureURL` stands in for the primary screen's picture in tests.
    public init(desktopPictureURL: @escaping @Sendable () throws -> URL? = PcWallpaperAdapter.primaryScreenPictureURL) {
        self.desktopPictureURL = desktopPictureURL
    }

    public func currentWallpaper() throws -> PcWallpaper {
        guard let url = try desktopPictureURL(), url.isFileURL else { return .notSet }
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory) else { return .notSet }
        guard !isDirectory.boolValue, let jpeg = try? Self.encode(fileAt: url) else { return .unreadable }
        return .picture(jpeg: jpeg)
    }

    /// The primary screen's desktop picture, read on the main thread (AppKit's screens belong to it).
    /// No screen listed is an error.
    public static func primaryScreenPictureURL() throws -> URL? {
        let read = { @MainActor () throws -> URL? in
            guard let screen = NSScreen.screens.first else {
                throw EdgeError.Unexpected(message: "macOS lists no screen")
            }
            return NSWorkspace.shared.desktopImageURL(for: screen)
        }

        if Thread.isMainThread {
            return try MainActor.assumeIsolated(read)
        }
        return try DispatchQueue.main.sync { try MainActor.assumeIsolated(read) }
    }

    /// The picture's first image, drawn opaque into sRGB at the engine's size and written as JPEG.
    static func encode(fileAt url: URL) throws -> Data {
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
              let image = CGImageSourceCreateImageAtIndex(source, 0, nil)
        else { throw EdgeError.Unexpected(message: "ImageIO decodes no image from the desktop picture") }

        let size = wallpaperScaledSize(width: UInt32(image.width), height: UInt32(image.height))
        guard size.width > 0, size.height > 0 else {
            throw EdgeError.Unexpected(message: "the desktop picture scales to \(size.width)×\(size.height)")
        }

        let context = try EdgeImageIO.context(
            width: Int(size.width),
            height: Int(size.height),
            space: EdgeImageIO.sRGB(),
            alpha: .noneSkipLast
        )
        context.interpolationQuality = .high
        context.draw(image, in: CGRect(x: 0, y: 0, width: Int(size.width), height: Int(size.height)))
        return try EdgeImageIO.encode(EdgeImageIO.makeImage(context), as: .jpeg, quality: jpegQuality)
    }
}
