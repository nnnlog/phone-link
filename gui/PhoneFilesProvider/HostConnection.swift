import FileProvider
import Foundation
import os

let log = Logger(subsystem: "\(PhoneLinkIdentity.bundleID).PhoneFilesProvider", category: "phone-files")

/// The connection to the app, which runs the engine. The app connects through the extension's
/// service (`ServiceSource`) whenever its phone's domain exists; the latest connection serves every
/// call. A process without a connection asks the app for one (`PhoneFilesXPC.needsHostNotification`)
/// when it starts and when a call finds none; the call waits briefly for it, then fails as unreachable.
final class HostConnection: NSObject, NSXPCListenerDelegate, PhoneFilesExtension, @unchecked Sendable {
    static let shared = HostConnection()

    /// How long a call waits for the app to connect.
    private static let attachWait: TimeInterval = 10

    let listener = NSXPCListener.anonymous()
    private let lock = NSLock()
    private var connection: NSXPCConnection?
    private var waiting: [(NSXPCConnection) -> Void] = []
    private var progressHandlers: [String: (Int64, Int64) -> Void] = [:]

    private override init() {
        super.init()
        listener.delegate = self
        listener.resume()
        askForHost()
    }

    /// Asks the app to connect to this process.
    private func askForHost() {
        DistributedNotificationCenter.default().postNotificationName(
            PhoneFilesXPC.needsHostNotification, object: nil, userInfo: nil, deliverImmediately: true
        )
    }

    func listener(_ listener: NSXPCListener, shouldAcceptNewConnection connection: NSXPCConnection) -> Bool {
        connection.exportedInterface = NSXPCInterface(with: PhoneFilesExtension.self)
        connection.exportedObject = self
        connection.remoteObjectInterface = NSXPCInterface(with: PhoneFilesHost.self)
        connection.invalidationHandler = { [weak self, weak connection] in self?.dropped(connection) }
        connection.interruptionHandler = { [weak self, weak connection] in self?.dropped(connection) }
        connection.resume()

        lock.lock()
        let previous = self.connection
        self.connection = connection
        let waiters = waiting
        waiting = []
        lock.unlock()

        previous?.invalidate()
        log.info("app connected")
        for waiter in waiters {
            waiter(connection)
        }
        return true
    }

    private func dropped(_ connection: NSXPCConnection?) {
        lock.lock()
        let current = connection != nil && self.connection === connection
        if current {
            self.connection = nil
        }
        lock.unlock()
        if current {
            log.info("app connection lost")
        }
    }

    // MARK: - PhoneFilesExtension

    func attach(reply: @escaping @Sendable () -> Void) {
        reply()
    }

    func progress(requestID: String, done: Int64, total: Int64) {
        lock.lock()
        let handler = progressHandlers[requestID]
        lock.unlock()
        handler?(done, total)
    }

    func onProgress(_ requestID: String, _ handler: ((Int64, Int64) -> Void)?) {
        lock.lock()
        progressHandlers[requestID] = handler
        lock.unlock()
    }

    // MARK: - Calls

    private func proxy(_ connection: NSXPCConnection, failure: ((Error) -> Void)?) -> PhoneFilesHost? {
        connection.remoteObjectProxyWithErrorHandler { error in
            log.error("call to the app failed: \(String(describing: error), privacy: .public); the call fails as unreachable")
            failure?(NSFileProviderError(.serverUnreachable))
        } as? PhoneFilesHost
    }

    /// Calls `body` with the app's proxy, or `failure` when the app does not connect in time. A
    /// connection that breaks during the call also ends in `failure`.
    func withHost(_ failure: @escaping (Error) -> Void, _ body: @escaping (PhoneFilesHost) -> Void) {
        lock.lock()
        if let connection {
            lock.unlock()
            guard let host = proxy(connection, failure: failure) else {
                return failure(NSFileProviderError(.serverUnreachable))
            }
            return body(host)
        }

        let once = Once()
        let first = waiting.isEmpty
        waiting.append { [weak self] connection in
            guard once.claim() else { return }
            guard let self, let host = self.proxy(connection, failure: failure) else {
                return failure(NSFileProviderError(.serverUnreachable))
            }
            body(host)
        }
        lock.unlock()

        if first {
            askForHost()
        }
        DispatchQueue.global().asyncAfter(deadline: .now() + Self.attachWait) {
            guard once.claim() else { return }
            log.notice("app did not connect within 10 s; the call fails as unreachable")
            failure(NSFileProviderError(.serverUnreachable))
        }
    }
}

/// Lets exactly one of several paths finish a call.
final class Once: @unchecked Sendable {
    private let lock = NSLock()
    private var claimed = false

    func claim() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        if claimed { return false }
        claimed = true
        return true
    }
}

/// The extension's service: the app connects through it.
final class ServiceSource: NSObject, NSFileProviderServiceSource {
    var serviceName: NSFileProviderServiceName { PhoneFilesXPC.serviceName }

    func makeListenerEndpoint() throws -> NSXPCListenerEndpoint {
        HostConnection.shared.listener.endpoint
    }
}

/// The File Provider error a host error stands for.
func providerError(_ error: Error?) -> Error? {
    guard let error else { return nil }
    let ns = error as NSError
    guard ns.domain == PhoneFilesXPC.errorDomain, let code = PhoneFilesXPCError(rawValue: ns.code) else {
        return error
    }

    switch code {
    case .notRunning, .unavailable:
        return NSFileProviderError(.serverUnreachable)
    case .unknownItem, .fileNotFound:
        return NSFileProviderError(.noSuchItem)
    case .exists:
        return NSFileProviderError(.filenameCollision)
    case .ignored:
        return NSFileProviderError(.excludedFromSync)
    case .notSupported:
        return CocoaError(.featureUnsupported, userInfo: [NSLocalizedDescriptionKey: ns.localizedDescription])
    case .cancelled:
        return CocoaError(.userCancelled)
    case .notInSync, .permissionRequired, .inProgress, .partial, .failed:
        return NSFileProviderError(.cannotSynchronize, userInfo: [NSLocalizedDescriptionKey: ns.localizedDescription])
    }
}

/// The engine's identifier of `identifier`.
func engineIdentifier(_ identifier: NSFileProviderItemIdentifier) -> String {
    identifier == .rootContainer ? "" : identifier.rawValue
}
