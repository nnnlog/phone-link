import CoreLocation
import CoreWLAN
import Foundation
import PhoneLinkEngine

/// Wi-Fi for instant hotspot: CoreWLAN scans for the phone's hotspot, joins and leaves it, and names
/// the network the Mac is on. macOS may withhold network names from apps without Location Services
/// authorization, so the first scan asks for it and waits for the user's answer.
public final class HotspotWifiAdapter: HotspotWifi, @unchecked Sendable {
    /// Called on the main thread when the Mac's Wi-Fi network or its link changes.
    public var onNetworkChanged: (@MainActor () -> Void)? {
        didSet { events.start { [weak self] in self?.onNetworkChanged?() } }
    }

    private let location = LocationAuthorization()
    private let events = WifiEvents()

    public init() {}

    public func hasInterface() -> Bool {
        CWWiFiClient.shared().interface() != nil
    }

    public func isVisible(ssid: String) throws -> Bool {
        try !scan(ssid).isEmpty
    }

    public func join(ssid: String, passphrase: String) throws {
        guard let interface = CWWiFiClient.shared().interface() else { throw WifiError.Unavailable }
        guard let network = try scan(ssid).first else {
            throw WifiError.Failed(reason: "the network is not visible")
        }

        do {
            try interface.associate(to: network, password: passphrase)
        } catch {
            throw WifiError.Failed(reason: error.localizedDescription)
        }
    }

    public func currentSsid() -> String? {
        CWWiFiClient.shared().interface()?.ssid()
    }

    public func leave() throws {
        guard let interface = CWWiFiClient.shared().interface() else { throw WifiError.Unavailable }
        interface.disassociate()
    }

    private func scan(_ ssid: String) throws -> Set<CWNetwork> {
        guard let interface = CWWiFiClient.shared().interface(), interface.powerOn() else {
            throw WifiError.Unavailable
        }

        location.requestAndWait(timeout: 60)
        do {
            return try interface.scanForNetworks(withName: ssid)
        } catch {
            throw WifiError.Failed(reason: error.localizedDescription)
        }
    }
}

/// CoreWLAN's SSID and link events, each handed to the main thread. Events are monitored once a
/// handler is set; a monitor CoreWLAN refuses is logged, and the state is then read only when the
/// phone record changes or an action ends.
private final class WifiEvents: NSObject, CWEventDelegate, @unchecked Sendable {
    private var changed: (@MainActor () -> Void)?
    private static let log = AppLog(category: "hotspot")

    func start(_ changed: @escaping @MainActor () -> Void) {
        let first = self.changed == nil
        self.changed = changed
        guard first else { return }

        let client = CWWiFiClient.shared()
        client.delegate = self
        for event in [CWEventType.ssidDidChange, .linkDidChange] {
            do {
                try client.startMonitoringEvent(with: event)
            } catch {
                Self.log.error("Wi-Fi event \(event.rawValue) not monitored: \(error: error)")
            }
        }
    }

    func ssidDidChangeForWiFiInterface(withName interfaceName: String) {
        notify()
    }

    func linkDidChangeForWiFiInterface(withName interfaceName: String) {
        notify()
    }

    private func notify() {
        DispatchQueue.main.async { [weak self] in
            MainActor.assumeIsolated { self?.changed?() }
        }
    }
}

/// Asks for Location Services authorization while it is undetermined and waits, up to a timeout, for
/// the user's answer. The engine calls the adapter on its own threads, never the main thread, which
/// the location manager needs.
private final class LocationAuthorization: NSObject, CLLocationManagerDelegate, @unchecked Sendable {
    private var manager: CLLocationManager?
    private let decided = NSCondition()
    private var undetermined = true

    func requestAndWait(timeout: TimeInterval) {
        DispatchQueue.main.async { [self] in
            if manager == nil {
                manager = CLLocationManager()
                manager?.delegate = self
            }
            record(manager?.authorizationStatus ?? .notDetermined)
            if manager?.authorizationStatus == .notDetermined {
                manager?.requestWhenInUseAuthorization()
            }
        }

        let deadline = Date(timeIntervalSinceNow: timeout)
        decided.lock()
        defer { decided.unlock() }
        while undetermined {
            if !decided.wait(until: deadline) { return }
        }
    }

    func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
        record(manager.authorizationStatus)
    }

    private func record(_ status: CLAuthorizationStatus) {
        decided.lock()
        undetermined = status == .notDetermined
        decided.broadcast()
        decided.unlock()
    }
}
