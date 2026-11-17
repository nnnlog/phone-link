import Foundation
import PhoneLinkEngine
import Testing
@testable import PhoneLinkKit

@MainActor
struct RemoteControlModelTests {
    @Test func readsTheStateOnceTheEngineStarts() {
        let api = FakeRemoteControl()
        api.started = false
        let hub = EngineEventHub()
        let model = RemoteControlModel(api: api, hub: hub)
        model.reload()
        #expect(model.state == nil && !model.allowsRemoteControl && !model.toggleApplies)

        api.started = true
        hub.postStarted()
        #expect(model.state == RemoteControlPhoneState(toggle: true, applicable: true, capable: true, v2Capable: true))
        #expect(model.allowsRemoteControl && model.toggleApplies)
        #expect(model.detail.contains("sleep, restart or shut down"))
    }

    @Test func theToggleIsWrittenAndReadBack() {
        let api = FakeRemoteControl()
        let hub = EngineEventHub()
        let model = RemoteControlModel(api: api, hub: hub)
        model.reload()

        model.allowsRemoteControl = false
        #expect(api.writes == [false])
        #expect(!model.allowsRemoteControl && model.state?.toggle == false)
        #expect(model.failure == nil)
    }

    @Test func aFailedWriteIsPresented() {
        let api = FakeRemoteControl()
        let model = RemoteControlModel(api: api, hub: EngineEventHub())
        model.reload()

        api.failWrites = true
        model.allowsRemoteControl = false
        #expect(model.failure?.title == "Couldn't Change the Setting")
        #expect(model.allowsRemoteControl, "the engine kept the toggle on")
    }

    @Test func theToggleIsOfferedOnlyWhileItApplies() {
        let api = FakeRemoteControl()
        let hub = EngineEventHub()
        let model = RemoteControlModel(api: api, hub: hub)
        api.capable = false
        model.reload()
        #expect(!model.toggleApplies && !model.allowsRemoteControl)
        #expect(model.detail == "This phone doesn't support controlling this Mac.")

        api.capable = true
        api.v2Capable = false
        hub.post(.remoteControl(change: .phoneChanged))
        #expect(model.toggleApplies && model.allowsRemoteControl)
        #expect(model.detail == "Your phone can lock this Mac without asking first.")

        api.optionOn = false
        hub.post(.phoneChanged)
        #expect(!model.toggleApplies)
        #expect(model.detail == "Remote control is turned off on this Mac.")
    }

    @Test func nothingIsReadOrWrittenAfterThePhoneLeft() {
        let api = FakeRemoteControl()
        let hub = EngineEventHub()
        let model = RemoteControlModel(api: api, hub: hub)
        model.reload()

        hub.post(.phoneUnlinked)
        #expect(model.state == nil)

        model.allowsRemoteControl = false
        hub.post(.remoteControl(change: .phoneChanged))
        #expect(api.writes.isEmpty && model.state == nil)

        hub.postStarted()
        #expect(model.state != nil)
    }
}

private final class FakeRemoteControl: RemoteControlApiProtocol, @unchecked Sendable {
    var started = true
    var toggle = true
    var capable = true
    var v2Capable = true
    var optionOn = true
    var failWrites = false
    var writes: [Bool] = []

    func phoneState() throws -> RemoteControlPhoneState? {
        guard started else { throw FeatureApiError.NotStarted }
        return RemoteControlPhoneState(
            toggle: toggle, applicable: capable && optionOn, capable: capable, v2Capable: v2Capable
        )
    }

    func setPhoneToggle(enabled: Bool) throws {
        guard started else { throw FeatureApiError.NotStarted }
        if failWrites {
            throw FeatureApiError.Store(message: "disk full")
        }

        writes.append(enabled)
        toggle = enabled
    }
}
