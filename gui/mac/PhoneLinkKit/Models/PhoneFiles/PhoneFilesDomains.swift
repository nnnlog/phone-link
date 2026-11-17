import FileProvider
import Foundation
import os
import PhoneLinkEngine

private let log = AppLog(category: "phone-files")

/// The phone's File Provider domain: added while phone files runs for the selected phone (display
/// name = the phone's name) and removed, with its local files, once it stops. While the domain exists
/// the app keeps a connection to the domain's extension and serves its calls (`PhoneFilesHostService`);
/// every change of the tree signals the working set. The system starts and stops extension processes
/// on its own and a connection ends with its process, so the app connects again whenever an extension
/// process asks for it (`PhoneFilesXPC.needsHostNotification`), and after each connection tells the
/// system to retry what failed while the app was unreachable.
@MainActor
public final class PhoneFilesDomains {
    /// How long a lost connection waits before the app connects again.
    private static let reconnectDelay: Duration = .seconds(3)

    private let api: any PhoneFilesApiProtocol
    private let service: PhoneFilesHostService
    private var domain: NSFileProviderDomain?
    private var connection: NSXPCConnection?
    private var reconciling: Task<Void, Never>?
    private var reconcileAgain = false
    /// A connection is being asked for; a second request waits for it.
    private var connecting = false
    /// A connect failed since the last connection (logged once: the app retries every few seconds).
    private var connectFailureLogged = false
    private var needsHostObserver: NSObjectProtocol?

    public init(api: any PhoneFilesApiProtocol, hub: EngineEventHub) {
        self.api = api
        service = PhoneFilesHostService(api: api)

        hub.onStart { [weak self] in self?.reconcile() }
        needsHostObserver = DistributedNotificationCenter.default().addObserver(
            forName: PhoneFilesXPC.needsHostNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.extensionNeedsHost() }
        }

