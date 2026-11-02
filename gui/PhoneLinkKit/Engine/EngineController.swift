import Foundation
import Observation
import os
import PhoneLinkEngine

private let logger = AppLog(category: "engine")

/// A Microsoft-account sign-in (or per-client-id consent) page the engine asks the app to host in a
/// web view while `Engine.start()` waits. `requestId` identifies it when answering the engine.
public struct WebSignIn: Equatable, Sendable {
    public let requestId: String
    public let request: WebSignInRequest

    public init(requestId: String, request: WebSignInRequest) {
        self.requestId = requestId
        self.request = request
    }
}

/// Owns the engine for the app: starts it, follows its session state, and hands the feature objects to
/// the models. Everything here runs on the main actor; the engine's own work runs on its runtime.
@MainActor
@Observable
public final class EngineController {
    public enum Phase: Equatable, Sendable {
        /// Not started yet.
        case idle
        case starting
        /// `start()` waits for the user to complete this sign-in / consent page in a web view.
        case webSignIn(WebSignIn)
        /// Signed in; no phone is selected: the user picks one of `phones`.
        case selectingPhone
        case running
        /// The last start failed; `start()` tries again.
        case failed(message: String)
        /// The account was removed while the engine ran; `start()` signs in and enrolls anew.
        case accountRemoved
        /// The user signed out; `start()` signs in as any account.
        case signedOut
    }

    public private(set) var phase: Phase = .idle
    /// The account's linked phones the user can select (filled while `selectingPhone`, and by
    /// `reloadPhones()`).
    public private(set) var phones: [PhoneChoice] = []
    /// The selected phone, once the engine has started.
    public private(set) var phone: PhoneInfo?
    /// The platform session with the phone is up.
    public private(set) var connected = false
    /// The phone turned every feature off for this Mac.
    public private(set) var allFeaturesDisabled = false
    /// The selected phone left the account's linked devices.
    public private(set) var phoneUnlinked = false
    /// Why the last connect the user or the app's activation started did not reach the phone; `nil`
    /// once a connect succeeds or the session is up.
    public private(set) var connectFailure: String?

    public let hub: EngineEventHub
    public let notifications: any NotificationsApiProtocol
    public let messaging: any MessagingApiProtocol
    public let contacts: any ContactsApiProtocol
    public let calling: any CallingApiProtocol
    public let photos: any PhotosApiProtocol
    public let remoteSnipping: any RemoteSnippingApiProtocol
    public let fileTransfer: any FileTransferApiProtocol
    public let phoneFiles: any PhoneFilesApiProtocol
    public let connectedCamera: any ConnectedCameraApiProtocol
    public let phoneApps: any PhoneAppsApiProtocol
    public let screenMirroring: any ScreenMirroringApiProtocol
    public let deviceStatus: any DeviceStatusApiProtocol
    public let wallpaper: any WallpaperApiProtocol
    public let clipboard: any ClipboardApiProtocol
    public let remoteControl: any RemoteControlApiProtocol
    public let audio: any AudioApiProtocol
    public let hotspot: any HotspotApiProtocol
    public let phoneApi: any PhoneApiProtocol
    public let pairing: any PairingApiProtocol

    /// Called when the phone asks to bring the window forward (with its `ms-phone:` URI).
    public var onBringWindowForward: ((String) -> Void)?
    /// Called when a start ends signed in with no phone selected: the app shows the phone picker.
    public var onPhoneSelectionNeeded: (() -> Void)?

    private let engine: any EngineProtocol
    private var startTask: Task<Void, Never>?
    /// Responses to notifications that arrived before the engine started — the click that launched
    /// the app — sent once it has.
    private var pendingResponses: [(id: String, userInfo: [String: String], response: NotificationResponse)] = []

