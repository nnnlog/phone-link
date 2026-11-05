import Observation

/// What a mirroring window's views share with the window around them: whether the user is resizing
/// the window (the navigation bar hides meanwhile) and the window's volume popup.
@MainActor
@Observable
public final class MirrorWindowContext {
    /// The user is dragging the window's border.
    public var liveResizing = false
    public let volume: MirrorVolumePopup

    public init(volume: MirrorVolumePopup) {
        self.volume = volume
    }
}
