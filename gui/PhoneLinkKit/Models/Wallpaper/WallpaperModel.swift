import AppKit
import Observation
import os
import PhoneLinkEngine

private let logger = AppLog(category: "wallpaper")

/// The selected phone's wallpaper, for the sidebar: its picture of the phone and, while the user
/// keeps "Use phone wallpaper as background" on, its background.
@MainActor
@Observable
public final class WallpaperModel {
    /// The phone's wallpaper; `nil` when there is none (or it does not decode).
    public private(set) var image: NSImage?

    /// The user's setting (default on), kept in the app's preferences.
    public var usesAsBackground: Bool {
        didSet { defaults.set(usesAsBackground, forKey: PreferenceKeys.useWallpaperAsBackground) }
    }

    /// The background shows: the setting is on, macOS's Reduce Transparency is off, and there is a
    /// wallpaper.
    public var showsBackground: Bool {
        usesAsBackground && !transparencyReduced && image != nil
    }

    /// macOS's Reduce Transparency, re-read when the accessibility display options change.
    public private(set) var transparencyReduced: Bool

    private let api: any WallpaperApiProtocol
    @ObservationIgnored private let defaults: UserDefaults
    @ObservationIgnored private let reducesTransparency: @MainActor () -> Bool
    @ObservationIgnored private var accessibilityObserver: NSObjectProtocol?

    public convenience init(api: any WallpaperApiProtocol, hub: EngineEventHub) {
        self.init(
            api: api,
            hub: hub,
            defaults: .standard,
            reducesTransparency: { NSWorkspace.shared.accessibilityDisplayShouldReduceTransparency }
        )

        accessibilityObserver = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.accessibilityDisplayOptionsDidChangeNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.accessibilityChanged() }
        }
    }

    init(
        api: any WallpaperApiProtocol,
        hub: EngineEventHub,
        defaults: UserDefaults,
        reducesTransparency: @escaping @MainActor () -> Bool
    ) {
        self.api = api
        self.defaults = defaults
        self.reducesTransparency = reducesTransparency
        transparencyReduced = reducesTransparency()
        usesAsBackground = defaults.object(forKey: PreferenceKeys.useWallpaperAsBackground) as? Bool ?? true

        hub.onEvent { [weak self] event in
            self?.handle(event)
        }
        hub.onStart { [weak self] in
            self?.reload()
        }
    }

    /// Re-reads Reduce Transparency.
    public func accessibilityChanged() {
        transparencyReduced = reducesTransparency()
    }

    public func reload() {
        do {
            guard let wallpaper = try api.wallpaper() else {
                image = nil
                return
            }
            image = NSImage(data: wallpaper.image)
            if image == nil {
                logger.notice("phone wallpaper not decoded; the sidebar shows none")
            }
        } catch let error as FeatureApiError where error == .NotStarted || error == .NoPhone {
            image = nil
        } catch {
            logger.error("phone wallpaper not read: \(error: error)")
        }
    }

    private func handle(_ event: EngineEvent) {
        switch event {
        case .wallpaper(.changed), .resync(feature: .wallpaper), .phoneChanged, .phoneUnlinked, .accountRemoved:
            reload()
        default:
            break
        }
    }
}
