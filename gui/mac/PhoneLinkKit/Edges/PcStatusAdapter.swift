import CoreWLAN
import Foundation
import IOKit.ps
import PhoneLinkEngine

/// The Mac's own status for the phone's `/remotedevicestatus` request: IOKit power sources, Low
/// Power Mode, the shared path monitor and CoreWLAN. The engine maps them to the wire fields.
public final class PcStatusAdapter: PcStatusSource {
    private let network: NetworkPathMonitor

    public init(network: NetworkPathMonitor) {
        self.network = network
    }

    public func internalBattery() throws -> PcBattery? {
        guard let info = IOPSCopyPowerSourcesInfo()?.takeRetainedValue() else {
            throw EdgeError.Unexpected(message: "IOPSCopyPowerSourcesInfo returned nothing")
        }
        guard let sources = IOPSCopyPowerSourcesList(info)?.takeRetainedValue() as? [CFTypeRef] else {
            throw EdgeError.Unexpected(message: "IOPSCopyPowerSourcesList returned nothing")
        }

        let descriptions = sources.compactMap {
            IOPSGetPowerSourceDescription(info, $0)?.takeUnretainedValue() as? [String: Any]
        }
        return try Self.internalBattery(in: descriptions)
    }

    public func lowPowerMode() throws -> Bool {
        ProcessInfo.processInfo.isLowPowerModeEnabled
    }

    public func internetPath() throws -> PcInternetPath {
        network.internetPath()
    }

    /// `nil` without an interface, while not associated (the RSSI is then 0) or when the value is
    /// withheld (also 0).
    public func wifiRssi() throws -> Int32? {
        guard let interface = CWWiFiClient.shared().interface() else { return nil }
        let rssi = interface.rssiValue()
        return rssi == 0 ? nil : Int32(rssi)
    }

    /// The first source whose type is the internal battery; `nil` when there is none. A battery
    /// whose description lacks a capacity or the power state cannot be reported.
    static func internalBattery(in descriptions: [[String: Any]]) throws -> PcBattery? {
        guard let battery = descriptions.first(where: { $0[kIOPSTypeKey] as? String == kIOPSInternalBatteryType }) else {
            return nil
        }
        guard let current = (battery[kIOPSCurrentCapacityKey] as? Int).flatMap(Int32.init(exactly:)),
              let maximum = (battery[kIOPSMaxCapacityKey] as? Int).flatMap(Int32.init(exactly:)),
              let state = battery[kIOPSPowerSourceStateKey] as? String
        else {
            throw EdgeError.Unexpected(message: "the internal battery's description lacks its capacity or power state")
        }
        return PcBattery(currentCapacity: current, maxCapacity: maximum, onAcPower: state == kIOPSACPowerValue)
    }
}
