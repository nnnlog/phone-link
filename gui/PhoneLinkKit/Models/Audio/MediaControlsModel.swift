import AppKit
import Observation
import os
import PhoneLinkEngine

private let logger = AppLog(category: "media-controls")

/// What plays on the phone and its play/pause, next and previous buttons, for the menu bar extra,
/// the main window and — when the user turns it on — macOS's Now Playing (Control Center and the
/// keyboard's media keys). Also the audio-controls setting of the selected phone.
///
/// Every value is the engine's: read when the engine starts and on every media-controls event.
@MainActor
@Observable
public final class MediaControlsModel {
    /// One media session on the phone.
    public struct Session: Identifiable, Equatable {
        public let id: Int64
        /// The media app's name; "Media" when the phone sent none.
        public let appName: String
        /// The title; the app's name when the phone sent none.
        public let title: String
        public let artist: String?
        /// The album art; `nil` → a placeholder.
        public let albumArt: NSImage?
        /// The app's notification icon as a template image the view tints; `nil` → a placeholder.
        public let appIcon: NSImage?
        public let playing: Bool

        init(_ view: MediaSessionView) {
            let appName = view.appName.flatMap(Self.nonEmpty) ?? String(localized: "Media")
            id = view.id
            self.appName = appName
            title = view.title.flatMap(Self.nonEmpty) ?? appName
            artist = view.artist.flatMap(Self.nonEmpty)
            albumArt = view.albumArt.flatMap { NSImage(data: $0) }
            appIcon = view.appIcon.flatMap(Self.silhouette)
            playing = view.playing
        }

        public init(
            id: Int64,
            appName: String,
            title: String,
            artist: String?,
            albumArt: NSImage?,
            appIcon: NSImage?,
            playing: Bool
        ) {
            self.id = id
            self.appName = appName
            self.title = title
            self.artist = artist
            self.albumArt = albumArt
            self.appIcon = appIcon
            self.playing = playing
        }

        private static func nonEmpty(_ s: String) -> String? {
            s.isEmpty ? nil : s
        }

        /// The phone sends the notification's small icon — a silhouette Android draws through its
        /// alpha alone — as a JPEG, which has no alpha: the glyph arrives light on black. Each pixel's
        /// brightest channel becomes the alpha of a black template image; `nil` for bytes that are no
        /// image.
        static func silhouette(_ data: Data) -> NSImage? {
            do {
                let image = try EdgeImageIO.decode(data)
                let context = try EdgeImageIO.context(
                    width: image.width,
                    height: image.height,
                    space: try EdgeImageIO.sRGB(),
                    alpha: .premultipliedLast
                )

                context.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
                guard let base = context.data else {
                    throw EdgeError.Unexpected(message: "no pixels in the \(image.width)×\(image.height) app icon context")
                }

                for row in 0..<context.height {
                    let pixels = (base + row * context.bytesPerRow).bindMemory(to: UInt8.self, capacity: context.width * 4)
                    for x in 0..<context.width {
                        let p = pixels + x * 4
                        p[3] = max(p[0], p[1], p[2])
                        p[0] = 0
                        p[1] = 0
                        p[2] = 0
                    }
                }

                let icon = NSImage(cgImage: try EdgeImageIO.makeImage(context), size: NSSize(width: image.width, height: image.height))
                icon.isTemplate = true
                return icon
            } catch {
                logger.notice("media app icon not decoded: \(error: error)")
                return nil
            }
        }

        /// The line under the title: the artist and the app.
        public var subtitle: String {
            [artist, appName].compactMap { $0 }.joined(separator: " · ")
        }
    }

    /// `nil` before the engine starts.
    public private(set) var state: MediaControlsState?
    public private(set) var sessions: [Session] = []
    /// The last change of the setting that failed; the view shows it and clears it.
    public var failure: SettingsFailure?

    /// Whether the phone's media also shows in macOS's Now Playing and answers the media keys (default
    /// off, kept in the app's preferences).
    public var usesNowPlaying: Bool {
        didSet {
            defaults.set(usesNowPlaying, forKey: PreferenceKeys.useNowPlaying)
            updateNowPlaying()
        }
    }

    private let api: any AudioApiProtocol
    @ObservationIgnored private let defaults: UserDefaults
    @ObservationIgnored private let nowPlaying: any NowPlayingSurface
    /// The account was removed or the phone left it: nothing is read or changed until the next start.
    @ObservationIgnored private var suspended = false

    public convenience init(api: any AudioApiProtocol, hub: EngineEventHub) {
        self.init(api: api, hub: hub, defaults: .standard, nowPlaying: SystemNowPlaying())
    }