    public init(engine: any EngineProtocol, hub: EngineEventHub) {
        self.engine = engine
        self.hub = hub

        notifications = engine.notifications()
        messaging = engine.messaging()
        contacts = engine.contacts()
        calling = engine.calling()
        photos = engine.photos()
        remoteSnipping = engine.remoteSnipping()
        fileTransfer = engine.fileTransfer()
        phoneFiles = engine.phoneFiles()
        connectedCamera = engine.connectedCamera()
        phoneApps = engine.phoneApps()
        screenMirroring = engine.screenMirroring()
        deviceStatus = engine.deviceStatus()
        wallpaper = engine.wallpaper()
        clipboard = engine.clipboard()
        remoteControl = engine.remoteControl()
        audio = engine.audio()
        hotspot = engine.hotspot()
        phoneApi = engine.phone()
        pairing = engine.pairing()

        hub.onEvent { [weak self] event in
            self?.handle(event)
        }
    }

    /// Starts the engine unless a start is under way. While it waits for sign-in the phase is
    /// `signingIn`; a start that fails leaves `failed` and can be tried again.
    public func start() {
        guard startTask == nil else { return }
        phase = .starting
        startTask = Task { [weak self, engine] in
            do {
                let status = try await engine.start()
                if status.needsPhoneSelection {
                    self?.awaitPhoneSelection()
                } else {
                    self?.started()
                }
            } catch {
                self?.startFailed(error, subject: "engine not started")
            }
        }
    }

    private func started() {
        startTask = nil
        phase = .running
        phoneUnlinked = false

        reloadPhone()
        reloadPhones()
        hub.postStarted()

        let responses = pendingResponses
        pendingResponses = []
        for pending in responses {
            send(id: pending.id, userInfo: pending.userInfo, response: pending.response)
        }
    }

    private func awaitPhoneSelection() {
        startTask = nil
        reloadPhones()
        phase = .selectingPhone
        onPhoneSelectionNeeded?()
    }

    /// Re-reads the account's linked phones.
    public func reloadPhones() {
        do {
            phones = try phoneApi.phones()
        } catch FeatureApiError.NotStarted {
            phones = []
        } catch {
            logger.error("linked phones not read: \(error: error)")
            phones = []
        }
    }

    /// The user selected the phone `id`: the first selection starts the phone session; a later one
    /// switches to it (the previous phone is disconnected) and every model reloads for it.
    public func selectPhone(_ id: String) {
        guard startTask == nil else { return }
        if phase == .selectingPhone { phase = .starting }
        startTask = Task { [weak self, phoneApi] in
            do {
                try await phoneApi.select(id: id)
                self?.started()
            } catch {
                self?.startFailed(error, subject: "phone not selected")
            }
        }
    }

    private func startFailed(_ error: Error, subject: StaticString) {
        startTask = nil
        logger.error("\(subject.description, privacy: .public): \(error: error)")

        // A start that finds the account removed announces it before it fails; that state says
        // what to do (sign in again).
        guard phase != .accountRemoved else { return }
        phase = .failed(message: UserText.describe(error))
    }

    /// The start under way, for tests to wait on.
    var pendingStart: Task<Void, Never>? { startTask }

    /// The user asked to reconnect to the phone now: an immediate connect while the engine runs, a
    /// start when none ran or the last one ended, and nothing while a start is under way (it
    /// connects when it finishes).
    public func refresh() {
        switch phase {
        case .running:
            do {
                try engine.refresh()
            } catch {
                logger.error("reconnect not started: \(error: error)")
                phase = .failed(message: UserText.describe(error))
            }
        case .idle, .failed, .accountRemoved:
            start()
        case .starting, .webSignIn, .selectingPhone, .signedOut:
            break
        }
    }

    /// The app became active (`true`) or resigned active.
    public func setForeground(_ foreground: Bool) {
        engine.setForeground(foreground: foreground)
    }

    /// The Mac's screen was locked or unlocked, or it sleeps or woke.
    public func systemEvent(_ event: SystemEvent) {
        engine.systemEvent(event: event)
    }

    /// What the user did with a notification the engine posted: held until a start under way or still
    /// to come has finished, dropped once the account that posted it was removed.
    public func notificationResponse(id: String, userInfo: [String: String], response: NotificationResponse) {
        switch phase {
        case .running:
            send(id: id, userInfo: userInfo, response: response)
        case .idle, .starting, .webSignIn, .selectingPhone, .failed:
            pendingResponses.append((id, userInfo, response))
        case .accountRemoved, .signedOut:
            logger.debug("notification response dropped; its account was removed")
        }
    }

