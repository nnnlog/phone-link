import Foundation
import Observation
import PhoneLinkEngine

private let log = AppLog(category: "calls")

/// What the Calls section shows in place of, or as, its history.
public enum CallsContent: Equatable, Sendable {
    /// The engine has not started.
    case nothing
    /// Call history is off on this Mac.
    case turnedOff
    /// The phone shares no call history: Link to Windows on it needs an update.
    case updateRequired
    /// The phone must allow Link to Windows to post notifications first.
    case needsNotifications
    /// The phone has not allowed its call history yet.
    case needsPhoneAccess
    /// No call is stored yet while the phone is asked for them or they are not up to date.
    case updating
    case empty
    case list
}

/// The section's waits.
struct CallsTiming: Sendable {
    /// Calls that are not up to date show as updating for at most this long, counted again each time
    /// they fall behind.
    var syncWait: Duration

    static let live = CallsTiming(syncWait: .seconds(120))
}

/// The Calls section: the phone's recent calls by day, the selected call's details, the prompt that
/// stands in for them while call history may not be used, and the missed-call badge.
///
/// The phone is asked for its calls once when none are stored, and when the user switches call
/// history on or allows it on the phone. Engine reads run off the main actor; a result a newer read
/// of the same kind superseded is dropped, and a failed read is logged while the last data stays.
/// Requests the user started report their failures as `alert`.
@MainActor
@Observable
public final class CallsModel {
    /// `nil` until the engine has started.
    public private(set) var state: CallingState?
    /// Every call, newest first.
    public private(set) var calls: [CallHistoryItem] = []
    /// The calls by day, filtered by the search.
    public private(set) var sections: [CallsSection] = []
    /// The search field's text: calls whose name or number holds it (a number by its digits).
    public var searchText = "" {
        didSet { applyFilter() }
    }
    /// The selected call's id.
    public var selection: Int64?
    public private(set) var isRequestingPermission = false
    /// The phone did not answer the last permission request.
    public private(set) var permissionTimedOut = false
    /// The wait for calls to come up to date ran out.
    public private(set) var syncWaitOver = false
    /// The failure of an action the user started.
    public var alert: MessagesAlert?
    /// How to allow notifications on the phone, while its sheet shows.
    public var postNotificationHelp: PostNotificationHelp?
    public let format: CallsFormat

    @ObservationIgnored private var syncWaitTask: Task<Void, Never>?
    @ObservationIgnored private var generations = CallsGenerations()
    @ObservationIgnored private var viewShown = false

    private let api: any CallingApiProtocol
    private let navigator: AppNavigator
    private let reads: any MessagesReading
    private let timing: CallsTiming

    public convenience init(api: any CallingApiProtocol, hub: EngineEventHub, navigator: AppNavigator) {
        self.init(api: api, hub: hub, navigator: navigator, reads: MessagesBackgroundReads(), timing: .live, format: CallsFormat())
    }

    init(
        api: any CallingApiProtocol,
        hub: EngineEventHub,
        navigator: AppNavigator,
        reads: any MessagesReading,
        timing: CallsTiming,
        format: CallsFormat
    ) {
        self.api = api
        self.navigator = navigator
        self.reads = reads
        self.timing = timing
        self.format = format

        hub.onEvent { [weak self] event in
            self?.handle(event)
        }
        hub.onStart { [weak self] in
            self?.engineStarted()
        }
    }

    // MARK: - What the views show

    /// The sidebar lists Calls only for an Android phone that has calling.
    public var isViewVisible: Bool {
        guard let state else { return false }
        return state.access != .unsupported
    }

    /// The sidebar's badge: missed calls not yet seen here.
    public var badgeCount: Int {
        Int(state?.unseenMissed ?? 0)
    }

