import AppKit
import Foundation
import Testing
import PhoneLinkEngine
@testable import PhoneLinkKit

@MainActor
@Suite struct WallpaperModelTests {
    private let api = WallpaperApiFake()
    private let hub = EngineEventHub()
    private let defaults = WallpaperTestDefaults()
    private let transparency = TransparencySetting()

    private func model() -> WallpaperModel {
        let transparency = transparency
        return WallpaperModel(api: api, hub: hub, defaults: defaults, reducesTransparency: { transparency.reduced })
    }

    private static let picture = EdgesTestImage.jpeg(width: 40, height: 80)

    @Test func theWallpaperLoadsOnStartAndOnItsEvents() {
        let model = model()
        #expect(model.image == nil)
        api.result = .success(PhoneWallpaper(image: Self.picture, origin: .phoneLink))
        hub.postStarted()
        #expect(model.image != nil)

        api.result = .success(nil)
        hub.post(.wallpaper(change: .changed))
        #expect(model.image == nil)

        api.result = .success(PhoneWallpaper(image: Self.picture, origin: .crossDevice))
        hub.post(.resync(feature: .photos))
        #expect(model.image == nil, "another feature's resync")
        hub.post(.resync(feature: .wallpaper))
        #expect(model.image != nil)

        api.result = .failure(.NotStarted)
        hub.post(.accountRemoved)
        #expect(model.image == nil)
    }

    @Test func aFailedReadKeepsTheLastImageAndBytesThatDoNotDecodeAreNone() {
        let model = model()
        api.result = .success(PhoneWallpaper(image: Self.picture, origin: .phoneLink))
        hub.postStarted()

        api.result = .failure(.Store(message: "disk"))
        hub.post(.wallpaper(change: .changed))
        #expect(model.image != nil)

        api.result = .success(PhoneWallpaper(image: Data("garbage".utf8), origin: .phoneLink))
        hub.post(.phoneChanged)
        #expect(model.image == nil)
    }

    @Test func theBackgroundNeedsTheSettingTransparencyAndAnImage() {
        let model = model()
        #expect(model.usesAsBackground, "on by default")
        #expect(!model.showsBackground, "no image yet")
        api.result = .success(PhoneWallpaper(image: Self.picture, origin: .phoneLink))
        model.reload()
        #expect(model.showsBackground)

        model.usesAsBackground = false
        #expect(!model.showsBackground)
        #expect(defaults.object(forKey: PreferenceKeys.useWallpaperAsBackground) as? Bool == false)
        #expect(!self.model().usesAsBackground, "the setting is kept")

        model.usesAsBackground = true
        transparency.reduced = true
        model.accessibilityChanged()
        #expect(model.transparencyReduced)
        #expect(!model.showsBackground)
    }
}

@MainActor
private final class TransparencySetting {
    var reduced = false
}

private final class WallpaperApiFake: WallpaperApiProtocol, @unchecked Sendable {
    private let lock = NSLock()
    private var stored: Result<PhoneWallpaper?, FeatureApiError> = .failure(.NotStarted)

    var result: Result<PhoneWallpaper?, FeatureApiError> {
        get { lock.withLock { stored } }
        set { lock.withLock { stored = newValue } }
    }

    func wallpaper() throws -> PhoneWallpaper? {
        try result.get()
    }
}

/// A defaults domain that lives only in memory.
private final class WallpaperTestDefaults: UserDefaults, @unchecked Sendable {
    private var values: [String: Any] = [:]

    init() {
        super.init(suiteName: nil)!
    }

    override func object(forKey defaultName: String) -> Any? {
        values[defaultName]
    }

    override func set(_ value: Any?, forKey defaultName: String) {
        values[defaultName] = value
    }

    override func set(_ value: Bool, forKey defaultName: String) {
        values[defaultName] = value
    }
}