        hub.onEvent { [weak self] event in
            guard case let .phoneFiles(change) = event else { return }
            switch change {
            case .running:
                self?.reconcile()
            case let .itemsChanged(domain):
                self?.signal(domain)
            case .stateChanged, .recycleBinNotice:
                break
            }
        }
    }

    /// Brings the system's domains in line with the engine's state, one pass at a time.
    public func reconcile() {
        guard reconciling == nil else {
            reconcileAgain = true
            return
        }

        reconciling = Task { [weak self] in
            await self?.reconcileNow()
            guard let self else { return }
            reconciling = nil
            if reconcileAgain {
                reconcileAgain = false
                reconcile()
            }
        }
    }

    private func reconcileNow() async {
        let state: PhoneFilesState?
        do {
            state = try api.state()
        } catch let error as FeatureApiError where error == .NotStarted || error == .NoPhone {
            state = nil
        } catch {
            log.error("phone-files state not read: \(error: error); the phone's folder is removed")
            state = nil
        }

        let wanted: NSFileProviderDomain? = state.flatMap { s in
            guard s.running else { return nil }
            let name = s.phoneName.isEmpty ? String(localized: "Phone") : s.phoneName
            let domain = NSFileProviderDomain(identifier: NSFileProviderDomainIdentifier(s.domain), displayName: name)
            domain.supportsSyncingTrash = true
            return domain
        }

        let existing: [NSFileProviderDomain]
        do {
            existing = try await NSFileProviderManager.domains()
        } catch {
            log.error("file provider domains not listed: \(error: error)")
            return
        }

        for d in existing where d.identifier != wanted?.identifier {
            disconnect()
            do {
                try await NSFileProviderManager.remove(d)
                log.info("phone-files domain \(d.identifier.rawValue, privacy: .private) removed")
            } catch {
                log.error("phone-files domain \(d.identifier.rawValue, privacy: .private) not removed: \(error: error)")
            }
        }

        guard let wanted else {
            domain = nil
            disconnect()
            return
        }

        if let present = existing.first(where: { $0.identifier == wanted.identifier }) {
            domain = present
        } else {
            do {
                try await NSFileProviderManager.add(wanted)
                log.info("phone-files domain \(wanted.identifier.rawValue, privacy: .private) added")
                domain = wanted
            } catch {
                log.error("phone-files domain \(wanted.identifier.rawValue, privacy: .private) not added: \(error: error)")
                return
            }
        }

        if connection == nil, let domain {
            connect(domain)
        }
        signal(wanted.identifier.rawValue)
    }

    /// An extension process has no connection to the app: the one the app holds went to a process that
    /// is gone (or is about to be replaced), so it connects again.
    private func extensionNeedsHost() {
        guard let domain else { return }
        log.debug("phone-files extension asks for a connection")
        disconnect()
        connect(domain)
    }

    /// Asks the system for the extension's service and its connection; `attach` takes it on the main
    /// actor.
    private func connect(_ domain: NSFileProviderDomain) {
        guard !connecting, let manager = NSFileProviderManager(for: domain) else { return }
        connecting = true

        manager.getService(named: PhoneFilesXPC.serviceName, for: .rootContainer) { [weak self] service, error in
            guard let service else {
                let box = ErrorBox(error)
                Task { @MainActor in
                    self?.connectFailed("phone-files extension service not found", box.error)
                }
                return
            }

            service.getFileProviderConnection { connection, error in
                let box = ConnectionBox(connection, error)
                Task { @MainActor in
                    self?.connecting = false
                    guard let connection = box.connection else {
                        self?.connectFailed("phone-files extension not reached", box.error)
                        return
                    }
                    self?.attach(connection)
                }
            }
        }
    }

    private func attach(_ connection: NSXPCConnection) {
        guard self.connection == nil, domain != nil else {
            connection.invalidate()
            return
        }

        connection.remoteObjectInterface = NSXPCInterface(with: PhoneFilesExtension.self)
        connection.exportedInterface = NSXPCInterface(with: PhoneFilesHost.self)
        connection.exportedObject = service

        let id = ObjectIdentifier(connection)
        connection.invalidationHandler = { [weak self] in
            Task { @MainActor in self?.connectionLost(id) }
        }
        connection.interruptionHandler = { [weak self] in
            Task { @MainActor in self?.connectionLost(id) }
        }
        connection.resume()

        let callFailed = LoggedOnce()
        let proxy = ExtensionProxy(connection.remoteObjectProxyWithErrorHandler { error in
            // Progress reports call it for each chunk: the first failure of a connection is logged.
            if callFailed.first() {
                log.notice("call to the phone-files extension failed: \(error: error); later failures of this connection are not logged")
            }
        } as? PhoneFilesExtension)
        service.setProgressSink { id, done, total in proxy.extension?.progress(requestID: id, done: done, total: total) }
        proxy.extension?.attach {}

        self.connection = connection
        connectFailureLogged = false
        log.info("phone-files extension connected")

        // What failed while the app could not be reached is tried again now, not after the system's back-off.
        if let domain, let manager = NSFileProviderManager(for: domain) {
            manager.signalErrorResolved(NSFileProviderError(.serverUnreachable)) { error in
                if let error {
                    log.notice("unreachable errors not cleared: \(error: error); the system retries after its back-off")
                }
            }
        }
    }

    private func disconnect() {
        service.setProgressSink(nil)
        connection?.invalidationHandler = nil
        connection?.interruptionHandler = nil
        connection?.invalidate()
        connection = nil
    }

    private func connectionLost(_ lost: ObjectIdentifier) {
        // A connection already replaced ends without effect.
        guard let current = connection, ObjectIdentifier(current) == lost else { return }
        log.info("phone-files extension connection lost")
        connection = nil
        service.setProgressSink(nil)
        scheduleReconnect()
    }

    private func connectFailed(_ subject: StaticString, _ error: Error?) {
        connecting = false
        if !connectFailureLogged {
            connectFailureLogged = true
            if let error {
                log.error("\(subject.description, privacy: .public): \(error: error); retried every 3 s, later failures are not logged")
            } else {
                log.error("\(subject.description, privacy: .public); retried every 3 s, later failures are not logged")
            }
        }
        scheduleReconnect()
    }

    private func scheduleReconnect() {
        Task { [weak self] in
            try? await Task.sleep(for: Self.reconnectDelay)
            guard let self, connection == nil, let domain else { return }
            connect(domain)
        }
    }

    private func signal(_ domainID: String) {
        guard let domain, domain.identifier.rawValue == domainID, let manager = NSFileProviderManager(for: domain) else { return }
        manager.signalEnumerator(for: .workingSet) { error in
            if let error {
                log.notice("working set not signalled: \(error: error); the Finder shows the change later")
            }
        }
    }

    /// The domain's folder in the Finder; `nil`, logged, when there is none.
    public func userVisibleURL() async -> URL? {
        guard let domain, let manager = NSFileProviderManager(for: domain) else {
            log.notice("phone folder not found in the Finder; phone files has no domain yet")
            return nil
        }

        do {
            return try await manager.getUserVisibleURL(for: .rootContainer)
        } catch {
            log.error("phone folder not found in the Finder: \(error: error)")
            return nil
        }
    }
}

/// A connection handed from the system's callback to the main actor.
private final class ConnectionBox: @unchecked Sendable {
    let connection: NSXPCConnection?
    let error: Error?

    init(_ connection: NSXPCConnection?, _ error: Error? = nil) {
        self.connection = connection
        self.error = error
    }
}

/// An error handed from the system's callback to the main actor.
private final class ErrorBox: @unchecked Sendable {
    let error: Error?

    init(_ error: Error?) {
        self.error = error
    }
}

/// `true` the first time only, from any thread.
private final class LoggedOnce: @unchecked Sendable {
    private let lock = NSLock()
    private var done = false

    func first() -> Bool {
        lock.withLock {
            defer { done = true }
            return !done
        }
    }
}

/// The extension's side of the connection, handed to progress reports from the engine's threads.
private final class ExtensionProxy: @unchecked Sendable {
    let `extension`: PhoneFilesExtension?

    init(_ proxy: PhoneFilesExtension?) {
        `extension` = proxy
    }
}
