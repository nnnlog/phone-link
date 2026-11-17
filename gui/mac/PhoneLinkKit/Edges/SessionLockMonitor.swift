import CoreGraphics
import PhoneLinkEngine

/// The Mac's session lock state, the app's single lock test, read from the current session
/// dictionary on every call.
public final class SessionLockMonitor: SessionLock {
    public init() {}

    public func lockState() -> LockState {
        Self.lockState(session: CGSessionCopyCurrentDictionary() as? [String: Any])
    }

    /// `CGSSessionScreenIsLocked` true → locked; the key absent or false → unlocked, because an
    /// unlocked session's dictionary has no such key; no dictionary (no login session) → unknown.
    static func lockState(session: [String: Any]?) -> LockState {
        guard let session else { return .unknown }
        return session["CGSSessionScreenIsLocked"] as? Bool == true ? .locked : .unlocked
    }
}
