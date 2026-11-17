import AppKit
import Foundation
import Testing
import PhoneLinkEngine
@testable import PhoneLinkKit

@MainActor
@Suite struct MediaControlsModelTests {
    private let api = AudioApiFake()
    private let hub = EngineEventHub()
    private let defaults = MediaTestDefaults()
    private let nowPlaying = NowPlayingFake()

    private func model() -> MediaControlsModel {
        MediaControlsModel(api: api, hub: hub, defaults: defaults, nowPlaying: nowPlaying)
    }

    private static let art = EdgesTestImage.jpeg(width: 20, height: 20)

    private static func session(_ id: Int64, playing: Bool, app: String? = "Player", title: String? = "Song") -> MediaSessionView {
        MediaSessionView(id: id, appName: app, title: title, artist: "Artist", albumArt: art, appIcon: nil, playing: playing)
    }

    private static func state(
        _ sessions: [MediaSessionView],
        selected: Int64? = nil,
        shown: Bool = true,
        setting: Bool = true,
        permission: Bool = true,
        commanding: Bool = false
    ) -> MediaControlsState {
        MediaControlsState(
            supported: true,
            setting: setting,
            notificationsPermission: permission,
            shown: shown,
            sessions: sessions,
            selectedId: selected ?? sessions.first?.id,
            commanding: commanding
        )
    }

    /// The phone's small icon is a light glyph on black (its alpha lost to JPEG): the glyph stays, the
    /// black goes.
    @Test func theAppIconIsTheSmallIconsGlyphAsATemplate() throws {
        let space = CGColorSpace(name: CGColorSpace.sRGB)!
        let context = CGContext(data: nil, width: 16, height: 16, bitsPerComponent: 8, bytesPerRow: 0, space: space,
                                bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!

        context.setFillColor(CGColor(red: 0, green: 0, blue: 0, alpha: 1))
        context.fill(CGRect(x: 0, y: 0, width: 16, height: 16))
        context.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
        context.fill(CGRect(x: 0, y: 8, width: 16, height: 8))

        let jpeg = EdgesTestImage.encode(context.makeImage()!, as: .jpeg, properties: [kCGImageDestinationLossyCompressionQuality: 0.97])

        var view = Self.session(1, playing: true)
        view.appIcon = jpeg
        let icon = try #require(MediaControlsModel.Session(view).appIcon)
        #expect(icon.isTemplate)

        let image = try #require(icon.cgImage(forProposedRect: nil, context: nil, hints: nil))
        #expect(EdgesTestImage.pixel(of: image, x: 8, y: 2)[3] > 240, "the glyph")
        #expect(EdgesTestImage.pixel(of: image, x: 8, y: 13)[3] < 15, "the black around it")

        view.appIcon = Data([1, 2, 3])
        #expect(MediaControlsModel.Session(view).appIcon == nil)
    }

    @Test func theControlsLoadOnStartAndOnTheirEvents() {
        let model = model()
        #expect(!model.isShown)

        api.result = .success(Self.state([Self.session(1, playing: true)]))
        hub.postStarted()
        #expect(model.isShown)
        let s = try! #require(model.selected)
        #expect(s.title == "Song" && s.subtitle == "Artist · Player" && s.playing && s.albumArt != nil)
        #expect(!model.offersPicker)

        api.result = .success(Self.state([Self.session(1, playing: false), Self.session(2, playing: false, app: nil, title: nil)], selected: 2))
        hub.post(.resync(feature: .photos))
        #expect(model.selected?.id == 1, "another feature's resync")

        hub.post(.audio(change: .changed))
        #expect(model.offersPicker)
        #expect(model.selected?.id == 2)
        #expect(model.selected?.appName == "Media" && model.selected?.title == "Media", "names fall back")

        api.result = .success(Self.state([], shown: false))
        hub.post(.resync(feature: .audio))
        #expect(!model.isShown && model.selected == nil)

        hub.post(.accountRemoved)
        api.result = .success(Self.state([Self.session(1, playing: true)]))
        hub.post(.audio(change: .changed))
        #expect(!model.isShown, "suspended until the next start")

        hub.postStarted()
        #expect(model.isShown)
    }

    @Test func buttonsAndThePickerCallTheEngineAndReread() {
        let model = model()
        api.result = .success(Self.state([Self.session(1, playing: true), Self.session(2, playing: false)]))
        model.reload()

        model.playPause()
        model.next()
        model.previous()
        model.select(2)

        #expect(api.calls == ["playPause", "next", "previous", "select 2"])
        #expect(api.reads == 5, "every call re-reads")
    }

    @Test func theSettingAppliesOnlyWithNotificationAccess() {
        let model = model()
        #expect(!model.settingOffered && !model.showsControls)

        api.result = .success(Self.state([], shown: false, permission: false))
        model.reload()
        #expect(model.settingOffered && !model.settingApplies && !model.showsControls)
        #expect(model.settingDetail.contains("notification access"))

        api.result = .success(Self.state([], shown: false))
        model.reload()
        #expect(model.showsControls)

        model.showsControls = false
        #expect(api.calls == ["setting false"])

        api.setFailure = .Store(message: "disk")
        model.showsControls = true
        #expect(model.failure != nil)
    }

    @Test func nowPlayingIsOptInAndFollowsTheSelectedSession() {
        let model = model()
        api.result = .success(Self.state([Self.session(1, playing: true)]))
        model.reload()
        #expect(!model.usesNowPlaying, "off by default")
        #expect(nowPlaying.shown == nil)

        model.usesNowPlaying = true
        #expect(defaults.object(forKey: PreferenceKeys.useNowPlaying) as? Bool == true)
        #expect(nowPlaying.shown?.title == "Song" && nowPlaying.shown?.playing == true)
        #expect(nowPlaying.shown?.artist == "Artist" && nowPlaying.shown?.appName == "Player")

        // Media keys: pause only while playing, play only while paused.
        nowPlaying.onCommand?(.play)
        #expect(api.calls.isEmpty)

        nowPlaying.onCommand?(.pause)
        nowPlaying.onCommand?(.togglePlayPause)
        nowPlaying.onCommand?(.next)
        nowPlaying.onCommand?(.previous)
        #expect(api.calls == ["playPause", "playPause", "next", "previous"])

        api.result = .success(Self.state([], shown: false))
        hub.post(.audio(change: .changed))
        #expect(nowPlaying.shown == nil && nowPlaying.clears > 0)

        api.result = .success(Self.state([Self.session(1, playing: false)]))
        hub.post(.audio(change: .changed))
        #expect(nowPlaying.shown?.playing == false)

        model.usesNowPlaying = false
        #expect(nowPlaying.shown == nil)
        #expect(!self.model().usesNowPlaying, "the setting is kept")
    }
}

private final class AudioApiFake: AudioApiProtocol, @unchecked Sendable {
    private let lock = NSLock()
    private var stored: Result<MediaControlsState, FeatureApiError> = .failure(.NotStarted)
    private var recorded: [String] = []
    private var readCount = 0
    private var failure: FeatureApiError?