    public var content: CallsContent {
        guard let state else { return .nothing }
        switch state.access {
        case .unsupported: return .nothing
        case .pcConsentRequired: return .turnedOff
        case .callLogsUnsupported: return .updateRequired
        case .postNotificationRequired: return .needsNotifications
        case .devicePermissionRequired: return .needsPhoneAccess
        case .granted: break
        }

        if !calls.isEmpty { return .list }
        return isUpdating ? .updating : .empty
    }

    /// The phone is being asked for its calls, or they are not up to date and the wait has not run
    /// out.
    public var isUpdating: Bool {
        guard let state, state.access == .granted else { return false }
        return state.refreshing || (!state.upToDate && !syncWaitOver)
    }

    /// The search filtered every call out.
    public var searchFoundNothing: Bool {
        !calls.isEmpty && sections.isEmpty
    }

    /// Whether `call` matches `query`: its shown name contains it (ignoring case), or its number
    /// contains the query's digits.
    public static func matches(_ call: CallHistoryItem, query: String) -> Bool {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return true }
        if CallsFormat.name(call).localizedCaseInsensitiveContains(query) { return true }
        let digits = query.filter(\.isNumber)
        return !digits.isEmpty && call.number.filter(\.isNumber).contains(digits)
    }

    /// The selected call.
    public var selectedCall: CallHistoryItem? {
        guard let selection else { return nil }
        return calls.first { $0.callId == selection }
    }

    /// Every call with the selected call's number, the selected one included, newest first; none for
    /// a private number.
    public var callsWithSelectedNumber: [CallHistoryItem] {
        guard let call = selectedCall, !call.number.isEmpty else { return [] }
        return calls.filter { $0.number == call.number }
    }

    // MARK: - Actions

    /// Turns on call history on this Mac; the phone is asked for its calls when none are stored.
    public func turnOn() {
        do {
            try api.setPcConsent(consent: true)
            reloadState()
        } catch {
            log.error("call history not turned on: \(error: error)")
            alert = MessagesAlert(title: String(localized: "Call History Not Turned On"), message: UserText.describe(error))
        }
    }

    /// Asks the phone to allow its call history; a grant is followed by a refresh.
    public func requestPermission() async {
        guard !isRequestingPermission else { return }
        isRequestingPermission = true
        permissionTimedOut = false
        defer { isRequestingPermission = false }

        do {
            let result = try await api.requestPermission()
            if result.timedOut {
                log.notice("call history permission request timed out")
                permissionTimedOut = true
            } else if let failure = UserText.failure(result.outcome) {
                log.error("call history permission not granted: \(result.outcome, privacy: .public)")
                alert = MessagesAlert(title: String(localized: "Call History Not Allowed"), message: failure)
            } else if let refresh = result.refresh, UserText.failure(refresh) != nil {
                log.notice("call history not refreshed after the grant: \(refresh, privacy: .public)")
            }

            reloadState()
        } catch {
            log.error("call history permission not requested: \(error: error)")
            alert = MessagesAlert(title: String(localized: "Call History Not Allowed"), message: UserText.describe(error))
        }
    }

    /// Shows how to allow Link to Windows to post notifications on the phone; nothing is sent.
    public func showPostNotificationHelp() {
        postNotificationHelp = PostNotificationHelp()
    }

    /// Messages, addressed to the call's number: its conversation, else a new message.
    public func message(_ call: CallHistoryItem) {
        guard !call.number.isEmpty else { return }
        navigator.showMessage(to: MessagesRecipient(address: call.number, name: call.name))
    }

    /// The Contacts section, with the call's contact selected.
    public func showContact(_ call: CallHistoryItem) {
        guard let id = call.contactId else { return }
        navigator.showContact(id)
    }

    /// The section is on screen: the missed calls stored so far stop counting in the badge.
    public func viewAppeared() {
        viewShown = true
        markSeen()
    }

    public func viewDisappeared() {
        viewShown = false
    }

    // MARK: - Engine events and loading

    private func handle(_ event: EngineEvent) {
        switch event {
        case let .calling(change):
            switch change {
            case .historyChanged:
                reloadHistory()
                reloadState()
            case .stateChanged:
                reloadState()
            case .controlChanged, .activeCalls:
                // Call control's; the history is unchanged.
                break
            }
        case .resync(.calling), .resync(.contacts), .contacts(.recordsChanged):
            // Contacts name the calls.
            reloadState()
            reloadHistory()
        case .phoneChanged:
            reloadState()
        default:
            break
        }
    }

    private func engineStarted() {
        syncWaitTask?.cancel()
        syncWaitTask = nil
        syncWaitOver = false
        permissionTimedOut = false

        reloadState()
        reloadHistory()
    }

    private func reloadState() {
        generations.state += 1
        let generation = generations.state

        reads.read({ [api] in try api.state() }) { [weak self] result in
            guard let self, generation == self.generations.state else { return }
            switch result {
            case let .success(loaded):
                let accessChanged = state != nil && state?.access != loaded.access
                if state?.upToDate == true && !loaded.upToDate {
                    restartUpToDateWait()
                }
                state = loaded
                if loaded.access != .postNotificationRequired {
                    postNotificationHelp = nil
                }
                waitForUpToDate()

                // The history is empty while the prompt stands in for it.
                if accessChanged {
                    reloadHistory()
                }
            case let .failure(error):
                record(error, "call history state not read")
            }
        }
    }

    /// Calls fell behind again: their wait starts over.
    private func restartUpToDateWait() {
        syncWaitTask?.cancel()
        syncWaitTask = nil
        syncWaitOver = false
    }

    /// Starts the wait for calls to come up to date when they are not and no wait runs or ran since
    /// they last fell behind.
    private func waitForUpToDate() {
        guard let state, !state.upToDate, syncWaitTask == nil, !syncWaitOver else { return }
        let wait = timing.syncWait
        syncWaitTask = Task { [weak self] in
            try? await Task.sleep(for: wait)
            guard !Task.isCancelled else { return }
            self?.syncWaitOver = true
        }
    }

    private func reloadHistory() {
        generations.history += 1
        let generation = generations.history

        reads.read({ [api] in try api.history() }) { [weak self] result in
            guard let self, generation == self.generations.history else { return }
            switch result {
            case let .success(loaded):
                calls = loaded
                applyFilter()
                if let selection, !loaded.contains(where: { $0.callId == selection }) {
                    self.selection = nil
                }
                if loaded.isEmpty && state?.access == .granted {
                    refreshIfEmpty()
                }
            case let .failure(error):
                record(error, "call history not read")
            }
        }
    }

    private func applyFilter() {
        let query = searchText
        sections = format.sections(calls.filter { Self.matches($0, query: query) })
    }

    /// The phone is asked for its calls once while none are stored; the engine keeps the "once".
    private func refreshIfEmpty() {
        Task { [api] in
            do {
                if let outcome = try await api.refreshIfEmpty(), UserText.failure(outcome) != nil {
                    log.notice("call history not refreshed: \(outcome, privacy: .public)")
                }
            } catch {
                if !Self.notStarted(error) {
                    log.error("call history not refreshed: \(error: error)")
                }
            }
        }
    }

    private func markSeen() {
        guard viewShown, state?.access == .granted else { return }
        do {
            try api.markSeen()
        } catch {
            record(error, "missed calls not marked seen")
        }
    }

    // MARK: - Failures

    /// A read that failed: the last data stays shown.
    private func record(_ error: any Error, _ subject: StaticString) {
        guard !Self.notStarted(error) else { return }
        log.error("\(subject.description, privacy: .public): \(error: error)")
    }

    private static func notStarted(_ error: any Error) -> Bool {
        if case FeatureApiError.NotStarted? = error as? FeatureApiError { true } else { false }
    }
}

/// The newest read of each kind; an older one's result is dropped.
private struct CallsGenerations {
    var state = 0
    var history = 0
}
