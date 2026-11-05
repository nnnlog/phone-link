import Foundation
import Observation

/// A mirroring window's volume popup: a mute button and a 0–100 slider for the app's volume on the
/// phone.
///
/// `state` is the volume as last received from the phone (or set by the mute button); the slider
/// starts there and goes back there whenever a new state is received. Moving the slider changes
/// nothing on the phone until the change is committed — on the pointer's release, the release of End,
/// Home or an arrow key, the slider losing the keyboard focus, the popup closing, or `scrollSettle`
/// after the last scroll-wheel notch — and a commit sends the slider's value, muted at 0. A dragged or
/// keyed change counts only when the value differs from the last one committed or received; scrolling
/// always counts. A state received drops the change not yet committed, as does the window closing
/// (`cancel`). The mute button sends the received volume with the mute flag turned over, at once.
@MainActor
@Observable
public final class MirrorVolumePopup {
    /// An app's volume on the phone.
    public struct State: Equatable, Sendable {
        public var volume: Int32
        public var muted: Bool

        public init(volume: Int32, muted: Bool) {
            self.volume = volume
            self.muted = muted
        }
    }

    /// How far one scroll-wheel notch moves the slider.
    public static let scrollStep: Double = 2
    /// How long after the last scroll-wheel notch its change is committed.
    public static let scrollSettle: Duration = .milliseconds(500)
    public static let range: ClosedRange<Double> = 0 ... 100

    /// The volume as last received (or set by the mute button).
    public private(set) var state: State
    /// The slider's value.
    public private(set) var slider: Double

    /// The volume last committed or received: a dragged or keyed value equal to it changes nothing.
    @ObservationIgnored private var lastCommitted: Int32
    @ObservationIgnored private var changed = false
    @ObservationIgnored private var scrolling = false
    @ObservationIgnored private var scrollEnd: Task<Void, Never>?
    @ObservationIgnored private let send: (State) -> Void

    /// A popup showing `state`; `send` passes a committed volume or a mute change on to the phone.
    public init(state: State, send: @escaping (State) -> Void) {
        self.state = state
        slider = Double(state.volume)
        lastCommitted = state.volume
        self.send = send
    }

    /// A state from the phone: shown, and a change not yet committed dropped. The same state again
    /// changes nothing.
    public func receive(_ new: State) {
        guard new != state else { return }
        state = new
        cancel()
        lastCommitted = new.volume
        slider = Double(new.volume)
    }

    /// The user dragged the slider, or moved it with a key, to `value`.
    public func moved(to value: Double) {
        slider = Self.clamped(value)
        if !scrolling {
            changed = Self.level(slider) != lastCommitted
        }
    }

    /// One scroll-wheel notch: up (`up`) or down. Its change is committed `scrollSettle` after the last one.
    public func scrolled(up: Bool) {
        scrollEnd?.cancel()
        scrolling = true
        slider = Self.clamped(slider + (up ? Self.scrollStep : -Self.scrollStep))

        scrollEnd = Task { [weak self] in
            try? await Task.sleep(for: Self.scrollSettle)
            guard !Task.isCancelled else { return }
            self?.commit()
        }
    }

    /// Sends the change not yet committed, if any.
    public func commit() {
        scrollEnd?.cancel()
        scrollEnd = nil
        let pending = changed || scrolling
        changed = false
        scrolling = false
        guard pending else { return }

        let volume = Self.level(slider)
        lastCommitted = volume
        send(State(volume: volume, muted: volume == 0))
    }

    /// Drops the change not yet committed.
    public func cancel() {
        scrollEnd?.cancel()
        scrollEnd = nil
        changed = false
        scrolling = false
    }

    /// The mute button: the received volume with the mute flag turned over, sent at once.
    public func toggleMute() {
        let new = State(volume: state.volume, muted: !state.muted)
        send(new)
        receive(new)
    }

    private static func clamped(_ value: Double) -> Double {
        min(max(value, range.lowerBound), range.upperBound)
    }

    /// The slider's value as a volume, rounded to the nearest whole number (a half to the even one).
    private static func level(_ value: Double) -> Int32 {
        Int32(value.rounded(.toNearestOrEven))
    }
}
