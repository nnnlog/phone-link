import IOKit.ps
import PhoneLinkEngine
import Testing
@testable import PhoneLinkKit

struct PcStatusAdapterTests {
    private static func battery(current: Int, max: Int, state: String) -> [String: Any] {
        [
            kIOPSTypeKey: kIOPSInternalBatteryType,
            kIOPSCurrentCapacityKey: current,
            kIOPSMaxCapacityKey: max,
            kIOPSPowerSourceStateKey: state,
        ]
    }

    @Test func theInternalBatteryIsReadFromItsDescription() throws {
        let charging = try PcStatusAdapter.internalBattery(in: [Self.battery(current: 87, max: 100, state: kIOPSACPowerValue)])
        #expect(charging == PcBattery(currentCapacity: 87, maxCapacity: 100, onAcPower: true))

        let onBattery = try PcStatusAdapter.internalBattery(in: [Self.battery(current: 4321, max: 5000, state: kIOPSBatteryPowerValue)])
        #expect(onBattery == PcBattery(currentCapacity: 4321, maxCapacity: 5000, onAcPower: false))
    }

    @Test func onlyTheInternalBatteryCounts() throws {
        let ups: [String: Any] = [
            kIOPSTypeKey: kIOPSUPSType,
            kIOPSCurrentCapacityKey: 50,
            kIOPSMaxCapacityKey: 100,
            kIOPSPowerSourceStateKey: kIOPSACPowerValue,
        ]

        #expect(try PcStatusAdapter.internalBattery(in: []) == nil, "a desktop Mac")
        #expect(try PcStatusAdapter.internalBattery(in: [ups]) == nil)

        let battery = try PcStatusAdapter.internalBattery(in: [ups, Self.battery(current: 10, max: 100, state: kIOPSBatteryPowerValue)])
        #expect(battery?.currentCapacity == 10)
    }

    @Test func aBatteryWithoutItsKeysCannotBeReported() {
        var incomplete = Self.battery(current: 1, max: 100, state: kIOPSACPowerValue)
        incomplete[kIOPSMaxCapacityKey] = nil
        #expect(throws: EdgeError.self) { try PcStatusAdapter.internalBattery(in: [incomplete]) }
    }

    @Test func thisMacsPowerSourcesCanBeRead() throws {
        let battery = try PcStatusAdapter(network: NetworkPathMonitor()).internalBattery()
        if let battery {
            #expect(battery.maxCapacity > 0)
        }
    }
}
