import Testing
@testable import PhoneLinkKit

struct BluetoothProximityTests {
    @Test func aDeviceIsThePhoneWhenItsNameIsOneOfThePhonesNames() {
        let names = ["Galaxy S25", "My Phone"]

        #expect(BluetoothProximity.isPhone(deviceName: "Galaxy S25", phoneNames: names))
        #expect(BluetoothProximity.isPhone(deviceName: "my phone", phoneNames: names), "case is ignored")
        #expect(BluetoothProximity.isPhone(deviceName: "  Galaxy S25\n", phoneNames: [" galaxy s25 "]), "surrounding white space is ignored")
        #expect(!BluetoothProximity.isPhone(deviceName: "Galaxy S25 Ultra", phoneNames: names), "no prefix match")
        #expect(!BluetoothProximity.isPhone(deviceName: "AirPods", phoneNames: names))
    }

    @Test func anEmptyOrMissingNameMatchesNothing() {
        #expect(!BluetoothProximity.isPhone(deviceName: nil, phoneNames: ["Phone"]))
        #expect(!BluetoothProximity.isPhone(deviceName: "", phoneNames: ["", " "]))
        #expect(!BluetoothProximity.isPhone(deviceName: "  ", phoneNames: ["  "]))
        #expect(!BluetoothProximity.isPhone(deviceName: "Phone", phoneNames: []))
    }
}
