import AppKit
import MediaPlayer

/// What macOS's Now Playing shows for the phone's selected media session.
public struct NowPlayingItem: Equatable {
    public let title: String
    public let artist: String?
    public let appName: String
    public let artwork: NSImage?
    public let playing: Bool
}

/// A media key, or a button of Control Center's Now Playing.
public enum NowPlayingCommand: Equatable, Sendable {
    case togglePlayPause
    case play
    case pause
    case next
    case previous
}

/// macOS's Now Playing as the media controls use it.
@MainActor
protocol NowPlayingSurface: AnyObject {
    /// Called with every command while an item shows.
    var onCommand: (@MainActor (NowPlayingCommand) -> Void)? { get set }
    /// Shows `item` and takes the media keys.
    func show(_ item: NowPlayingItem)
    /// Shows nothing and gives the media keys back.
    func clear()
}

/// Now Playing through `MPNowPlayingInfoCenter` and `MPRemoteCommandCenter`. Only play, pause, next
/// and previous are offered: the phone has no seek, position or rating for its media.
@MainActor
final class SystemNowPlaying: NowPlayingSurface {
    var onCommand: (@MainActor (NowPlayingCommand) -> Void)?

    private var targets: [(MPRemoteCommand, Any)] = []
    private var shown: NowPlayingItem?

    func show(_ item: NowPlayingItem) {
        guard item != shown else { return }
        if targets.isEmpty {
            register()
        }

        shown = item
        var info: [String: Any] = [
            MPMediaItemPropertyTitle: item.title,
            MPMediaItemPropertyAlbumTitle: item.appName,
            MPNowPlayingInfoPropertyMediaType: MPNowPlayingInfoMediaType.audio.rawValue,
            MPNowPlayingInfoPropertyPlaybackRate: item.playing ? 1.0 : 0.0,
        ]
        if let artist = item.artist {
            info[MPMediaItemPropertyArtist] = artist
        }
        if let artwork = item.artwork {
            info[MPMediaItemPropertyArtwork] = Self.artwork(artwork)
        }

        let center = MPNowPlayingInfoCenter.default()
        center.nowPlayingInfo = info
        center.playbackState = item.playing ? .playing : .paused
    }

    func clear() {
        guard shown != nil || !targets.isEmpty else { return }
        shown = nil

        let center = MPNowPlayingInfoCenter.default()
        center.nowPlayingInfo = nil
        center.playbackState = .stopped

        for (command, target) in targets {
            command.removeTarget(target)
            command.isEnabled = false
        }
        targets = []
    }

    private func register() {
        let commands = MPRemoteCommandCenter.shared()
        let handled: [(MPRemoteCommand, NowPlayingCommand)] = [
            (commands.togglePlayPauseCommand, .togglePlayPause),
            (commands.playCommand, .play),
            (commands.pauseCommand, .pause),
            (commands.nextTrackCommand, .next),
            (commands.previousTrackCommand, .previous),
        ]

        for (command, value) in handled {
            command.isEnabled = true
            let target = command.addTarget(handler: Self.handler(for: value, deliverTo: self))
            targets.append((command, target))
        }
    }

    /// The artwork for `image`. MediaPlayer calls the artwork's closure on its own queue, so the closure
    /// is made outside the main actor: one made in a main-actor method would be main-actor isolated, and
    /// Swift traps when it runs elsewhere. The image is only handed back, never changed.
    nonisolated static func artwork(_ image: NSImage) -> MPMediaItemArtwork {
        let box = ImageBox(image: image)
        return MPMediaItemArtwork(boundsSize: image.size) { @Sendable _ in box.image }
    }

    /// A command handler for MediaPlayer, made outside the main actor for the same reason as the
    /// artwork's closure.
    private nonisolated static func handler(
        for command: NowPlayingCommand,
        deliverTo surface: SystemNowPlaying
    ) -> @Sendable (MPRemoteCommandEvent) -> MPRemoteCommandHandlerStatus {
        let deliver = delivery(of: command, to: surface)
        return { @Sendable _ in
            deliver()
            return .success
        }
    }

    /// Hands `command` to the surface on the main actor, whatever thread it is called on.
    nonisolated static func delivery(of command: NowPlayingCommand, to surface: SystemNowPlaying) -> @Sendable () -> Void {
        let weak = WeakSurface(surface)
        return {
            Task { @MainActor in weak.surface?.onCommand?(command) }
        }
    }
}

/// An image handed to MediaPlayer's queue, read there only.
private final class ImageBox: @unchecked Sendable {
    let image: NSImage

    init(image: NSImage) {
        self.image = image
    }
}

/// The surface, held weakly by a handler that MediaPlayer may call off the main actor; read only on it.
private final class WeakSurface: @unchecked Sendable {
    weak var surface: SystemNowPlaying?

    init(_ surface: SystemNowPlaying) {
        self.surface = surface
    }
}
