import AppKit

/// The Mac's ring for an incoming call.
@MainActor
public protocol CallRinging: AnyObject {
    func start()
    func stop()
}

/// Rings with a system sound, repeated every two seconds until stopped. The phone's own ringtone
/// cannot play on the Mac, because macOS opens no voice link to the phone.
@MainActor
public final class CallRinger: CallRinging {
    private static let interval: TimeInterval = 2
    private let sound = NSSound(named: "Ping")
    private var timer: Timer?

    public init() {}

    public func start() {
        guard timer == nil else { return }
        ring()
        timer = Timer.scheduledTimer(withTimeInterval: Self.interval, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.ring()
            }
        }
    }

    public func stop() {
        timer?.invalidate()
        timer = nil
        sound?.stop()
    }

    private func ring() {
        sound?.stop()
        sound?.play()
    }
}
