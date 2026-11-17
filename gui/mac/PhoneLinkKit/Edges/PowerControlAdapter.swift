import CoreServices
import Foundation
import IOKit.pwr_mgt
import PhoneLinkEngine

/// The Mac's actions on the phone's remote-control request: lock the session, sleep, shut down,
/// restart. The engine decides whether to act; this only acts.
///
/// - Lock: `SACLockScreenImmediate` of the private `login.framework`, looked up when called — macOS
///   has no public call that locks the session at once whatever the "require password" setting says.
///   A missing symbol or a nonzero return is a failure.
/// - Sleep: IOKit `IOPMSleepSystem` on the root power domain; it returns once sleep is requested.
/// - Shut down / restart: an Apple event `kAEShutDown` / `kAERestart` to `loginwindow`, sent without
///   waiting for a reply. Not forced: every application is asked to quit and one can cancel.
public final class PowerControlAdapter: PcPowerControl {
    /// What the adapter calls; the tests replace them so nothing acts on the machine.
    struct Calls: Sendable {
        var lockScreen: @Sendable () throws -> Void
        var sleep: @Sendable () throws -> Void
        var sendToLoginWindow: @Sendable (NSAppleEventDescriptor) throws -> Void
    }

    static let loginFrameworkPath = "/System/Library/PrivateFrameworks/login.framework/Versions/Current/login"
    static let lockSymbol = "SACLockScreenImmediate"
    static let loginWindowBundleID = "com.apple.loginwindow"

    private let calls: Calls

    public convenience init() {
        self.init(calls: .live)
    }

    init(calls: Calls) {
        self.calls = calls
    }

    public func lockScreen() throws {
        try calls.lockScreen()
    }

    public func sleep() throws {
        try calls.sleep()
    }

    public func shutDown() throws {
        try calls.sendToLoginWindow(Self.loginWindowEvent(AEEventID(kAEShutDown)))
    }

    public func restart() throws {
        try calls.sendToLoginWindow(Self.loginWindowEvent(AEEventID(kAERestart)))
    }

    /// The core event `eventID` addressed to `loginwindow`.
    static func loginWindowEvent(_ eventID: AEEventID) -> NSAppleEventDescriptor {
        NSAppleEventDescriptor.appleEvent(
            withEventClass: AEEventClass(kCoreEventClass),
            eventID: eventID,
            targetDescriptor: NSAppleEventDescriptor(bundleIdentifier: loginWindowBundleID),
            returnID: AEReturnID(kAutoGenerateReturnID),
            transactionID: AETransactionID(kAnyTransactionID)
        )
    }

    /// `SACLockScreenImmediate` from `library` (`nil` when it is not there).
    static func lockFunction(library: String = loginFrameworkPath) -> (@convention(c) () -> Int32)? {
        guard let handle = dlopen(library, RTLD_LAZY), let symbol = dlsym(handle, lockSymbol) else { return nil }
        return unsafeBitCast(symbol, to: (@convention(c) () -> Int32).self)
    }
}

extension PowerControlAdapter.Calls {
    static let live = PowerControlAdapter.Calls(
        lockScreen: {
            guard let lock = PowerControlAdapter.lockFunction() else {
                throw EdgeError.Unexpected(message: "\(PowerControlAdapter.lockSymbol) is not available on this Mac")
            }
            let status = lock()
            guard status == 0 else {
                throw EdgeError.Unexpected(message: "\(PowerControlAdapter.lockSymbol) returned \(status)")
            }
        },
        sleep: {
            let rootDomain = IOPMFindPowerManagement(kIOMainPortDefault)
            guard rootDomain != IO_OBJECT_NULL else {
                throw EdgeError.Unexpected(message: "IOPMFindPowerManagement found no root power domain")
            }
            defer { IOServiceClose(rootDomain) }
            let result = IOPMSleepSystem(rootDomain)
            guard result == kIOReturnSuccess else {
                throw EdgeError.Unexpected(message: "IOPMSleepSystem returned \(result)")
            }
        },
        sendToLoginWindow: { event in
            do {
                _ = try event.sendEvent(options: [.noReply], timeout: 10)
            } catch {
                throw EdgeError.Unexpected(message: "loginwindow did not take the event: \(error)")
            }
        }
    )
}
