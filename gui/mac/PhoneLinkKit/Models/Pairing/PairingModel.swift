import Foundation
import Observation
import os
import PhoneLinkEngine

/// Pairing a phone to the signed-in Microsoft account by QR code or PIN: the pages of the pairing
/// view, from the engine's session steps. The QR code is the entry; *Pair manually* shows the manual
/// instructions, whose *Continue* starts a PIN session in place of the QR one.
///
/// Two limits are the view's own: a page still loading its code after `loadingLimit` fails and the
/// session is cancelled; a wait for the phone's permissions longer than `permissionsLimit` shows the
/// error page (the session goes on until the next start or cancel).
@MainActor
@Observable
public final class PairingModel {
    public enum Page: Equatable {
        /// No session yet.
        case idle
        /// A session of this flow is being created.
        case loading(PairingFlow)
        /// Show this QR code; `manualLink` is for the manual instructions.
        case qrCode(PairingCode, manualLink: String)
        /// How to pair by hand: open `link` on the phone and sign in with `accountName`.
        case manualInstructions(link: String, accountName: String?)
        case pin(PairingCode, PairingPinKind)
        /// The shown PIN expired or was wrong; a challenge PIN can be refreshed.
        case pinProblem(PairingCode, PairingPinKind, PairingPinProblem, refreshing: Bool, failure: PairingRefreshFailure?)
        case waitingForPermissions
        /// The phone joined; this Mac is learning it.
        case completing
        case paired
        case failed(Failure)
    }

    public enum Failure: Equatable {
        /// The QR code, PIN or session expired; a new one can be requested.
        case expired
        /// No code came within the loading limit.
        case loadingTookTooLong
        /// The phone's permissions did not come within the permissions limit.
        case permissionsTookTooLong
        /// The session could not start (`message` says why).
        case couldNotStart(String)
        case ended(PairingResult)
    }

    public private(set) var page: Page = .idle
    /// The flow of the current or last session.
    public private(set) var flow: PairingFlow = .qrCode
    /// Called with the phone's id when a session pairs it (the app selects that phone).
    @ObservationIgnored public var onPaired: ((String) -> Void)?

    private let api: any PairingApiProtocol
    private let loadingLimit: Duration
    private let permissionsLimit: Duration
    @ObservationIgnored private var session: UInt64?
    /// The QR session's last code and manual link.
    @ObservationIgnored private var lastScan: (code: PairingCode, manualLink: String)?
    @ObservationIgnored private var limitTask: Task<Void, Never>?

    private static let log = AppLog(category: "pairing")

    public init(
        api: any PairingApiProtocol,
        hub: EngineEventHub,
        loadingLimit: Duration = .seconds(35),
        permissionsLimit: Duration = .seconds(180)
    ) {
        self.api = api
        self.loadingLimit = loadingLimit
        self.permissionsLimit = permissionsLimit

        hub.onEvent { [weak self] event in
            if case let .pairing(update) = event {
                self?.apply(update)
            }
        }
    }

    /// Starts a session of `flow`, cancelling the one in progress.
    public func start(_ flow: PairingFlow = .qrCode) {
        self.flow = flow
        lastScan = nil
        do {
            session = try api.start(flow: flow)
        } catch {
            session = nil
            Self.log.error("pairing not started: \(error: error)")
            show(.failed(.couldNotStart(UserText.describe(error))))
            return
        }

        show(.loading(flow))
    }

    /// From the QR page: how to pair by hand, with the QR session's manual link. The QR session goes
    /// on until *Continue*.
    public func showManualInstructions() {
        guard let lastScan else { return }
        show(.manualInstructions(link: lastScan.manualLink, accountName: api.accountName()))
    }

    /// From the manual instructions: back to the QR code — the running QR session's, else a new one.
    public func backToQrCode() {
        if session != nil, flow == .qrCode, case .manualInstructions = page, let lastScan {
            show(.qrCode(lastScan.code, manualLink: lastScan.manualLink))
        } else {
            start(.qrCode)
        }
    }

    /// From the manual instructions: a PIN session in place of the QR one.
    public func continueManually() {
        start(.manualPin)
    }

    /// Asks for a new challenge PIN.
    public func refreshPin() {
        guard case let .pinProblem(pin, kind, problem, false, failure) = page, kind == .challenge else { return }
        if api.refreshChallengePin() {
            page = .pinProblem(pin, kind, problem, refreshing: true, failure: failure)
        }
    }

