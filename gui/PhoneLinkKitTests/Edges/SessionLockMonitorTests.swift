import Foundation
import PhoneLinkEngine
import Testing
@testable import PhoneLinkKit

struct SessionLockMonitorTests {
    @Test func theLockKeyDecides() {
        #expect(SessionLockMonitor.lockState(session: ["CGSSessionScreenIsLocked": true]) == .locked)
        #expect(SessionLockMonitor.lockState(session: ["CGSSessionScreenIsLocked": NSNumber(value: 1)]) == .locked)
        #expect(SessionLockMonitor.lockState(session: ["CGSSessionScreenIsLocked": false]) == .unlocked)
    }

    @Test func anUnlockedSessionHasNoLockKey() {
        #expect(SessionLockMonitor.lockState(session: ["kCGSSessionOnConsoleKey": true, "kCGSSessionUserIDKey": 501]) == .unlocked)
        #expect(SessionLockMonitor.lockState(session: [:]) == .unlocked)
    }

    @Test func noSessionIsUnknown() {
        #expect(SessionLockMonitor.lockState(session: nil) == .unknown)
    }
}
