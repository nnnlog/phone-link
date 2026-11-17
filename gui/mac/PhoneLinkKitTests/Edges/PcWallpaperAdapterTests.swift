import AppKit
import Foundation
import PhoneLinkEngine
import Testing
@testable import PhoneLinkKit

struct PcWallpaperAdapterTests {
    private static func file(_ data: Data, name: String) throws -> URL {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let url = dir.appendingPathComponent(name)
        try data.write(to: url)
        return url
    }

    private static func jpeg(_ wallpaper: PcWallpaper) -> Data? {
        if case let .picture(jpeg) = wallpaper { return jpeg }
        return nil
    }

    @Test func aTallPictureIsScaledTo512HighAndEncodedAsJpeg() throws {
        let url = try Self.file(EdgesTestImage.png(width: 1024, height: 2048), name: "tall.png")
        let wallpaper = try PcWallpaperAdapter(desktopPictureURL: { url }).currentWallpaper()

        let jpeg = try #require(Self.jpeg(wallpaper))
        #expect(EdgesTestImage.typeIdentifier(jpeg) == "public.jpeg")

        let size = try #require(EdgesTestImage.size(jpeg))
        #expect(size.width == 256 && size.height == 512)
    }

    @Test func aPictureUpTo512HighKeepsItsSize() throws {
        let url = try Self.file(EdgesTestImage.png(width: 300, height: 200, opaque: false), name: "small.png")
        let jpeg = try #require(Self.jpeg(try PcWallpaperAdapter(desktopPictureURL: { url }).currentWallpaper()))

        let size = try #require(EdgesTestImage.size(jpeg))
        #expect(size.width == 300 && size.height == 200)
        #expect(jpeg.starts(with: [0xFF, 0xD8, 0xFF]))
    }

    @Test func noPictureFileIsNotSetAndAnUndecodableOneIsUnreadable() throws {
        #expect(try PcWallpaperAdapter(desktopPictureURL: { nil }).currentWallpaper() == .notSet)
        let missing = FileManager.default.temporaryDirectory.appendingPathComponent("no-such-\(UUID().uuidString).heic")
        #expect(try PcWallpaperAdapter(desktopPictureURL: { missing }).currentWallpaper() == .notSet)
        #expect(try PcWallpaperAdapter(desktopPictureURL: { URL(string: "https://example.com/a.jpg")! }).currentWallpaper() == .notSet)

        let garbage = try Self.file(Data("not a picture".utf8), name: "garbage.jpg")
        #expect(try PcWallpaperAdapter(desktopPictureURL: { garbage }).currentWallpaper() == .unreadable)
        let folder = garbage.deletingLastPathComponent()
        #expect(try PcWallpaperAdapter(desktopPictureURL: { folder }).currentWallpaper() == .unreadable)
    }

    @Test func aScreenListThatCannotBeReadFailsTheRead() {
        let adapter = PcWallpaperAdapter(desktopPictureURL: { throw EdgeError.Unexpected(message: "no screen") })
        #expect(throws: EdgeError.self) { try adapter.currentWallpaper() }
    }

    @MainActor @Test func thisMacsDesktopPictureCanBeRead() throws {
        // On a Mac with a screen this reads its picture; without one (a headless runner) the read fails.
        guard !NSScreen.screens.isEmpty else {
            #expect(throws: EdgeError.self) { try PcWallpaperAdapter().currentWallpaper() }
            return
        }

        let wallpaper = try PcWallpaperAdapter().currentWallpaper()
        if let jpeg = Self.jpeg(wallpaper) {
            #expect(try #require(EdgesTestImage.size(jpeg)).height <= 512)
        }
    }
}