    init(api: any AudioApiProtocol, hub: EngineEventHub, defaults: UserDefaults, nowPlaying: any NowPlayingSurface) {
        self.api = api
        self.defaults = defaults
        self.nowPlaying = nowPlaying
        usesNowPlaying = defaults.object(forKey: PreferenceKeys.useNowPlaying) as? Bool ?? false

        nowPlaying.onCommand = { [weak self] command in
            self?.perform(command)
        }
        hub.onEvent { [weak self] event in
            self?.handle(event)
        }
        hub.onStart { [weak self] in
            self?.suspended = false
            self?.reload()
        }
    }

    // MARK: - The controls

    /// The controls show: they are on, the notifications have synced, and the phone plays something.
    public var isShown: Bool {
        state?.shown == true && selected != nil
    }

    /// The session the buttons act on.
    public var selected: Session? {
        if let id = state?.selectedId, let s = sessions.first(where: { $0.id == id }) {
            return s
        }
        return sessions.first
    }

    /// Two or more sessions: the user picks which one the buttons act on.
    public var offersPicker: Bool {
        sessions.count > 1
    }

    /// A command was sent and the phone has not shown its effect yet.
    public var isCommanding: Bool {
        state?.commanding == true
    }

    public func playPause() {
        run("media play/pause not sent") { try api.playPause() }
    }

    public func next() {
        run("media next track not sent") { try api.next() }
    }

    public func previous() {
        run("media previous track not sent") { try api.previous() }
    }

    /// The picker: the buttons act on `id` from now on.
    public func select(_ id: Int64) {
        run("media session not selected") { try api.selectSession(id: id) }
    }

    // MARK: - The setting

    /// The phone supports media controls, so Settings offers the switch.
    public var settingOffered: Bool {
        state?.supported == true
    }

    /// The switch applies: the phone supports media controls and grants notification access.
    public var settingApplies: Bool {
        settingOffered && state?.notificationsPermission == true
    }

    /// "Show what plays on the phone": on only while it applies.
    public var showsControls: Bool {
        get { settingApplies && state?.setting == true }
        set { setSetting(newValue) }
    }

    /// The line under the switch.
    public var settingDetail: String {
        guard let state else { return String(localized: "Control what plays on your phone from this Mac.") }
        if !state.supported {
            return String(localized: "This phone doesn't support media controls.")
        }
        if !state.notificationsPermission {
            return String(localized: "This needs notification access on your phone. Request it in Settings › Notifications.")
        }
        return String(localized: "Play, pause and skip what plays on your phone from the menu bar and the Phone Link window.")
    }

    public func reload() {
        guard !suspended else { return }
        do {
            let state = try api.controls()
            self.state = state
            sessions = state.sessions.map(Session.init)
        } catch let error as FeatureApiError where error == .NotStarted || error == .NoPhone {
            clear()
        } catch {
            logger.error("media controls not read: \(error: error)")
        }

        updateNowPlaying()
    }

    private func setSetting(_ on: Bool) {
        guard !suspended else { return }
        do {
            try api.setSetting(on: on)
        } catch let error as FeatureApiError where error == .NotStarted {
            clear()
            return
        } catch {
            logger.error("media controls setting not changed: \(error: error)")
            failure = SettingsFailure(title: String(localized: "Couldn't Change the Setting"), message: UserText.describe(error))
        }

        reload()
    }

    private func run(_ subject: StaticString, _ call: () throws -> Void) {
        guard !suspended else { return }
        do {
            try call()
        } catch let error as FeatureApiError where error == .NotStarted {
            logger.debug("media command dropped; the engine has not started")
        } catch {
            logger.error("\(subject.description, privacy: .public): \(error: error)")
        }

        reload()
    }

    private func clear() {
        state = nil
        sessions = []
        updateNowPlaying()
    }

    private func handle(_ event: EngineEvent) {
        switch event {
        case .accountRemoved, .phoneUnlinked:
            suspended = true
            clear()
        case .audio, .resync(feature: .audio), .phoneChanged:
            reload()
        default:
            break
        }
    }

    // MARK: - Now Playing

    private func updateNowPlaying() {
        guard usesNowPlaying, isShown, let session = selected else {
            nowPlaying.clear()
            return
        }
        nowPlaying.show(NowPlayingItem(
            title: session.title,
            artist: session.artist,
            appName: session.appName,
            artwork: session.albumArt,
            playing: session.playing
        ))
    }

    private func perform(_ command: NowPlayingCommand) {
        guard let session = selected else { return }
        switch command {
        case .togglePlayPause:
            playPause()
        case .play where !session.playing, .pause where session.playing:
            playPause()
        case .play, .pause:
            break
        case .next:
            next()
        case .previous:
            previous()
        }
    }
}
