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

        let timer = Timer(timeInterval: Self.interval, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.ring()
            }
        }
        // The common modes: the ring goes on while an alert is up or a menu (the menu bar extra's) is
        // open, which run the run loop in modes of their own.
        RunLoop.main.add(timer, forMode: .common)
        self.timer = timer
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
