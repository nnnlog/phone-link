import Foundation
import IOBluetooth
import PhoneLinkEngine

/// The Bluetooth Classic state of the phone for the messaging proximity gate. IOBluetooth is
/// touched only when the engine asks, so the Bluetooth permission prompt appears only when the gate
/// needs it.
public final class BluetoothProximity: PhoneProximity {
    public init() {}

    /// The phone is the first paired device whose name is one of the phone's names: which paired
    /// device is the phone is the page's open question, and the name is all both sides know.
    public func state(phone: ProximityPhone) -> ProximityState {
        guard let controller = IOBluetoothHostController.default(),
              controller.powerState == kBluetoothHCIPowerStateON
        else { return .bluetoothOff }

        let paired = IOBluetoothDevice.pairedDevices() as? [IOBluetoothDevice] ?? []
        guard let device = paired.first(where: { Self.isPhone(deviceName: $0.name, phoneNames: phone.names) }) else {
            return .phoneNotPaired
        }
        return device.isConnected() ? .connected : .notConnected
    }

    /// The names match ignoring case and surrounding white space; an empty name matches nothing.
    static func isPhone(deviceName: String?, phoneNames: [String]) -> Bool {
        guard let name = deviceName?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty else {
            return false
        }
        return phoneNames.contains {
            $0.trimmingCharacters(in: .whitespacesAndNewlines).caseInsensitiveCompare(name) == .orderedSame
        }
    }
}