    private func send(id: String, userInfo: [String: String], response: NotificationResponse) {
        do {
            try engine.notificationResponse(id: id, userInfo: userInfo, response: response)
        } catch {
            logger.error("notification response not sent: \(error: error)")
        }
    }

    /// Ends the phone session (the app quits).
    public func stop() async {
        do {
            try await engine.stop()
        } catch {
            logger.notice("engine stop failed: \(error: error)")
        }
    }

    /// Whether the user can sign out now: not while a start or a phone selection is under way (it
    /// would go on for the account being signed out), nor once signed out.
    public var canSignOut: Bool {
        startTask == nil && phase != .signedOut
    }

    /// The user's sign-out, local only: the engine ends the phone session, both PC identities forget
    /// the Microsoft account, and this Mac's copies of the phone's data are deleted. The models drop
    /// what they show, as at an account removal; `start()` then signs in as any account.
    public func signOut() async throws {
        guard canSignOut else { return }
        try await engine.signOut()

        pendingResponses = []
        phone = nil
        phones = []
        connected = false
        allFeaturesDisabled = false
        phoneUnlinked = false
        connectFailure = nil

        hub.post(.accountRemoved)
        phase = .signedOut
    }

    /// This Mac's device-bound registration (device PUID and its `distinguishedDeviceId`s), or `nil`
    /// when the machine is not registered.
    public func deviceRegistration() -> DeviceRegistration? {
        engine.deviceRegistration()
    }

    /// Resets the machine's device registration: deletes this Mac's records from the account, signs
    /// out, and forgets the device credential. The next sign-in registers it afresh. Behaves like a
    /// sign-out for the app's state.
    public func resetDeviceRegistration() async throws {
        guard canSignOut else { return }
        try await engine.resetDeviceRegistration()

        pendingResponses = []
        phone = nil
        phones = []
        connected = false
        allFeaturesDisabled = false
        phoneUnlinked = false
        connectFailure = nil

        hub.post(.accountRemoved)
        phase = .signedOut
    }

    /// The web view finished the current sign-in / consent page with the JSON the page posted. The
    /// engine resumes the start; a further page may follow.
    public func completeWebSignIn(json: String) {
        guard case let .webSignIn(signIn) = phase else { return }
        phase = .starting
        engine.completeWebSignIn(requestId: signIn.requestId, resultJson: json)
    }

    /// The user closed the web view (`declined`) or it could not run. The start fails and can retry.
    public func cancelWebSignIn(declined: Bool) {
        guard case let .webSignIn(signIn) = phase else { return }
        engine.cancelWebSignIn(requestId: signIn.requestId, declined: declined)
    }

    private func handle(_ event: EngineEvent) {
        switch event {
        case let .webSignInRequested(requestId, request):
            phase = .webSignIn(WebSignIn(requestId: requestId, request: request))
        case let .phoneSession(connected, _):
            self.connected = connected
            if connected {
                connectFailure = nil
            }
            reloadPhone()
        case .phoneUnlinked:
            phoneUnlinked = true
        case .accountRemoved:
            phase = .accountRemoved
            connected = false
        case let .allFeaturesDisabled(disabled):
            allFeaturesDisabled = disabled
        case .phoneChanged:
            reloadPhone()
        case let .bringWindowForward(uri):
            onBringWindowForward?(uri)
        case let .connectCompleted(immediate, outcome):
            if let failure = UserText.failure(outcome) {
                // A background connect's failure is retried without the user; it is not news.
                if immediate {
                    connectFailure = failure
                }
            } else {
                connectFailure = nil
            }
        case .notifications, .clipboard, .deviceStatus, .messaging, .contacts, .calling, .photos, .remoteControl, .wallpaper, .remoteSnipping, .fileTransfer, .phoneFiles, .connectedCamera, .phoneApps, .screenMirroring, .audio, .pairing, .resync:
            break
        }
    }

    private func reloadPhone() {
        let info: PhoneInfo
        do {
            info = try phoneApi.info()
        } catch FeatureApiError.NotStarted, FeatureApiError.NoPhone {
            return
        } catch {
            logger.notice("phone not read: \(error: error); the last one read stays shown")
            return
        }

        phone = info
        connected = info.connected
        allFeaturesDisabled = info.allFeaturesDisabled
    }
}