    var result: Result<MediaControlsState, FeatureApiError> {
        get { lock.withLock { stored } }
        set { lock.withLock { stored = newValue } }
    }

    var setFailure: FeatureApiError? {
        get { lock.withLock { failure } }
        set { lock.withLock { failure = newValue } }
    }

    var calls: [String] { lock.withLock { recorded } }
    var reads: Int { lock.withLock { readCount } }

    private func record(_ call: String) {
        lock.withLock { recorded.append(call) }
    }

    func controls() throws -> MediaControlsState {
        lock.withLock { readCount += 1 }
        return try result.get()
    }

    func playPause() throws { record("playPause") }
    func next() throws { record("next") }
    func previous() throws { record("previous") }
    func selectSession(id: Int64) throws { record("select \(id)") }

    func setSetting(on: Bool) throws {
        if let failure = setFailure { throw failure }
        record("setting \(on)")
    }
}

@MainActor
private final class NowPlayingFake: NowPlayingSurface {
    var onCommand: (@MainActor (NowPlayingCommand) -> Void)?
    var shown: NowPlayingItem?
    var clears = 0

    func show(_ item: NowPlayingItem) {
        shown = item
    }

    func clear() {
        shown = nil
        clears += 1
    }
}

/// A defaults domain that lives only in memory.
private final class MediaTestDefaults: UserDefaults, @unchecked Sendable {
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

/// MediaPlayer calls the artwork and command closures on its own queues; the app crashed when they were
/// main-actor isolated.
@Suite struct SystemNowPlayingTests {
    @Test func theArtworkIsReadOffTheMainActor() async {
        let image = NSImage(data: EdgesTestImage.jpeg(width: 20, height: 20))!
        let artwork = SystemNowPlaying.artwork(image)
        let size = await Task.detached { artwork.image(at: CGSize(width: 10, height: 10))?.size }.value
        #expect(size == image.size)
    }

    @Test @MainActor func aCommandFromAnotherQueueReachesTheMainActor() async {
        let surface = SystemNowPlaying()
        var received: [NowPlayingCommand] = []
        surface.onCommand = { received.append($0) }

        let deliver = SystemNowPlaying.delivery(of: .next, to: surface)
        await Task.detached { deliver() }.value
        for _ in 0..<20 where received.isEmpty {
            await Task.yield()
        }
        #expect(received == [.next])
    }
}