    /// Ends the session (the user left pairing).
    public func cancel() {
        limitTask?.cancel()
        api.cancel()
        session = nil
        page = .idle
    }

    private func apply(_ update: PairingUpdate) {
        guard update.session == session else { return }
        switch update.step {
        case let .creating(flow):
            show(.loading(flow))
        case let .waitingForScan(qrCode, manualLink):
            lastScan = (qrCode, manualLink)
            // The manual instructions stay up while the QR session goes on.
            if case .manualInstructions = page { return }
            show(.qrCode(qrCode, manualLink: manualLink))
        case let .waitingForPin(pin, kind):
            show(.pin(pin, kind))
        case let .pinRefresh(pin, kind, problem, refreshing, failure):
            show(.pinProblem(pin, kind, problem, refreshing: refreshing, failure: failure))
        case .waitingForPermissions:
            if page != .waitingForPermissions { show(.waitingForPermissions) }
        case .completing:
            show(.completing)
        case let .ended(result):
            session = nil
            ended(result)
        }
    }

    private func ended(_ result: PairingResult) {
        switch result {
        case let .success(phoneId):
            show(.paired)
            onPaired?(phoneId)
        case .canceled:
            // A cancel this model asked for has already set the page.
            if case .failed = page { return }
            show(.idle)
        case .sessionExpired:
            show(.failed(.expired))
        default:
            // The engine logs the result where it ends the session.
            show(.failed(.ended(result)))
        }
    }

    /// Shows `page` and starts the limit that page has, if any.
    private func show(_ newPage: Page) {
        page = newPage
        limitTask?.cancel()
        limitTask = nil

        let (limit, failure): (Duration, Failure)
        switch newPage {
        case .loading:
            (limit, failure) = (loadingLimit, .loadingTookTooLong)
        case .waitingForPermissions:
            (limit, failure) = (permissionsLimit, .permissionsTookTooLong)
        default:
            return
        }

        let shown = newPage
        limitTask = Task { [weak self] in
            try? await Task.sleep(for: limit)
            guard !Task.isCancelled, let self, self.page == shown else { return }
            if failure == .loadingTookTooLong {
                Self.log.error("pairing code not received in time; the session is cancelled")
                self.api.cancel()
                self.session = nil
            } else {
                Self.log.error("phone permissions not granted in time; pairing shows the failure")
            }
            self.page = .failed(failure)
        }
    }
}

extension PairingModel.Failure {
    /// What the error page says.
    public var message: String {
        switch self {
        case .expired:
            return String(localized: "The code expired. Get a new one to try again.")
        case .loadingTookTooLong:
            return String(localized: "Phone Link couldn't get a code. Check your internet connection and try again.")
        case .permissionsTookTooLong:
            return String(localized: "This is taking longer than expected. Check your phone, then try again.")
        case let .couldNotStart(reason):
            return reason
        case let .ended(result):
            switch result {
            case .accountMismatched:
                return String(localized: "Your phone is signed in to a different Microsoft account. Sign in to Link to Windows with the account you use on this Mac.")
            case .tokenInvalid, .accountTokenUnavailable:
                return String(localized: "Phone Link couldn't use your Microsoft account. Sign out and sign in again, then try again.")
            case .networkUnavailable:
                return String(localized: "Phone Link couldn't reach Microsoft. Check your internet connection and try again.")
            case .timedOut:
                return String(localized: "Microsoft didn't answer in time. Try again.")
            case .phoneNotFound:
                return String(localized: "Your phone joined your account, but Phone Link couldn't find it yet. Try again in a moment.")
            case .sessionExpired:
                return Self.expired.message
            case .serviceError, .internalError, .success, .canceled:
                return String(localized: "Pairing didn't finish. Try again.")
            }
        }
    }
}

extension PairingRefreshFailure {
    /// Why a new PIN could not be had.
    public var message: String {
        switch self {
        case .networkUnavailable: String(localized: "Check your internet connection and try again.")
        case .timeoutError: String(localized: "Microsoft didn't answer in time. Try again.")
        case .unauthorizedRequest, .msaUnauthorizedRequest: String(localized: "Phone Link couldn't use your Microsoft account.")
        case .forbiddenDcgRequest, .internalError: String(localized: "Phone Link couldn't get a new PIN.")
        }
    }
}
