import Dispatch
import Network
import os
import PhoneLinkEngine

/// Internet reachability and the kind of the current path, from one `NWPathMonitor` that runs for
/// the adapter's lifetime. The engine's reachability flag and the PC status for the phone read the
/// same instance.
public final class NetworkPathMonitor: Reachability {
    private let monitor = NWPathMonitor()
    /// The latest path the monitor delivered; `nil` until the first one, which follows the start
    /// within milliseconds.
    private let path = OSAllocatedUnfairLock<NWPath?>(initialState: nil)

    public init() {
        let path = self.path
        monitor.pathUpdateHandler = { update in
            path.withLock { $0 = update }
        }
        monitor.start(queue: DispatchQueue(label: "\(PhoneLinkIdentity.bundleID).network-path"))
    }

    deinit {
        monitor.cancel()
    }

    public func isInternetAvailable() -> Bool {
        internetPath() != .unavailable
    }

    /// The kind of the current path; unavailable until the monitor delivered one.
    public func internetPath() -> PcInternetPath {
        guard let current = path.withLock({ $0 }) else { return .unavailable }
        return Self.kind(satisfied: current.status == .satisfied, uses: current.usesInterfaceType)
    }

    /// A satisfied path's kind is the first of Wi-Fi, cellular and wired Ethernet it uses, else
    /// other; any other status is unavailable.
    static func kind(satisfied: Bool, uses: (NWInterface.InterfaceType) -> Bool) -> PcInternetPath {
        guard satisfied else { return .unavailable }
        if uses(.wifi) { return .wiFi }
        if uses(.cellular) { return .cellular }
        if uses(.wiredEthernet) { return .wiredEthernet }
        return .other
    }
}
