import AppKit
import Carbon
import CoreAudio
import CoreVideo
import Foundation
import os
import PhoneLinkEngine

private let log = AppLog(category: "mirroring")

/// Screen mirroring's edge on this Mac: the primary display's values, whether an audio output device
/// exists, whether the keyboard input source is an input method, each session's video and each app's
/// audio.
///
/// The engine calls it on its own threads, several sessions at once. Each session has its own H.264
/// decoder, made on its first frame and reset when its stream ends; a decoded picture goes to the
/// sink its window attached, so a frame counts as presented only once a window shows it. Each app's
/// audio plays on a player of its own from its start to its end.
public final class MirrorPresenterAdapter: MirrorPresenter, @unchecked Sendable {
    /// Called on the main thread when the Mac's default output device or its device list changes.
    public var onAudioOutputChanged: (@MainActor () -> Void)? {
        didSet { observeOutputDevices() }
    }

    /// One session's video: its decoder (used by one frame at a time) and its window's sink.
    private final class VideoStream: @unchecked Sendable {
        let decoding = NSLock()
        let decoder = H264Decoder()
        var sink: (any MirrorVideoSink)?
    }

    private let lock = NSLock()
    private var streams: [String: VideoStream] = [:]
    private var players: [String: MirrorAudioPlayer] = [:]
    private var observingOutputs = false

    public init() {}

    // MARK: - Windows

    /// Session `sessionId`'s window shows its pictures on `sink` from now on.
    public func attach(_ sink: any MirrorVideoSink, to sessionId: String) {
        lock.withLock { stream(sessionId).sink = sink }
    }

    /// Session `sessionId`'s window closed: its decoder goes with it.
    public func detach(_ sessionId: String) {
        let stream = lock.withLock { streams.removeValue(forKey: sessionId) }
        if let stream {
            stream.decoding.withLock { stream.decoder.reset() }
        }
    }

    /// The stream of `sessionId`, made when missing; the caller holds `lock`.
    private func stream(_ sessionId: String) -> VideoStream {
        if let stream = streams[sessionId] { return stream }
        let stream = VideoStream()
        streams[sessionId] = stream
        return stream
    }

    // MARK: - MirrorPresenter

    public func displayGeometry() -> DisplayGeometry {
        if Thread.isMainThread {
            return MainActor.assumeIsolated { Self.primaryDisplay() }
        }
        return DispatchQueue.main.sync { MainActor.assumeIsolated { Self.primaryDisplay() } }
    }

    /// The screen with the menu bar: its visible frame (no menu bar, no Dock) less a window's title
    /// bar, and its whole size, in pixels; 96 dpi per point scale.
    @MainActor
    private static func primaryDisplay() -> DisplayGeometry {
        guard let screen = NSScreen.screens.first else {
            return DisplayGeometry(usableWidth: 0, usableHeight: 0, dpi: 96, width: 0, height: 0)
        }

        let scale = screen.backingScaleFactor
        let titleBar = NSWindow.frameRect(forContentRect: .zero, styleMask: [.titled]).height
        let visible = screen.visibleFrame
        func pixels(_ points: CGFloat) -> UInt32 { UInt32(max(0, (points * scale).rounded(.down))) }
        return DisplayGeometry(
            usableWidth: pixels(visible.width),
            usableHeight: pixels(visible.height - titleBar),
            dpi: UInt32((96 * scale).rounded()),
            width: pixels(screen.frame.width),
            height: pixels(screen.frame.height)
        )
    }

    public func hasAudioOutput() -> Bool {
        var device = AudioObjectID(kAudioObjectUnknown)
        var size = UInt32(MemoryLayout<AudioObjectID>.size)
        var address = Self.defaultOutputAddress
        let status = AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &device)
        return status == noErr && device != kAudioObjectUnknown
    }

    public func inputSourceIsInputMethod() -> Bool {
        guard let source = TISCopyCurrentKeyboardInputSource()?.takeRetainedValue(),
              let raw = TISGetInputSourceProperty(source, kTISPropertyInputSourceType)
        else { return false }
        let type = Unmanaged<CFString>.fromOpaque(raw).takeUnretainedValue()
        return CFEqual(type, kTISTypeKeyboardInputMode) || CFEqual(type, kTISTypeKeyboardInputMethodModeEnabled)
    }

    public func videoFrame(sessionId: String, data: Data, keyframe: Bool) -> Bool {
        let stream = lock.withLock { stream(sessionId) }
        let picture: CVPixelBuffer? = stream.decoding.withLock { stream.decoder.decode(data) }
        guard let picture else { return false }
        guard let sink = lock.withLock({ stream.sink }) else { return false }
        return sink.present(picture)
    }

    public func resetVideo(sessionId: String) {
        let stream = lock.withLock { streams[sessionId] }
        if let stream {
            stream.decoding.withLock { stream.decoder.reset() }
        }
    }

    public func audioStarted(packageName: String, format: AudioStreamFormat) {
        let player = MirrorAudioPlayer(packageName: packageName, format: format)
        let replaced = lock.withLock {
            let old = players.removeValue(forKey: packageName)
            if let player { players[packageName] = player }
            return old
        }
        replaced?.stop()
    }

    public func audioFrame(packageName: String, data: Data, timestamp: UInt64) {
        lock.withLock { players[packageName] }?.play(data)
    }

    public func audioEnded(packageName: String) {
        lock.withLock { players.removeValue(forKey: packageName) }?.stop()
    }

    // MARK: - Output devices

    private static var defaultOutputAddress: AudioObjectPropertyAddress {
        AudioObjectPropertyAddress(
            mSelector: kAudioHardwarePropertyDefaultOutputDevice,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
    }

    private func observeOutputDevices() {
        let start = lock.withLock {
            defer { observingOutputs = true }
            return !observingOutputs
        }
        guard start else { return }

        let changed: AudioObjectPropertyListenerBlock = { [weak self] _, _ in
            DispatchQueue.main.async {
                MainActor.assumeIsolated { self?.onAudioOutputChanged?() }
            }
        }

        for selector in [kAudioHardwarePropertyDefaultOutputDevice, kAudioHardwarePropertyDevices] {
            var address = AudioObjectPropertyAddress(
                mSelector: selector,
                mScope: kAudioObjectPropertyScopeGlobal,
                mElement: kAudioObjectPropertyElementMain
            )
            let status = AudioObjectAddPropertyListenerBlock(AudioObjectID(kAudioObjectSystemObject), &address, DispatchQueue.main, changed)
            if status != noErr {
                log.error("audio output changes not observed: status \(status); the engine is not told when outputs change")
            }
        }
    }
}
