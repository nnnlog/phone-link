import Foundation
import os
import PhoneLinkEngine
import Testing
@testable import PhoneLinkKit

@MainActor
struct HotspotModelTests {
    @Test func aBluetoothFailureReadsAsAnyBluetoothFeatureWouldWordIt() {
        let outcome = HotspotModel.outcome(for: HotspotError.Bluetooth(error: .PhoneNotFound))
        #expect(outcome.message == UserText.describe(BluetoothError.PhoneNotFound))
        #expect(outcome.message.contains("didn't find your phone"))
        #expect(outcome.manualJoin == nil)
    }

    @Test func aFailedPairingPointsAtTryingAgainNotAtASetupStep() {
        let outcome = HotspotModel.outcome(for: HotspotError.Bluetooth(error: .PairingFailed(reason: "refused")))
        #expect(outcome.message.contains("refused"))
        #expect(!outcome.message.localizedCaseInsensitiveContains("set up"))
    }

    @Test func theItemAndTileReadAsOnWhileThisMacIsOnTheHotspot() {
        let api = FakeHotspotApi(HotspotPhoneState(supported: true, busy: false, joined: false))
        let model = HotspotModel(api: api, hub: EngineEventHub())
        model.reload()
        #expect(model.item == .connect)
        #expect(model.tile == PhoneControlTile(.hotspot, "Hotspot", symbol: "personalhotspot"))

        api.state.withLock { $0 = HotspotPhoneState(supported: true, busy: false, joined: true) }
        model.reload()
        #expect(model.item == .connected)
        #expect(model.tile == PhoneControlTile(.hotspot, "Hotspot", state: "Connected", symbol: "personalhotspot", isOn: true))
    }

    @Test func togglingWhileOnTheHotspotDisconnects() async {
        let api = FakeHotspotApi(HotspotPhoneState(supported: true, busy: false, joined: true))
        let model = HotspotModel(api: api, hub: EngineEventHub())
        model.reload()
        model.toggle()
        #expect(model.item == .working(.disconnecting))
        #expect(model.tile?.state == "Disconnecting…")

        await waitUntil { model.working == nil }
        #expect(api.calls.withLock { $0 } == ["disconnect"])
        #expect(model.item == .connect)
    }

    @Test func togglingOffTheHotspotConnects() async {
        let api = FakeHotspotApi(HotspotPhoneState(supported: true, busy: false, joined: false))
        let model = HotspotModel(api: api, hub: EngineEventHub())
        model.reload()
        model.toggle()
        #expect(model.item == .working(.connecting))

        await waitUntil { model.working == nil }
        #expect(api.calls.withLock { $0 } == ["connect"])
        #expect(model.item == .connected)
    }

    @Test func aJoinThatDidNotFinishOffersTheNetwork() {
        let outcome = HotspotModel.outcome(for: HotspotError.JoinFailed(ssid: "Galaxy", passphrase: "12345678", reason: "timeout"))
        #expect(outcome.manualJoin?.ssid == "Galaxy")
        #expect(outcome.manualJoin?.passphrase == "12345678")
    }
}

/// A hotspot API whose connect joins and whose disconnect leaves.
private final class FakeHotspotApi: HotspotApiProtocol, @unchecked Sendable {
    let state: OSAllocatedUnfairLock<HotspotPhoneState>
    let calls = OSAllocatedUnfairLock(initialState: [String]())

    init(_ state: HotspotPhoneState) {
        self.state = OSAllocatedUnfairLock(initialState: state)
    }

    func phoneState() throws -> HotspotPhoneState? {
        state.withLock { $0 }
    }

    func connect() async throws -> String {
        calls.withLock { $0.append("connect") }
        state.withLock { $0.joined = true }
        return "Galaxy"
    }

    func disconnect() async throws {
        calls.withLock { $0.append("disconnect") }
        state.withLock { $0.joined = false }
    }
}

@MainActor
private func waitUntil(_ condition: () -> Bool) async {
    for _ in 0..<1000 {
        if condition() { return }
        try? await Task.sleep(for: .milliseconds(2))
    }
}
