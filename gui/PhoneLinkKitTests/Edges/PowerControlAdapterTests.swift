import CoreServices
import Foundation
import os
import PhoneLinkEngine
import Testing
@testable import PhoneLinkKit

/// Every test replaces the adapter's calls: none may lock, sleep, restart or shut down this Mac.
struct PowerControlAdapterTests {
    private final class Recorder: Sendable {
        let calls = OSAllocatedUnfairLock(initialState: [String]())
        let events = OSAllocatedUnfairLock(initialState: [(AEEventClass, AEEventID, String?)]())

        var adapterCalls: PowerControlAdapter.Calls {
            PowerControlAdapter.Calls(
                lockScreen: { self.calls.withLock { $0.append("lock") } },
                sleep: { self.calls.withLock { $0.append("sleep") } },
                sendToLoginWindow: { event in
                    let target = event.attributeDescriptor(forKeyword: keyAddressAttr).map { String(decoding: $0.data, as: UTF8.self) }
                    let seen = (event.eventClass, event.eventID, target)
                    self.events.withLock { $0.append(seen) }
                    self.calls.withLock { $0.append("event") }
                }
            )
        }
    }

    @Test func eachActionMakesItsCall() throws {
        let recorder = Recorder()
        let adapter = PowerControlAdapter(calls: recorder.adapterCalls)

        try adapter.lockScreen()
        try adapter.sleep()
        try adapter.shutDown()
        try adapter.restart()

        #expect(recorder.calls.withLock { $0 } == ["lock", "sleep", "event", "event"])
        let events = recorder.events.withLock { $0 }
        #expect(events.map(\.0) == [AEEventClass(kCoreEventClass), AEEventClass(kCoreEventClass)])
        #expect(events.map(\.1) == [AEEventID(kAEShutDown), AEEventID(kAERestart)])
        #expect(events.map(\.2) == ["com.apple.loginwindow", "com.apple.loginwindow"])
    }

    @Test func theEventIsAddressedToLoginWindow() {
        let event = PowerControlAdapter.loginWindowEvent(AEEventID(kAERestart))
        #expect(event.eventClass == AEEventClass(kCoreEventClass))
        #expect(event.eventID == AEEventID(kAERestart))

        let target = event.attributeDescriptor(forKeyword: keyAddressAttr)
        #expect(target?.descriptorType == DescType(typeApplicationBundleID))
        #expect(target.map { String(decoding: $0.data, as: UTF8.self) } == "com.apple.loginwindow")
    }

    @Test func aFailingCallIsTheEdgesFailure() {
        let failing = PowerControlAdapter.Calls(
            lockScreen: { throw EdgeError.Unexpected(message: "no lock") },
            sleep: { throw EdgeError.Unexpected(message: "no sleep") },
            sendToLoginWindow: { _ in throw EdgeError.Unexpected(message: "no event") }
        )
        let adapter = PowerControlAdapter(calls: failing)

        #expect(throws: EdgeError.self) { try adapter.lockScreen() }
        #expect(throws: EdgeError.self) { try adapter.sleep() }
        #expect(throws: EdgeError.self) { try adapter.shutDown() }
        #expect(throws: EdgeError.self) { try adapter.restart() }
    }

    /// The lock function is looked up, never called.
    @Test func theLockFunctionIsFoundOnThisMacAndAMissingLibraryIsNil() {
        #expect(PowerControlAdapter.lockFunction() != nil)
        #expect(PowerControlAdapter.lockFunction(library: "/System/Library/PrivateFrameworks/NoSuch.framework/NoSuch") == nil)
    }
}
